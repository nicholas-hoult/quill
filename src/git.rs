//! Git 信息：通过 git 命令行读取，不引入 libgit2。
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// M 修改 / A 新增 / D 删除 / ? 未跟踪
    pub status: char,
    /// 改动已进入暂存区
    pub staged: bool,
}

fn git(root: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

pub fn parse_porcelain(s: &str) -> Vec<Change> {
    s.lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let (x, y) = (l.as_bytes()[0] as char, l.as_bytes()[1] as char);
            let status = if l.starts_with("??") { '?' } else if x == 'A' || y == 'A' { 'A' } else if x == 'D' || y == 'D' { 'D' } else { 'M' };
            // 重命名显示成 `旧 -> 新`，只取新路径
            let raw = l[3..].trim_matches('"');
            let path = raw.rsplit(" -> ").next().unwrap_or(raw).to_string();
            Change { path, status, staged: x != ' ' && x != '?' && y == ' ' }
        })
        .collect()
}

pub fn status(root: &str) -> Vec<Change> { git(root, &["status", "--porcelain=v1"]).map(|s| parse_porcelain(&s)).unwrap_or_default() }

fn run(root: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").args(args).current_dir(root).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_string()) }
}

pub fn stage(root: &str, path: &str) -> Result<(), String> { run(root, &["add", "--", path]).map(|_| ()) }
pub fn stage_all(root: &str) -> Result<(), String> { run(root, &["add", "-A"]).map(|_| ()) }
pub fn unstage(root: &str, path: &str) -> Result<(), String> {
    // 没有任何提交的新仓库里 `restore --staged` 会失败，退回 rm --cached
    run(root, &["restore", "--staged", "--", path]).or_else(|_| run(root, &["rm", "--cached", "-q", "--", path])).map(|_| ())
}
/// 提交已暂存的内容，返回 git 的摘要输出
pub fn commit(root: &str, message: &str) -> Result<String, String> {
    if message.trim().is_empty() { return Err("提交信息不能为空".into()); }
    run(root, &["commit", "-m", message.trim()])
}

pub fn branch(root: &str) -> Option<String> {
    git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// HEAD 中该文件的内容，用于行级改动标记；新文件返回 None。
pub fn head_content(root: &str, rel: &str) -> Option<String> { git(root, &["show", &format!("HEAD:{rel}")]) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析状态() {
        let c = parse_porcelain(" M src/a.rs\n?? new.txt\nA  b.rs\n D gone.rs\n");
        let s: Vec<_> = c.iter().map(|x| x.status).collect();
        assert_eq!(s, vec!['M', '?', 'A', 'D']);
        assert_eq!(c[0].path, "src/a.rs");
    }

    #[test]
    fn 暂存状态与重命名() {
        let c = parse_porcelain("M  staged.rs
 M worktree.rs
MM both.rs
R  old.rs -> new.rs
");
        assert_eq!(c.iter().map(|x| x.staged).collect::<Vec<_>>(), vec![true, false, false, true]);
        assert_eq!(c[3].path, "new.rs");
    }

    #[test]
    fn 暂存提交与撤销暂存_真实仓库() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path().to_str().unwrap();
        let sh = |a: &[&str]| { let o = Command::new("git").args(a).current_dir(r).output().unwrap(); assert!(o.status.success(), "{a:?}: {}", String::from_utf8_lossy(&o.stderr)); };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@t.t"]);
        sh(&["config", "user.name", "t"]);
        std::fs::write(d.path().join("a.txt"), "1").unwrap();
        assert_eq!(status(r), vec![Change { path: "a.txt".into(), status: '?', staged: false }]);
        stage(r, "a.txt").unwrap();
        assert!(status(r)[0].staged);
        unstage(r, "a.txt").unwrap();
        assert!(!status(r)[0].staged, "空仓库里也应能撤销暂存");
        assert!(commit(r, "  ").is_err());
        stage_all(r).unwrap();
        commit(r, "feat: first").unwrap();
        assert!(status(r).is_empty());
        assert_eq!(branch(r).is_some(), true);
    }
}

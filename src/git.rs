//! Git 信息：通过 git 命令行读取，不引入 libgit2。
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    /// M 修改 / A 新增 / D 删除 / ? 未跟踪
    pub status: char,
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
            Change { path: l[3..].trim_matches('"').to_string(), status }
        })
        .collect()
}

pub fn status(root: &str) -> Vec<Change> { git(root, &["status", "--porcelain=v1"]).map(|s| parse_porcelain(&s)).unwrap_or_default() }

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
}

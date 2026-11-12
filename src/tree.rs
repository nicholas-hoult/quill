//! 文件树的扁平化：把「展开的目录集合」变成可直接渲染的行列表。
//! 错了表现为：展开子目录后顺序不对、深度缩进错位、隐藏文件夹（.git）冒出来。
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

const HIDDEN: &[&str] = &[".git", ".DS_Store", "node_modules", ".venv", "__pycache__", "dist", ".worktrees"];

/// 读取一个目录：目录在前、文件在后，各自按名称（不区分大小写）排序。
pub fn read_dir_sorted(dir: &Path) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| (e.file_name().to_string_lossy().to_string(), e.file_type().map(|t| t.is_dir()).unwrap_or(false))).collect())
        .unwrap_or_default();
    v.retain(|(n, _)| !HIDDEN.contains(&n.as_str()) && !n.starts_with("target"));
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    v
}

pub fn build_rows(root: &Path, expanded: &HashSet<String>) -> Vec<TreeRow> {
    fn walk(dir: &Path, depth: usize, expanded: &HashSet<String>, out: &mut Vec<TreeRow>) {
        for (name, is_dir) in read_dir_sorted(dir) {
            let path = dir.join(&name).to_string_lossy().to_string();
            let open = is_dir && expanded.contains(&path);
            out.push(TreeRow { path: path.clone(), name, depth, is_dir, expanded: open });
            if open { walk(Path::new(&path), depth + 1, expanded, out); }
        }
    }
    let mut out = vec![];
    walk(root, 0, expanded, &mut out);
    out
}

/// 项目内所有文件的相对路径（遵守 .gitignore），供 Cmd+P 使用。
pub fn list_files(root: &Path, limit: usize) -> Vec<String> {
    let mut out = vec![];
    for e in ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
        if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = e.path().strip_prefix(root) {
                out.push(rel.to_string_lossy().to_string());
                if out.len() >= limit { break; }
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, PartialEq, Clone)]
pub struct Hit { pub path: String, pub line: usize, pub text: String }

pub fn match_lines(content: &str, query: &str) -> Vec<(usize, String)> {
    if query.is_empty() { return vec![]; }
    let q = query.to_lowercase();
    content.lines().enumerate().filter(|(_, l)| l.to_lowercase().contains(&q)).map(|(i, l)| (i + 1, l.chars().take(240).collect())).collect()
}

/// 项目内文本搜索（不区分大小写，最多 `limit` 条）
pub fn search_text(root: &Path, query: &str, limit: usize) -> Vec<Hit> {
    let mut hits = vec![];
    for e in ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
        if e.metadata().map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(true) { continue; }
        let Ok(content) = std::fs::read_to_string(e.path()) else { continue };
        let rel = e.path().strip_prefix(root).unwrap_or(e.path()).to_string_lossy().to_string();
        for (line, text) in match_lines(&content, query) {
            hits.push(Hit { path: rel.clone(), line, text });
            if hits.len() >= limit { return hits; }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/inner")).unwrap();
        std::fs::create_dir_all(r.join(".git")).unwrap();
        std::fs::create_dir_all(r.join("Zdir")).unwrap();
        std::fs::write(r.join("b.txt"), "x").unwrap();
        std::fs::write(r.join("A.md"), "hello\nWorld").unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(r.join("src/inner/deep.rs"), "").unwrap();
        d
    }

    #[test]
    fn 目录在前_忽略大小写排序_隐藏_git() {
        let d = setup();
        let rows = build_rows(d.path(), &HashSet::new());
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["src", "Zdir", "A.md", "b.txt"]);
    }

    #[test]
    fn 展开后子项紧跟其后且深度加一() {
        let d = setup();
        let src = d.path().join("src").to_string_lossy().to_string();
        let inner = d.path().join("src/inner").to_string_lossy().to_string();
        let rows = build_rows(d.path(), &HashSet::from([src, inner]));
        let names: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.depth)).collect();
        assert_eq!(names[..4], [("src", 0), ("inner", 1), ("deep.rs", 2), ("main.rs", 1)]);
        assert!(rows[0].expanded);
    }

    #[test]
    fn 文件列表与搜索() {
        let d = setup();
        let files = list_files(d.path(), 100);
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(!files.iter().any(|f| f.starts_with(".git")));
        let hits = search_text(d.path(), "world", 10);
        assert_eq!(hits, vec![Hit { path: "A.md".into(), line: 2, text: "World".into() }]);
    }
}

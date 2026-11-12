//! Agent 可用的工具：只读的 read_file / list_dir / search 在这里执行；edit_file 不直接写盘，
//! 由界面把新内容放进编辑器走审阅。路径一律限制在项目根目录内。
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub fn defs() -> Value {
    json!([
        { "name": "read_file", "description": "读取项目内一个文本文件的完整内容。path 为相对项目根目录的路径。",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } },
        { "name": "list_dir", "description": "列出项目内一个目录的直接子项。path 为相对路径，\".\" 表示项目根目录。",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] } },
        { "name": "search", "description": "在项目文件内容中搜索文本（不区分大小写），返回「路径:行号: 内容」。",
          "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] } },
        { "name": "edit_file", "description": "提出对一个文件的修改：给出该文件修改后的完整新内容。修改不会直接写盘，会先进入用户的逐块审阅，用户可以接受或拒绝。文件不存在则视为新建。",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } }
    ])
}

/// 把相对路径安全地接到项目根上：拒绝绝对路径和 `..`。
pub fn safe_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel.trim());
    if p.is_absolute() { return Err("只能使用相对于项目根目录的路径".into()); }
    let mut out = root.to_path_buf();
    for c in p.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => {}
            _ => return Err("路径不能包含 ..".into()),
        }
    }
    Ok(out)
}

const MAX_READ: usize = 60_000;

/// 执行只读工具。edit_file 不在此处理。
pub fn run(root: &Path, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "read_file" => {
            let p = safe_path(root, input["path"].as_str().ok_or("缺少 path")?)?;
            let text = std::fs::read_to_string(&p).map_err(|e| format!("读取失败：{e}"))?;
            if text.len() > MAX_READ {
                let cut = (0..=MAX_READ).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
                return Ok(format!("{}\n…（文件过长，已截断，共 {} 字节）", &text[..cut], text.len()));
            }
            Ok(text)
        }
        "list_dir" => {
            let p = safe_path(root, input["path"].as_str().unwrap_or("."))?;
            let items = crate::tree::read_dir_sorted(&p);
            if items.is_empty() { return Ok("（空目录或不存在）".into()); }
            Ok(items.iter().map(|(n, d)| if *d { format!("dir  {n}/") } else { format!("file {n}") }).collect::<Vec<_>>().join("\n"))
        }
        "search" => {
            let q = input["query"].as_str().ok_or("缺少 query")?;
            let hits = crate::tree::search_text(root, q, 40);
            if hits.is_empty() { return Ok("没有匹配".into()); }
            Ok(hits.iter().map(|h| format!("{}:{}: {}", h.path, h.line, h.text.trim())).collect::<Vec<_>>().join("\n"))
        }
        other => Err(format!("未知工具：{other}")),
    }
}

/// 给界面看的一句话摘要，例如「读取 src/a.rs」
pub fn summary(name: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    match name {
        "read_file" => format!("读取 {}", s("path")),
        "list_dir" => format!("列出目录 {}", s("path")),
        "search" => format!("搜索「{}」", s("query")),
        "edit_file" => format!("修改 {}", s("path")),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/a.rs"), "fn main() { let Needle = 1; }").unwrap();
        std::fs::write(d.path().join("README.md"), "hi").unwrap();
        d
    }

    #[test]
    fn 路径不能逃出项目根() {
        let root = Path::new("/p");
        assert!(safe_path(root, "../etc/passwd").is_err());
        assert!(safe_path(root, "/etc/passwd").is_err());
        assert!(safe_path(root, "a/../../b").is_err());
        assert_eq!(safe_path(root, "./src/a.rs").unwrap(), PathBuf::from("/p/src/a.rs"));
        assert_eq!(safe_path(root, ".").unwrap(), PathBuf::from("/p"));
    }

    #[test]
    fn 读取_列目录_搜索() {
        let d = proj();
        assert_eq!(run(d.path(), "read_file", &json!({"path":"README.md"})).unwrap(), "hi");
        let l = run(d.path(), "list_dir", &json!({"path":"."})).unwrap();
        assert_eq!(l, "dir  src/\nfile README.md");
        let s = run(d.path(), "search", &json!({"query":"needle"})).unwrap();
        assert!(s.starts_with("src/a.rs:1:"), "{s}");
        assert!(run(d.path(), "read_file", &json!({"path":"../x"})).is_err());
        assert!(run(d.path(), "read_file", &json!({"path":"nope.txt"})).is_err());
        assert!(run(d.path(), "rm_rf", &json!({})).is_err());
    }

    #[test]
    fn 过长文件被截断且不切坏汉字() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("big.txt"), "汉".repeat(30_000)).unwrap();
        let r = run(d.path(), "read_file", &json!({"path":"big.txt"})).unwrap();
        assert!(r.contains("已截断"));
    }

    #[test]
    fn 摘要() {
        assert_eq!(summary("read_file", &json!({"path":"a.rs"})), "读取 a.rs");
        assert_eq!(summary("search", &json!({"query":"x"})), "搜索「x」");
    }
}

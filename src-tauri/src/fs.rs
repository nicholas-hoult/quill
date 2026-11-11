//! 文件系统命令：目录、读写、文件列表、项目内搜索。
use ignore::WalkBuilder;
use serde::Serialize;
use std::fs;
use std::path::Path;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LIST: usize = 30_000;

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[tauri::command]
pub fn read_dir(path: String) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = fs::read_dir(&path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" || name == ".DS_Store" {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some(Entry { name, path: e.path().to_string_lossy().to_string(), is_dir })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

#[tauri::command]
pub fn read_file(path: String) -> Result<String, String> {
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err("文件过大（超过 8MB）".into());
    }
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| "不是 UTF-8 文本文件".into())
}

#[tauri::command]
pub fn write_file(path: String, content: String) -> Result<(), String> {
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, content).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_entry(path: String, is_dir: bool) -> Result<(), String> {
    if Path::new(&path).exists() {
        return Err("已存在同名文件".into());
    }
    if is_dir {
        fs::create_dir_all(&path).map_err(|e| e.to_string())
    } else {
        write_file(path, String::new())
    }
}

#[tauri::command]
pub fn rename_entry(from: String, to: String) -> Result<(), String> {
    if Path::new(&to).exists() {
        return Err("目标已存在".into());
    }
    fs::rename(from, to).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_entry(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) }.map_err(|e| e.to_string())
}

/// 列出项目内所有文件（相对路径），遵守 .gitignore。
#[tauri::command]
pub fn list_files(root: String) -> Vec<String> {
    let mut out = Vec::new();
    for e in WalkBuilder::new(&root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
        if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if let Ok(rel) = e.path().strip_prefix(&root) {
                out.push(rel.to_string_lossy().to_string());
                if out.len() >= MAX_LIST {
                    break;
                }
            }
        }
    }
    out
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// 在一段文本里找匹配行（纯函数，便于测试）。
pub fn match_lines(content: &str, query: &str, case_sensitive: bool) -> Vec<(usize, String)> {
    if query.is_empty() {
        return vec![];
    }
    let q = if case_sensitive { query.to_string() } else { query.to_lowercase() };
    content
        .lines()
        .enumerate()
        .filter(|(_, l)| if case_sensitive { l.contains(&q) } else { l.to_lowercase().contains(&q) })
        .map(|(i, l)| (i + 1, l.chars().take(240).collect()))
        .collect()
}

#[tauri::command]
pub fn search_text(root: String, query: String, case_sensitive: bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    for e in WalkBuilder::new(&root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if e.metadata().map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(true) {
            continue;
        }
        let Ok(content) = fs::read_to_string(e.path()) else { continue };
        let rel = e.path().strip_prefix(&root).unwrap_or(e.path()).to_string_lossy().to_string();
        for (line, text) in match_lines(&content, &query, case_sensitive) {
            hits.push(Hit { path: rel.clone(), line, text });
            if hits.len() >= 800 {
                return hits;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 忽略大小写匹配() {
        let r = match_lines("Hello\nworld\nHELLO again", "hello", false);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn 区分大小写与空查询() {
        assert_eq!(match_lines("Hello\nhello", "hello", true).len(), 1);
        assert!(match_lines("abc", "", false).is_empty());
    }

    #[test]
    fn 读写与目录排序() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_string_lossy().to_string();
        write_file(format!("{root}/b.txt"), "x".into()).unwrap();
        create_entry(format!("{root}/z_dir"), true).unwrap();
        let list = read_dir(root.clone()).unwrap();
        assert!(list[0].is_dir, "目录应排在文件前");
        assert_eq!(read_file(format!("{root}/b.txt")).unwrap(), "x");
        assert!(create_entry(format!("{root}/b.txt"), false).is_err());
    }
}

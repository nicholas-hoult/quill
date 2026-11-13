//! 项目符号索引（#6）：给 `@文件#符号` 提供候选和源码行。在后台线程里建，按 (mtime, 大小) 缓存，
//! 保存一次文件不会把整个项目重新解析一遍。
//! 错了表现为：@ 菜单里没有刚写的函数、打开项目卡顿（在界面线程里解析）、大项目内存暴涨。

use crate::mentions::SymRef;
use crate::outline;
use crate::syntax;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

/// 超过这个大小的文件不建索引（多半是生成文件 / 压缩过的）
pub const MAX_FILE: u64 = 200 * 1024;
/// 索引总条数上限
pub const MAX_TOTAL: usize = 6000;

pub struct Entry { mtime: SystemTime, len: u64, syms: Vec<SymRef> }
pub type Cache = HashMap<String, Entry>;

/// 只留顶层符号和一层嵌套（类里的方法、impl 里的函数），更深的没有引用价值
fn convert(path: &str, syms: Vec<outline::Symbol>) -> Vec<SymRef> {
    syms.into_iter().filter(|s| s.depth <= 1).map(|s| SymRef { path: path.to_string(), name: s.name, kind: s.kind, line: s.line, end_line: s.end_line }).collect()
}

/// `files` 是相对项目根的路径。命中缓存（mtime 和大小都没变）的文件不重新解析；缓存里已经不在 `files` 里的条目会被清掉。
pub fn build(root: &Path, files: &[String], cache: &mut Cache) -> Vec<SymRef> {
    let mut out: Vec<SymRef> = vec![];
    let present: std::collections::HashSet<&str> = files.iter().map(String::as_str).collect();
    cache.retain(|k, _| present.contains(k.as_str()));
    for f in files {
        if out.len() >= MAX_TOTAL { break; }
        let Some(lang) = syntax::lang_for_path(f) else { continue };
        let Ok(meta) = std::fs::metadata(root.join(f)) else { continue };
        if meta.len() > MAX_FILE { continue; }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let hit = cache.get(f).filter(|e| e.mtime == mtime && e.len == meta.len());
        let syms = match hit {
            Some(e) => e.syms.clone(),
            None => {
                let Ok(text) = std::fs::read_to_string(root.join(f)) else { continue };
                let syms = convert(f, outline::symbols(lang, &text));
                cache.insert(f.clone(), Entry { mtime, len: meta.len(), syms: syms.clone() });
                syms
            }
        };
        let room = MAX_TOTAL - out.len();
        out.extend(syms.into_iter().take(room));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("quill-symindex-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        d
    }

    #[test]
    fn 建索引_顶层和一层嵌套() {
        let d = tmp("basic");
        std::fs::write(d.join("src/a.rs"), "fn top() {}\nstruct S;\nimpl S {\n    fn method(&self) {\n        fn inner() {}\n    }\n}\n").unwrap();
        std::fs::write(d.join("notes.txt"), "不是代码").unwrap();
        let mut cache = Cache::new();
        let s = build(&d, &["src/a.rs".into(), "notes.txt".into(), "missing.rs".into()], &mut cache);
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"top") && names.contains(&"S") && names.contains(&"method"), "{names:?}");
        assert!(!names.contains(&"inner"), "嵌套两层以上的不要：{names:?}");
        let top = s.iter().find(|x| x.name == "top").unwrap();
        assert_eq!((top.path.as_str(), top.line, top.end_line), ("src/a.rs", 0, 0));
        assert!(s.iter().all(|x| x.path == "src/a.rs"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn 文件改了只重新解析改过的() {
        let d = tmp("cache");
        std::fs::write(d.join("src/a.rs"), "fn one() {}\n").unwrap();
        std::fs::write(d.join("src/b.rs"), "fn bee() {}\n").unwrap();
        let files: Vec<String> = vec!["src/a.rs".into(), "src/b.rs".into()];
        let mut cache = Cache::new();
        let s1 = build(&d, &files, &mut cache);
        assert_eq!(s1.len(), 2);
        // 篡改缓存里 b 的内容，证明未改动的文件确实走缓存（不会重新解析）
        cache.get_mut("src/b.rs").unwrap().syms[0].name = "来自缓存".into();
        // 改 a（内容长度变了，mtime 也变）
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(d.join("src/a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
        let s2 = build(&d, &files, &mut cache);
        let names: Vec<&str> = s2.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"two"), "改过的文件要更新：{names:?}");
        assert!(names.contains(&"来自缓存"), "没改的文件走缓存：{names:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn 文件被删后缓存清掉() {
        let d = tmp("evict");
        std::fs::write(d.join("src/a.rs"), "fn one() {}\n").unwrap();
        let mut cache = Cache::new();
        build(&d, &["src/a.rs".into()], &mut cache);
        assert_eq!(cache.len(), 1);
        let s = build(&d, &[], &mut cache);
        assert!(s.is_empty() && cache.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn 太大的文件和总数上限() {
        let d = tmp("limits");
        let big = format!("fn x() {{}}\n{}", "// pad\n".repeat((MAX_FILE as usize / 7) + 10));
        std::fs::write(d.join("src/big.rs"), big).unwrap();
        let mut cache = Cache::new();
        assert!(build(&d, &["src/big.rs".into()], &mut cache).is_empty(), "超过 200KB 不建索引");
        // 总数上限：一个文件里放很多函数
        let many: String = (0..(MAX_TOTAL + 500)).map(|i| format!("fn f{i}() {{}}\n")).collect();
        assert!((many.len() as u64) < MAX_FILE * 4);
        if (many.len() as u64) <= MAX_FILE {
            std::fs::write(d.join("src/many.rs"), many).unwrap();
            let s = build(&d, &["src/many.rs".into()], &mut cache);
            assert!(s.len() <= MAX_TOTAL);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}

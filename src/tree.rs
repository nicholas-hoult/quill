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
    // 绝大多数文件没有命中：整文件小写一次做快速排除，省掉逐行分配小写字符串（整库搜索时是大头）
    if !content.to_lowercase().contains(&q) { return vec![]; }
    content.lines().enumerate().filter(|(_, l)| l.to_lowercase().contains(&q)).map(|(i, l)| (i + 1, l.chars().take(240).collect())).collect()
}

/// 能搜索的文件（遵守 .gitignore；跳过 .git、超过 2MB 的文件），顺序即遍历顺序。惰性：不需要的部分不会被遍历
fn searchable_files(root: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten().filter_map(|e| {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) { return None; }
        if e.metadata().map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(true) { return None; }
        Some(e.into_path())
    })
}

/// 多线程 map：结果顺序与输入一致。线程从共享计数器里领下一项，所以耗时不均也不会让某个线程闲着
fn par_map<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = threads.min(items.len()).max(1);
    if threads == 1 { return items.iter().map(&f).collect(); }
    let next = AtomicUsize::new(0);
    let mut parts: Vec<Vec<(usize, R)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads).map(|_| sc.spawn(|| {
            let mut mine = vec![];
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= items.len() { break; }
                mine.push((i, f(&items[i])));
            }
            mine
        })).collect();
        handles.into_iter().map(|h| h.join().expect("搜索线程不应 panic")).collect()
    });
    let mut all: Vec<(usize, R)> = parts.drain(..).flatten().collect();
    all.sort_unstable_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

/// 项目内文本搜索（不区分大小写，最多 `limit` 条）。
/// 文件分批多线程读取（读文件和 UTF-8 校验是大头），批内结果按遍历顺序合并，所以结果顺序确定，
/// 等于顺序扫描的前 `limit` 条；凑够 `limit` 条后不再处理后面的批
pub fn search_text(root: &Path, query: &str, limit: usize) -> Vec<Hit> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let mut files = searchable_files(root);
    let mut hits = vec![];
    loop {
        // 边遍历边处理：常见词凑够 limit 条后，目录剩下的部分根本不会被遍历
        let batch: Vec<std::path::PathBuf> = files.by_ref().take(threads * 32).collect();
        if batch.is_empty() { break; }
        let found = par_map(&batch, threads, |p| std::fs::read_to_string(p).map(|c| match_lines(&c, query)).unwrap_or_default());
        for (p, lines) in batch.iter().zip(found) {
            if lines.is_empty() { continue; }
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy().to_string();
            for (line, text) in lines {
                hits.push(Hit { path: rel.clone(), line, text });
                if hits.len() >= limit { return hits; }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 行搜索_不区分大小写_行号从1开始() {
        let r = match_lines("Hello\nworld\nHELLO again", "hello");
        assert_eq!(r, vec![(1, "Hello".to_string()), (3, "HELLO again".to_string())]);
    }

    #[test]
    fn 行搜索_非ascii大小写与中文() {
        // 优化成「整文件先小写做快速排除」后，Unicode 的大小写折叠结果必须和逐行一致
        assert_eq!(match_lines("Straße\nÄpfel\n你好 World", "äPFEL"), vec![(2, "Äpfel".to_string())]);
        assert_eq!(match_lines("a\n你好 World", "你好"), vec![(2, "你好 World".to_string())]);
    }

    #[test]
    fn 行搜索_没有命中和空查询() {
        assert!(match_lines("abc\ndef", "zzz").is_empty());
        assert!(match_lines("abc", "").is_empty());
    }

    #[test]
    fn 行搜索_长行只取前240个字符_不会在多字节字符中间截断() {
        let line = "好".repeat(300);
        let r = match_lines(&line, "好");
        assert_eq!(r[0].1.chars().count(), 240);
    }

    #[test]
    fn 行搜索_跨行的词不会误命中() {
        // 查询里没有换行，不能因为整文件快速排除而把相邻两行拼起来命中
        assert!(match_lines("ab\ncd", "b\nc").is_empty());
    }

    #[test]
    fn 并行映射_结果顺序与输入一致() {
        let items: Vec<usize> = (0..1000).collect();
        // 故意让不同项耗时不同，顺序仍必须按输入排
        let r = par_map(&items, 8, |&i| { if i % 7 == 0 { std::thread::sleep(std::time::Duration::from_micros(200)); } i * 2 });
        assert_eq!(r, items.iter().map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(par_map::<usize, usize>(&[], 4, |&i| i), Vec::<usize>::new());
        assert_eq!(par_map(&[5usize], 8, |&i| i + 1), vec![6]); // 线程数多于项数
    }

    #[test]
    fn 搜索_命中顺序确定且等于顺序扫描的前N条() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..300 { std::fs::write(d.path().join(format!("f{i:03}.txt")), format!("x\nhit {i}\nhit again {i}\n")).unwrap(); }
        // 参照：按同一份文件清单顺序、单线程逐个文件匹配
        let mut want = vec![];
        for f in searchable_files(d.path()) {
            let rel = f.strip_prefix(d.path()).unwrap().to_string_lossy().to_string();
            for (line, text) in match_lines(&std::fs::read_to_string(&f).unwrap(), "hit") { want.push(Hit { path: rel.clone(), line, text }); }
        }
        want.truncate(100);
        let a = search_text(d.path(), "hit", 100);
        let b = search_text(d.path(), "hit", 100);
        assert_eq!(a.len(), 100);
        assert_eq!(a, want, "并行扫描的结果必须和顺序扫描的前 100 条完全一致（含顺序）");
        assert_eq!(a, b, "两次结果必须一致");
    }

    #[test]
    fn 搜索_跳过大文件和非utf8() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("big.txt"), "needle ".repeat(400_000)).unwrap(); // > 2MB
        std::fs::write(d.path().join("bin.dat"), [0xffu8, 0xfe, b'n', b'e', b'e', b'd', b'l', b'e']).unwrap();
        std::fs::write(d.path().join("ok.txt"), "a needle b").unwrap();
        let hits = search_text(d.path(), "needle", 10);
        assert_eq!(hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), vec!["ok.txt"]);
    }

    /// 度量（不进常规测试）：`SEARCH_BENCH_DIR=<目录> cargo test --release search_bench -- --ignored --nocapture`
    /// 搜一个不存在的词 = 最坏情况（要读完所有文件）
    #[test]
    #[ignore]
    fn search_bench() {
        let Ok(dir) = std::env::var("SEARCH_BENCH_DIR") else { return };
        let root = Path::new(&dir);
        for round in 0..3 {
            let t = std::time::Instant::now();
            let hits = search_text(root, "zzqq_不存在的词_xx", 600);
            eprintln!("[bench] 第{}轮 无命中全库扫描 {:.0}ms（命中 {}）", round + 1, t.elapsed().as_secs_f64() * 1000., hits.len());
        }
        // 拆开看：纯遍历 / 遍历+读文件
        let t = std::time::Instant::now();
        let (mut n, mut bytes) = (0usize, 0u64);
        for e in ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) { n += 1; bytes += e.metadata().map(|m| m.len()).unwrap_or(0); }
        }
        eprintln!("[bench] 纯遍历 {:.0}ms：{} 个文件 {:.0}MB", t.elapsed().as_secs_f64() * 1000., n, bytes as f64 / 1e6);
        let t = std::time::Instant::now();
        let mut read = 0u64;
        for e in ignore::WalkBuilder::new(root).hidden(false).filter_entry(|e| e.file_name() != ".git").build().flatten() {
            if !e.file_type().map(|t| t.is_file()).unwrap_or(false) { continue; }
            if e.metadata().map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(true) { continue; }
            if let Ok(c) = std::fs::read_to_string(e.path()) { read += c.len() as u64; }
        }
        eprintln!("[bench] 遍历+读文件 {:.0}ms：读入 {:.0}MB 文本", t.elapsed().as_secs_f64() * 1000., read as f64 / 1e6);
        let t = std::time::Instant::now();
        let hits = search_text(root, "fn ", 600);
        eprintln!("[bench] 常见词（达到上限提前结束）{:.0}ms（命中 {}）", t.elapsed().as_secs_f64() * 1000., hits.len());
    }

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

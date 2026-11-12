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
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] } },
        { "name": "run_command", "description": "在项目根目录运行一条 shell 命令（例如构建、跑测试），返回合并后的输出和退出码。每次运行都要用户确认，超时 60 秒会被终止。只在确实需要时使用，优先用 read_file / search / diagnostics。",
          "input_schema": { "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] } },
        { "name": "diagnostics", "description": "查看语言服务器给出的诊断（错误 / 警告）。path 可选，为相对项目根目录的路径；不给则返回所有已知诊断。修改代码后用它检查有没有引入新问题。",
          "input_schema": { "type": "object", "properties": { "path": { "type": "string" } } } }
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

/// 「应用代码块 / edit_file」的目标路径：模型常写 `/src/a.rs` 或 `./src/a.rs`，宽容地当成相对项目根；
/// 但不允许 `..` 逃出项目根（否则会在项目外建目录、写文件）。
pub fn apply_target(root: &Path, model_path: &str) -> Result<PathBuf, String> {
    let rel = model_path.trim().trim_start_matches("./").trim_start_matches('/');
    if rel.is_empty() { return Err("路径为空".into()); }
    safe_path(root, rel)
}

const MAX_READ: usize = 60_000;

/// 有副作用、必须先经用户确认的工具。edit_file 不在此列：它本身就要走逐块审阅。
pub fn needs_permission(name: &str) -> bool { name == "run_command" }

/// 命令输出最多保留的字节数（头 + 尾）
pub const MAX_OUTPUT: usize = 20_000;

/// 输出过长时保留开头和结尾（报错通常在最后），中间注明省略了多少字节。
pub fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let half = max / 2;
    let head_end = (0..=half).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    let tail_start = (s.len() - half..=s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
    format!("{}\n…（中间省略 {} 字节）…\n{}", &s[..head_end], tail_start - head_end, &s[tail_start..])
}

/// 在项目根用 `sh -c` 运行命令：stdin 关闭，stdout 与 stderr 合并。超时或被取消会连同子进程一起杀掉。
/// 返回「输出 + 末行状态」（`[退出码 N]` / `[超时…]` / `[已取消]`）；只有拉不起进程才返回 Err。
pub fn exec(root: &Path, command: &str, timeout: std::time::Duration, cancel: &std::sync::atomic::AtomicBool) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    // exec 2>&1：让 stderr 和 stdout 走同一根管道，输出顺序和终端里一致
    let mut child = Command::new("sh")
        .arg("-c").arg(format!("exec 2>&1\n{command}"))
        .current_dir(root)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
        .process_group(0) // 自成一组，超时时连孙进程（cargo、node…）一起杀
        .spawn().map_err(|e| format!("无法启动命令：{e}"))?;
    let mut pipe = child.stdout.take().ok_or("无法读取命令输出")?;
    // 单独的线程读管道：不读的话输出多了子进程会写满管道卡死
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || { let mut buf = vec![]; let _ = pipe.read_to_end(&mut buf); let _ = tx.send(buf); });

    let start = Instant::now();
    let mut note: Option<String> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(e) => return Err(format!("等待命令失败：{e}")),
        }
        if cancel.load(Ordering::Relaxed) { note = Some("[已取消，进程已终止]".into()); break None; }
        if start.elapsed() >= timeout { note = Some(format!("[超时 {} 秒，进程已终止]", timeout.as_secs_f32().round() as u64)); break None; }
        std::thread::sleep(Duration::from_millis(15));
    };
    if status.is_none() {
        // 负的进程号 = 整个进程组
        let _ = Command::new("kill").args(["-9", &format!("-{}", child.id())]).status();
        let _ = child.kill();
        let _ = child.wait();
    }
    // 逃出进程组的后台进程可能一直占着管道，不无限等
    let out = rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    let body = truncate_output(text.trim_end(), MAX_OUTPUT);
    let tail = note.unwrap_or_else(|| match status.and_then(|s| s.code()) { Some(c) => format!("[退出码 {c}]"), None => "[被信号终止]".into() });
    Ok(if body.is_empty() { tail } else { format!("{body}\n{tail}") })
}

/// 把语言服务器诊断整理成「路径:行:列 级别 消息」。`files` 的路径是绝对路径；`filter` 为相对路径时只看那个文件。
pub fn format_diagnostics(root: &Path, files: &[(String, Vec<crate::lsp::RawDiag>)], filter: Option<&str>) -> String {
    const MAX_ITEMS: usize = 100;
    let mut rows: Vec<(String, usize, usize, u8, String)> = vec![];
    for (path, diags) in files {
        let rel = Path::new(path).strip_prefix(root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.clone());
        if let Some(f) = filter { if rel != f.trim().trim_start_matches("./") { continue; } }
        for d in diags { rows.push((rel.clone(), d.start.0 + 1, d.start.1 + 1, d.severity, d.message.lines().next().unwrap_or("").to_string())); }
    }
    if rows.is_empty() { return "没有诊断（没有问题，或这类文件的语言服务器没有运行）".into(); }
    rows.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));
    let total = rows.len();
    let mut out: Vec<String> = rows.into_iter().take(MAX_ITEMS).map(|(p, l, c, sev, m)| {
        let level = match sev { 1 => "错误", 2 => "警告", 3 => "信息", _ => "提示" };
        format!("{p}:{l}:{c} {level} {m}")
    }).collect();
    if total > MAX_ITEMS { out.push(format!("…还有 {} 条未列出", total - MAX_ITEMS)); }
    out.join("\n")
}

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

/// 给界面看的摘要，拆成「动作」和「对象」两段（对象用等宽字体显示），例如 ("读取", "src/a.rs")
pub fn parts(name: &str, input: &Value) -> (String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let (verb, obj) = match name {
        "read_file" => ("读取", s("path")),
        "list_dir" => ("列出目录", s("path")),
        "search" => ("搜索", format!("「{}」", s("query"))),
        "edit_file" => ("修改", s("path")),
        "run_command" => ("运行", s("command")),
        "diagnostics" => ("查看诊断", if s("path").is_empty() { "全部".to_string() } else { s("path") }),
        other => return (other.to_string(), String::new()),
    };
    (verb.to_string(), obj)
}

/// 一句话摘要，例如「读取 src/a.rs」
pub fn summary(name: &str, input: &Value) -> String {
    let (v, o) = parts(name, input);
    if o.is_empty() { v } else if o.starts_with('「') { format!("{v}{o}") } else { format!("{v} {o}") }
}

/// 从 run_command 的结果里取出末尾的状态行：`[退出码 0]` → ("退出码 0", true)；超时 / 取消 / 非零退出 → false。
/// 返回 (标签, 是否成功, 去掉状态行后的正文)
pub fn result_status(result: &str) -> Option<(String, bool, &str)> {
    let (body, last) = match result.rfind('\n') { Some(i) => (&result[..i], &result[i + 1..]), None => ("", result) };
    let inner = last.trim().strip_prefix('[')?.strip_suffix(']')?;
    let ok = inner == "退出码 0";
    let label = inner.split('，').next().unwrap_or(inner).to_string();
    Some((label, ok, body))
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
        assert_eq!(parts("run_command", &json!({"command":"cargo test"})), ("运行".to_string(), "cargo test".to_string()));
        assert_eq!(summary("diagnostics", &json!({})), "查看诊断 全部");
        assert_eq!(summary("diagnostics", &json!({"path":"src/a.rs"})), "查看诊断 src/a.rs");
    }

    // ---- #3：终端 / 诊断 ----
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn run_cmd(root: &Path, cmd: &str, secs: f32) -> String {
        exec(root, cmd, Duration::from_secs_f32(secs), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn 只有_run_command_需要确认() {
        assert!(needs_permission("run_command"));
        for n in ["read_file", "list_dir", "search", "edit_file", "diagnostics"] { assert!(!needs_permission(n), "{n}"); }
    }

    #[test]
    fn 命令输出带退出码_stderr_合并() {
        let d = tempfile::tempdir().unwrap();
        let ok = run_cmd(d.path(), "echo hi", 5.0);
        assert!(ok.contains("hi") && ok.trim_end().ends_with("[退出码 0]"), "{ok}");
        let bad = run_cmd(d.path(), "echo oops 1>&2; exit 3", 5.0);
        assert!(bad.contains("oops") && bad.trim_end().ends_with("[退出码 3]"), "{bad}");
    }

    #[test]
    fn 命令在项目根运行且标准输入已关闭() {
        let d = tempfile::tempdir().unwrap();
        let pwd = run_cmd(d.path(), "pwd", 5.0);
        let want = std::fs::canonicalize(d.path()).unwrap();
        assert!(pwd.lines().next().unwrap().ends_with(want.to_str().unwrap()), "{pwd}");
        // cat 读 stdin：没关闭的话会一直卡住直到超时
        let t = Instant::now();
        let r = run_cmd(d.path(), "cat", 5.0);
        assert!(t.elapsed() < Duration::from_secs(3) && r.contains("[退出码 0]"), "{r}");
    }

    #[test]
    fn 超时会杀掉整个进程组() {
        let d = tempfile::tempdir().unwrap();
        let t = Instant::now();
        let r = run_cmd(d.path(), "sleep 30 & sleep 30", 0.3);
        assert!(t.elapsed() < Duration::from_secs(5), "没有及时结束：{:?}", t.elapsed());
        assert!(r.contains("超时"), "{r}");
    }

    #[test]
    fn 取消标志会中断命令() {
        let d = tempfile::tempdir().unwrap();
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let f2 = flag.clone();
        std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(150)); f2.store(true, Ordering::Relaxed); });
        let t = Instant::now();
        let r = exec(d.path(), "sleep 30", Duration::from_secs(30), &flag).unwrap();
        assert!(t.elapsed() < Duration::from_secs(5) && r.contains("已取消"), "{r}");
    }

    #[test]
    fn 输出截断保留头尾且不切坏汉字() {
        assert_eq!(truncate_output("短", 100), "短");
        let s = format!("{}{}{}", "头".repeat(5000), "中".repeat(5000), "尾".repeat(5000));
        let r = truncate_output(&s, 3000);
        assert!(r.starts_with("头") && r.trim_end().ends_with("尾") && r.contains("省略"), "{}", &r[r.len().saturating_sub(80)..]);
        assert!(r.len() < 3300);
    }

    fn rd(line: usize, col: usize, sev: u8, msg: &str) -> crate::lsp::RawDiag {
        crate::lsp::RawDiag { start: (line, col), end: (line, col + 1), severity: sev, message: msg.into(), source: String::new() }
    }

    #[test]
    fn 诊断格式化_相对路径_一基行列_按路径和行排序() {
        let root = Path::new("/p");
        let files = vec![
            ("/p/src/b.rs".to_string(), vec![rd(9, 3, 1, "类型不匹配")]),
            ("/p/src/a.rs".to_string(), vec![rd(5, 0, 2, "未使用的变量"), rd(1, 4, 1, "找不到 x")]),
        ];
        let out = format_diagnostics(root, &files, None);
        assert_eq!(out, "src/a.rs:2:5 错误 找不到 x\nsrc/a.rs:6:1 警告 未使用的变量\nsrc/b.rs:10:4 错误 类型不匹配");
        let one = format_diagnostics(root, &files, Some("src/b.rs"));
        assert_eq!(one, "src/b.rs:10:4 错误 类型不匹配");
    }

    #[test]
    fn 诊断为空和超量() {
        let root = Path::new("/p");
        assert!(format_diagnostics(root, &[], None).contains("没有诊断"));
        let many: Vec<_> = (0..130).map(|i| rd(i, 0, 1, "x")).collect();
        let out = format_diagnostics(root, &[("/p/a.rs".into(), many)], None);
        assert_eq!(out.lines().count(), 101);
        assert!(out.lines().last().unwrap().contains("还有 30 条"));
    }

    #[test]
    fn 解析命令结果的状态行() {
        assert_eq!(result_status("a\nb\n[退出码 0]"), Some(("退出码 0".into(), true, "a\nb")));
        assert_eq!(result_status("boom\n[退出码 101]"), Some(("退出码 101".into(), false, "boom")));
        assert_eq!(result_status("[退出码 0]"), Some(("退出码 0".into(), true, "")));
        let t = result_status("x\n[超时 60 秒，进程已终止]").unwrap();
        assert!(!t.1 && t.0.starts_with("超时"));
        assert_eq!(result_status("错误：没有打开项目"), None);
        assert_eq!(result_status("普通输出\n没有状态行"), None);
    }

    #[test]
    fn 应用目标路径_宽容开头的斜杠_但不能逃出项目根() {
        let root = Path::new("/p");
        assert_eq!(apply_target(root, "src/a.rs").unwrap(), PathBuf::from("/p/src/a.rs"));
        assert_eq!(apply_target(root, "./src/a.rs").unwrap(), PathBuf::from("/p/src/a.rs"));
        assert_eq!(apply_target(root, "/src/a.rs").unwrap(), PathBuf::from("/p/src/a.rs"));
        assert_eq!(apply_target(root, "  notes/new.txt ").unwrap(), PathBuf::from("/p/notes/new.txt"));
        assert!(apply_target(root, "../x").is_err());
        assert!(apply_target(root, "../../etc/hosts").is_err());
        assert!(apply_target(root, "/../x").is_err());
        assert!(apply_target(root, "src/../../x").is_err());
        assert!(apply_target(root, "").is_err());
        assert!(apply_target(root, "  ./ ").is_err());
    }
}

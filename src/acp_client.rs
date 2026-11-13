//! ACP 子进程管理（#7）：把本机 Claude Code（经 ACP 适配器）当子进程跑起来，stdin/stdout 逐行 JSON-RPC。
//! 只负责「起进程 / 写一行 / 读一行 / 进程退出」，协议语义在 `acp.rs`。
//! 错了表现为：发消息没反应（进程没起来或 PATH 不对）、关掉 Quill 后留下孤儿进程、进程崩了界面一直转圈。

use crate::acp::{self, Incoming};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// 适配器的包名和固定版本（对标 Zed，agent_servers 里固定的也是 0.94.0）：适配器还在变（v2 草案），不跟 latest 走
pub const ADAPTER_PKG: &str = "@agentclientprotocol/claude-agent-acp";
pub const ADAPTER_VERSION: &str = "0.94.0";

/// 默认启动命令：适配器基于 Claude Agent SDK，沿用本机 Claude Code 的登录。
/// **固定版本**。这是兜底：npx 第一次会下载并缓存；缓存好之后 `default_command` 直接跑缓存里的入口，不再经过 npx
pub const DEFAULT_COMMAND: &str = "npx -y @agentclientprotocol/claude-agent-acp@0.94.0";

pub enum Event {
    Line(Incoming),
    /// 进程结束了。附带 stderr 末尾，用来告诉用户为什么
    Exited(String),
    /// Agent 的某个终端有新输出 / 退出了（`acp_term`）
    TermOutput,
    TermExit(String),
}

pub struct Handle {
    tx: std::sync::mpsc::Sender<String>,
    child: Arc<Mutex<Child>>,
}

impl Handle {
    /// 写一行（自动补换行）。进程已退出时静默丢弃，退出事件会另外送达
    pub fn send(&self, line: String) { let _ = self.tx.send(line); }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            kill_group(c.id());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// 整组杀掉（适配器会再起 claude、node 等子进程）
fn kill_group(pid: u32) {
    let _ = Command::new("kill").args(["-KILL", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// npx 的缓存目录（`~/.npm/_npx`）。npm 把 `npx -y 包@版本` 装在 `<hash>/node_modules/` 下
fn npx_cache_root() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| Path::new(&h).join(".npm/_npx"))
}

/// 在 npx 缓存里找「版本正好是 `ADAPTER_VERSION` 且入口文件在」的适配器，返回入口路径。
/// 为什么：`npx` 每次启动都要先跑一遍 npm（解析、校验缓存），实测握手 11.7s，直接运行入口 3.4s（同负载下）。
/// 版本不同的不能顶替——固定版本就是为了协议行为不变
pub fn cached_adapter(root: &Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let nm = entry.path().join("node_modules");
        let Ok(pkg) = std::fs::read_to_string(nm.join(ADAPTER_PKG).join("package.json")) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&pkg) else { continue };
        if v["version"].as_str() != Some(ADAPTER_VERSION) { continue; }
        let bin = nm.join(".bin/claude-agent-acp");
        if bin.is_file() { return Some(bin); }
    }
    None
}

/// 单引号包起来，交给 shell 时原样是一个参数（路径里可能有空格 / 单引号）
fn shell_quote(s: &str) -> String { format!("'{}'", s.replace('\'', r"'\''")) }

/// 没有自定义命令时用的：缓存里有固定版本的适配器就直接跑，否则回退 npx（它会下载并缓存，下次就走直接路径）
pub fn default_command() -> String {
    match npx_cache_root().and_then(|r| cached_adapter(&r)) {
        Some(bin) => shell_quote(&bin.to_string_lossy()),
        None => DEFAULT_COMMAND.to_string(),
    }
}

/// 实际要运行的命令：`QUILL_ACP` 环境变量（测试用）> 设置里填的 > 默认
pub fn command_line(configured: &str) -> String {
    if let Ok(c) = std::env::var("QUILL_ACP") { if !c.trim().is_empty() { return c; } }
    let c = configured.trim();
    if c.is_empty() { default_command() } else { c.to_string() }
}

/// 用用户的登录 shell 运行命令：从 Finder 打开的应用 PATH 很短，npx / claude 往往不在里面，
/// 登录 shell（`-l`）加载用户的 profile 才找得到。`exec` 让 shell 换成目标进程，杀进程时不留中间层。
fn shell_command(command: &str) -> Command {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".to_string());
    let mut c = Command::new(shell);
    c.arg("-l").arg("-c").arg(format!("exec {command}"));
    c
}

/// 开发日志用的绝对时间（毫秒）：`QUILL_ACP_LOG` 里每行都带，才能把「按下发送」和「收到第一个字」对上
fn epoch_ms() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) }

/// 往 `QUILL_ACP_LOG` 追加一行标记（只在设了该变量时；开发测量用，比如「用户按下了发送」）
pub fn log_mark(text: &str) {
    if let Some(mut f) = std::env::var("QUILL_ACP_LOG").ok().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok()) {
        let _ = writeln!(f, "{} ## {text}", epoch_ms());
    }
}

/// 起进程。成功返回句柄；读到的每一行（解析后）和进程退出都通过 `events` 送出。
pub fn spawn(command: &str, cwd: &Path, events: async_channel::Sender<Event>) -> Result<Handle, String> {
    let mut cmd = shell_command(command);
    let mut child = cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .process_group(0)
        .spawn().map_err(|e| format!("无法启动 Agent：{e}"))?;
    let (stdin, stdout, stderr) = (child.stdin.take().unwrap(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let child = Arc::new(Mutex::new(child));

    // 写线程：mpsc → stdin。Handle 释放后 Sender 关闭，循环结束，stdin 随之关闭（Agent 看到 EOF 会自行退出）
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut stdin = stdin;
        let log = std::env::var("QUILL_ACP_LOG").ok().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
        while let Ok(line) = rx.recv() {
            if let Some(mut f) = log.as_ref() { let _ = writeln!(f, "{} → {line}", epoch_ms()); }
            if stdin.write_all(line.as_bytes()).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush()).is_err() { break; }
        }
    });

    // stderr 只留末尾一小段，进程出错退出时拿来解释
    let tail = Arc::new(Mutex::new(String::new()));
    let t2 = tail.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut t) = t2.lock() {
                t.push_str(&line);
                t.push('\n');
                if t.len() > 4000 {
                    let from = t.len() - 3000;
                    let from = (from..=t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                    let kept = t[from..].to_string();
                    *t = kept;
                }
            }
        }
    });

    // 读线程：逐行解析；EOF = 进程结束
    let c2 = child.clone();
    std::thread::spawn(move || {
        // 调试：QUILL_ACP_LOG=文件 时把双方的原始 JSON 行记下来（→ 是我们发的，← 是 Agent 发的）
        let log = std::env::var("QUILL_ACP_LOG").ok().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
        let log = log.map(|f| Arc::new(Mutex::new(f)));
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(f) = &log { if let Ok(mut f) = f.lock() { let _ = writeln!(f, "{} ← {line}", epoch_ms()); } }
            let ev = acp::parse_line(&line);
            if ev == Incoming::Ignore { continue; }
            if events.send_blocking(Event::Line(ev)).is_err() { return; }
        }
        let status = c2.lock().ok().and_then(|mut c| c.wait().ok());
        // 给 stderr 线程一点时间把最后几行读完
        std::thread::sleep(std::time::Duration::from_millis(80));
        let mut msg = tail.lock().map(|t| t.trim().to_string()).unwrap_or_default();
        if msg.is_empty() { msg = status.map(|s| format!("进程已退出（{s}）")).unwrap_or_else(|| "进程已退出".to_string()); }
        let _ = events.send_blocking(Event::Exited(msg));
    });

    Ok(Handle { tx, child })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认命令固定了适配器版本() {
        // 版本号必须写死：npx 不带版本会取 latest，适配器一升级协议行为就可能变
        let pkg = DEFAULT_COMMAND.split_whitespace().last().unwrap();
        let (name, ver) = pkg.rsplit_once('@').unwrap();
        assert_eq!(name, "@agentclientprotocol/claude-agent-acp");
        assert_eq!(pkg, format!("{ADAPTER_PKG}@{ADAPTER_VERSION}"), "兜底命令和常量要一致，否则缓存命中的版本和 npx 兜底下载的版本不同");
        assert!(ver.split('.').count() == 3 && ver.split('.').all(|p| p.parse::<u32>().is_ok()), "{ver}");
    }

    #[test]
    fn 命令优先级_环境变量_设置_默认() {
        // 环境变量是进程全局的，这里只测不依赖它的两条
        if std::env::var("QUILL_ACP").is_err() {
            assert_eq!(command_line(""), default_command());
            assert_eq!(command_line("   "), default_command());
            assert_eq!(command_line(" claude-agent-acp "), "claude-agent-acp");
        }
    }

    #[test]
    fn 起一个回显进程_读到行并在退出时通知() {
        let (tx, rx) = async_channel::unbounded();
        let dir = std::env::temp_dir();
        // 一个最小「Agent」：输出一行响应后退出，并向 stderr 写一句原因
        let cmd = r#"sh -c 'echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}"; echo boom >&2'"#;
        let h = spawn(cmd, &dir, tx).unwrap();
        let first = rx.recv_blocking().unwrap();
        assert!(matches!(first, Event::Line(Incoming::Response { id: 1, .. })));
        let second = rx.recv_blocking().unwrap();
        match second { Event::Exited(msg) => assert!(msg.contains("boom"), "{msg}"), _ => panic!("应该是退出事件") }
        drop(h);
    }

    #[test]
    fn 写入的行能被子进程读到() {
        let (tx, rx) = async_channel::unbounded();
        let dir = std::env::temp_dir();
        // cat 把收到的每一行原样写回
        let h = spawn("cat", &dir, tx).unwrap();
        h.send(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#.to_string());
        let ev = rx.recv_blocking().unwrap();
        assert!(matches!(ev, Event::Line(Incoming::Response { id: 7, .. })));
        drop(h); // 释放时整组杀掉
    }

    #[test]
    fn 启动不存在的命令也有退出事件() {
        let (tx, rx) = async_channel::unbounded();
        let h = spawn("definitely-not-a-command-xyz", &std::env::temp_dir(), tx).unwrap();
        match rx.recv_blocking().unwrap() { Event::Exited(msg) => assert!(!msg.is_empty()), _ => panic!() }
        drop(h);
    }

    // ---- 绕过 npx（#8）：npx 每次启动要多花几秒（实测握手 11.7s vs 直接运行 3.4s），缓存里已有固定版本就直接跑 ----

    fn fake_cache(root: &Path, hash: &str, version: &str, with_bin: bool) {
        let nm = root.join(hash).join("node_modules");
        std::fs::create_dir_all(nm.join("@agentclientprotocol/claude-agent-acp")).unwrap();
        std::fs::write(nm.join("@agentclientprotocol/claude-agent-acp/package.json"), format!(r#"{{"name":"@agentclientprotocol/claude-agent-acp","version":"{version}"}}"#)).unwrap();
        if with_bin { std::fs::create_dir_all(nm.join(".bin")).unwrap(); std::fs::write(nm.join(".bin/claude-agent-acp"), "#!/usr/bin/env node\n").unwrap(); }
    }

    fn tmp_root(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("quill-npx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn 缓存里有同版本适配器就返回入口() {
        let root = tmp_root("hit");
        fake_cache(&root, "aaa", ADAPTER_VERSION, true);
        let bin = cached_adapter(&root).unwrap();
        assert!(bin.ends_with("aaa/node_modules/.bin/claude-agent-acp"), "{bin:?}");
    }

    #[test]
    fn 版本不同_没有入口_缓存为空_都不返回() {
        let root = tmp_root("miss");
        assert!(cached_adapter(&root).is_none(), "缓存为空");
        assert!(cached_adapter(&root.join("不存在")).is_none(), "目录不存在");
        fake_cache(&root, "old", "0.1.0", true);
        assert!(cached_adapter(&root).is_none(), "版本不同：固定版本是为了协议行为不变，不能用别的版本顶替");
        fake_cache(&root, "nobin", ADAPTER_VERSION, false);
        assert!(cached_adapter(&root).is_none(), "有包但入口文件不在（缓存装了一半）");
    }

    #[test]
    fn 多个缓存目录里挑版本对的那个() {
        let root = tmp_root("multi");
        fake_cache(&root, "a-old", "0.2.0", true);
        fake_cache(&root, "b-ok", ADAPTER_VERSION, true);
        assert!(cached_adapter(&root).unwrap().to_string_lossy().contains("b-ok"));
    }

    #[test]
    fn 路径含空格和单引号时能被_shell_原样执行() {
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("/it's/x"), r#"'/it'\''s/x'"#);
        // 真的交给 sh 执行：引用后得到原样的一个参数
        let out = Command::new("sh").arg("-c").arg(format!("printf %s {}", shell_quote("/it's a/b"))).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/it's a/b");
    }

    #[test]
    fn 默认命令要么是缓存入口要么回退_npx() {
        let c = default_command();
        let shape_ok = c == DEFAULT_COMMAND || (c.starts_with('\'') && c.ends_with("claude-agent-acp'"));
        eprintln!("本机默认命令解析为：{c}");
        assert!(shape_ok, "{c}");
    }
}

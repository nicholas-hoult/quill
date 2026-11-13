//! ACP 子进程管理（#7）：把本机 Claude Code（经 ACP 适配器）当子进程跑起来，stdin/stdout 逐行 JSON-RPC。
//! 只负责「起进程 / 写一行 / 读一行 / 进程退出」，协议语义在 `acp.rs`。
//! 错了表现为：发消息没反应（进程没起来或 PATH 不对）、关掉 Quill 后留下孤儿进程、进程崩了界面一直转圈。

use crate::acp::{self, Incoming};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// 默认启动命令：适配器基于 Claude Agent SDK，沿用本机 Claude Code 的登录
pub const DEFAULT_COMMAND: &str = "npx -y @agentclientprotocol/claude-agent-acp";

pub enum Event {
    Line(Incoming),
    /// 进程结束了。附带 stderr 末尾，用来告诉用户为什么
    Exited(String),
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

/// 实际要运行的命令：`QUILL_ACP` 环境变量（测试用）> 设置里填的 > 默认
pub fn command_line(configured: &str) -> String {
    if let Ok(c) = std::env::var("QUILL_ACP") { if !c.trim().is_empty() { return c; } }
    let c = configured.trim();
    if c.is_empty() { DEFAULT_COMMAND.to_string() } else { c.to_string() }
}

/// 用用户的登录 shell 运行命令：从 Finder 打开的应用 PATH 很短，npx / claude 往往不在里面，
/// 登录 shell（`-l`）加载用户的 profile 才找得到。`exec` 让 shell 换成目标进程，杀进程时不留中间层。
fn shell_command(command: &str) -> Command {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".to_string());
    let mut c = Command::new(shell);
    c.arg("-l").arg("-c").arg(format!("exec {command}"));
    c
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
        while let Ok(line) = rx.recv() {
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
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
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
    fn 命令优先级_环境变量_设置_默认() {
        // 环境变量是进程全局的，这里只测不依赖它的两条
        if std::env::var("QUILL_ACP").is_err() {
            assert_eq!(command_line(""), DEFAULT_COMMAND);
            assert_eq!(command_line("   "), DEFAULT_COMMAND);
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
}

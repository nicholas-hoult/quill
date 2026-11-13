//! Agent 的终端（#7）：`terminal/create` 时 Quill 起进程，输出实时累积，卡片里能看到正在跑的命令（对标 Zed）。
//! 纯逻辑（输出缓冲、命令行拼装、退出状态）在 `acp.rs`，这里只管进程。
//! 错了表现为：命令输出乱码 / 丢最后几行、命令跑完了 Agent 还在等、停止后留下孤儿进程、一直打印的命令把界面刷卡。

use crate::acp::{Exit, OutputBuf, shell_invocation};
use crate::acp_client::Event;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Term {
    buf: Arc<Mutex<OutputBuf>>,
    exit: Arc<Mutex<Option<Exit>>>,
    pid: u32,
}

#[derive(Default)]
pub struct Terminals { map: HashMap<String, Term>, next: u64 }

/// 通知界面重绘的最小间隔：一直刷输出的命令（日志、进度条）不能每个读取块都触发重绘
const NOTIFY_EVERY: Duration = Duration::from_millis(60);

fn kill_group(pid: u32) {
    let _ = Command::new("kill").args(["-KILL", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

impl Terminals {
    /// 起一个命令。cwd 由调用方决定（默认项目根）。返回终端 ID。
    pub fn create(&mut self, command: &str, args: &[String], env: &[(String, String)], cwd: &Path, limit: Option<usize>, events: async_channel::Sender<Event>) -> Result<String, String> {
        if command.trim().is_empty() { return Err("命令为空".into()); }
        let (script, argv) = shell_invocation(command, args);
        // 登录 shell：从 Finder 打开的应用 PATH 很短，用户的 cargo / node 往往不在里面
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".to_string());
        let mut cmd = Command::new(shell);
        cmd.arg("-l").arg("-c").arg(script).args(argv).envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("无法运行 {command}：{e}"))?;
        let pid = child.id();
        let mut stdout = child.stdout.take().unwrap();
        let buf = Arc::new(Mutex::new(OutputBuf::new(limit)));
        let exit: Arc<Mutex<Option<Exit>>> = Arc::new(Mutex::new(None));
        self.next += 1;
        let id = format!("term-{}", self.next);

        let (b2, ev2) = (buf.clone(), events.clone());
        let reader = std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            let mut last = Instant::now() - NOTIFY_EVERY;
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut b) = b2.lock() { b.push(&chunk[..n]); }
                        if last.elapsed() >= NOTIFY_EVERY { last = Instant::now(); if ev2.send_blocking(Event::TermOutput).is_err() { break; } }
                    }
                }
            }
        });
        let (e2, id2) = (exit.clone(), id.clone());
        std::thread::spawn(move || {
            let st = child.wait().ok();
            // 先等读线程把管道里剩下的输出读完，再宣布退出，免得丢最后几行
            let _ = reader.join();
            let ex = st.map(|s| Exit { code: s.code(), signal: s.signal() }).unwrap_or(Exit { code: None, signal: None });
            if let Ok(mut e) = e2.lock() { *e = Some(ex); }
            let _ = events.send_blocking(Event::TermExit(id2));
        });
        self.map.insert(id.clone(), Term { buf, exit, pid });
        Ok(id)
    }

    /// (输出, 是否被截断, 退出状态)
    pub fn output(&self, id: &str) -> Option<(String, bool, Option<Exit>)> {
        let t = self.map.get(id)?;
        let b = t.buf.lock().ok()?;
        Some((b.text().to_string(), b.truncated(), t.exit.lock().ok().and_then(|e| *e)))
    }

    /// 外层 None = 没有这个终端；内层 None = 还在运行
    pub fn exit(&self, id: &str) -> Option<Option<Exit>> {
        self.map.get(id).map(|t| t.exit.lock().ok().and_then(|e| *e))
    }

    pub fn kill(&self, id: &str) {
        if let Some(t) = self.map.get(id) { if self.exit(id) == Some(None) { kill_group(t.pid); } }
    }

    /// 停止时：终止所有还在跑的命令
    pub fn kill_all(&self) {
        for id in self.map.keys() { self.kill(id); }
    }

    /// 界面用：(输出, 是否还在运行)
    pub fn view(&self, id: &str) -> Option<(String, bool)> {
        let (text, _, exit) = self.output(id)?;
        Some((text, exit.is_none()))
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        for (id, t) in self.map.iter() { if self.exit(id) == Some(None) { kill_group(t.pid); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(command: &str, args: &[&str]) -> (Terminals, String, async_channel::Receiver<Event>) {
        let (tx, rx) = async_channel::unbounded();
        let mut t = Terminals::default();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let id = t.create(command, &args, &[], &std::env::temp_dir(), None, tx).unwrap();
        (t, id, rx)
    }

    fn wait_exit(rx: &async_channel::Receiver<Event>) -> String {
        loop {
            match rx.recv_blocking().unwrap() { Event::TermExit(id) => return id, _ => continue }
        }
    }

    #[test]
    fn 跑完的命令有完整输出和退出码() {
        let (t, id, rx) = run("sh", &["-c", "printf 'a\\nb\\n'; exit 3"]);
        assert_eq!(wait_exit(&rx), id);
        let (out, truncated, exit) = t.output(&id).unwrap();
        assert_eq!(out, "a\nb\n");
        assert!(!truncated);
        assert_eq!(exit, Some(Exit { code: Some(3), signal: None }));
    }

    #[test]
    fn stderr_合并进输出() {
        let (t, id, rx) = run("sh", &["-c", "echo out; echo err >&2"]);
        wait_exit(&rx);
        let out = t.output(&id).unwrap().0;
        assert!(out.contains("out") && out.contains("err"), "{out}");
    }

    #[test]
    fn 整条命令行塞进_command_也能跑() {
        let (t, id, rx) = run("echo hello | tr a-z A-Z", &[]);
        wait_exit(&rx);
        assert_eq!(t.output(&id).unwrap().0.trim(), "HELLO");
    }

    #[test]
    fn 参数里的空格和引号原样传递() {
        let (t, id, rx) = run("printf", &["%s|", "a b", "c'd"]);
        wait_exit(&rx);
        assert_eq!(t.output(&id).unwrap().0, "a b|c'd|");
    }

    #[test]
    fn 终止长时间运行的命令() {
        let (t, id, rx) = run("sleep", &["30"]);
        assert_eq!(t.exit(&id), Some(None)); // 还在运行
        t.kill(&id);
        wait_exit(&rx);
        let exit = t.exit(&id).unwrap().unwrap();
        assert_eq!(exit.signal, Some(9));
        assert_eq!(t.exit("不存在"), None);
    }

    #[test]
    fn 环境变量和工作目录生效() {
        let (tx, rx) = async_channel::unbounded();
        let mut t = Terminals::default();
        let dir = std::env::temp_dir();
        let id = t.create("sh", &["-c".into(), "echo $QUILL_T; pwd".into()], &[("QUILL_T".into(), "值1".into())], &dir, None, tx).unwrap();
        wait_exit(&rx);
        let out = t.output(&id).unwrap().0;
        assert!(out.starts_with("值1\n"), "{out}");
        let want = dir.canonicalize().unwrap();
        assert!(out.lines().nth(1).map(|l| Path::new(l).canonicalize().ok() == Some(want.clone())).unwrap_or(false), "{out}");
    }

    #[test]
    fn 空命令报错() {
        let (tx, _rx) = async_channel::unbounded();
        assert!(Terminals::default().create("  ", &[], &[], &std::env::temp_dir(), None, tx).is_err());
    }
}

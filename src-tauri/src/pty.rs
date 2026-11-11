//! 内置终端：portable-pty，数据经事件推给前端 xterm.js。
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, Emitter, State};

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[derive(Default)]
pub struct Ptys {
    next: AtomicU32,
    map: Mutex<HashMap<u32, Session>>,
}

#[derive(Serialize, Clone)]
struct PtyData {
    id: u32,
    data: String,
}

/// 把字节流切成合法 UTF-8，残留的半个字符留给下一次。
pub fn split_utf8(buf: &mut Vec<u8>) -> String {
    match std::str::from_utf8(buf) {
        Ok(s) => {
            let s = s.to_string();
            buf.clear();
            s
        }
        Err(e) => {
            let n = e.valid_up_to();
            let s = String::from_utf8_lossy(&buf[..n]).to_string();
            // 不完整的尾部（至多 3 字节）留下；真正的非法字节丢弃
            if e.error_len().is_some() {
                buf.clear();
            } else {
                buf.drain(..n);
            }
            s
        }
    }
}

#[tauri::command]
pub fn pty_spawn(app: AppHandle, state: State<Ptys>, cwd: String, cols: u16, rows: u16) -> Result<u32, String> {
    let pair = native_pty_system()
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut cmd = CommandBuilder::new(shell);
    cmd.arg("-l");
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let id = state.next.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut carry: Vec<u8> = Vec::new();
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            carry.extend_from_slice(&chunk[..n]);
            let data = split_utf8(&mut carry);
            if !data.is_empty() {
                let _ = app.emit("pty-data", PtyData { id, data });
            }
        }
        let _ = app.emit("pty-exit", id);
    });
    state.map.lock().unwrap().insert(id, Session { master: pair.master, writer, child });
    Ok(id)
}

#[tauri::command]
pub fn pty_write(state: State<Ptys>, id: u32, data: String) {
    if let Some(s) = state.map.lock().unwrap().get_mut(&id) {
        let _ = s.writer.write_all(data.as_bytes());
    }
}

#[tauri::command]
pub fn pty_resize(state: State<Ptys>, id: u32, cols: u16, rows: u16) {
    if let Some(s) = state.map.lock().unwrap().get(&id) {
        let _ = s.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    }
}

#[tauri::command]
pub fn pty_kill(state: State<Ptys>, id: u32) {
    if let Some(mut s) = state.map.lock().unwrap().remove(&id) {
        let _ = s.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 半个汉字留到下一次() {
        let bytes = "你好".as_bytes();
        let mut buf = bytes[..4].to_vec(); // 你 + 好的前 1 字节
        assert_eq!(split_utf8(&mut buf), "你");
        buf.extend_from_slice(&bytes[4..]);
        assert_eq!(split_utf8(&mut buf), "好");
        assert!(buf.is_empty());
    }
}

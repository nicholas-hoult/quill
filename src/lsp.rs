//! LSP 客户端：JSON-RPC over stdio。
//! 纯逻辑部分（帧解析、UTF-16 位置换算、响应解析）带测试；错了表现为：
//! 诊断下划线错位（中文 / emoji 之后偏移）、补全插入位置不对、跳转定义打开了错误的文件。
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

// ---------- 纯逻辑 ----------

pub fn encode_frame(v: &Value) -> Vec<u8> {
    let body = v.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// 增量帧解析：喂入任意切分的字节，吐出完整的 JSON 消息。
#[derive(Default)]
pub struct FrameParser {
    buf: Vec<u8>,
}

impl FrameParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.buf.extend_from_slice(bytes);
        let mut out = vec![];
        loop {
            let Some(h) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else { break };
            let header = String::from_utf8_lossy(&self.buf[..h]).to_string();
            let len = header.lines().find_map(|l| l.strip_prefix("Content-Length:").or_else(|| l.strip_prefix("content-length:"))).and_then(|v| v.trim().parse::<usize>().ok());
            let Some(len) = len else { self.buf.drain(..h + 4); continue };
            if self.buf.len() < h + 4 + len { break; }
            let body: Vec<u8> = self.buf[h + 4..h + 4 + len].to_vec();
            self.buf.drain(..h + 4 + len);
            if let Ok(v) = serde_json::from_slice::<Value>(&body) { out.push(v); }
        }
        out
    }
}

/// 字节列（行内）→ UTF-16 列
pub fn utf16_col(line: &str, byte_col: usize) -> usize {
    line[..byte_col.min(line.len())].chars().map(|c| c.len_utf16()).sum()
}

/// UTF-16 列 → 字节列（行内）；超出行尾则取行尾
pub fn byte_col(line: &str, utf16: usize) -> usize {
    let mut u = 0;
    for (i, c) in line.char_indices() {
        if u >= utf16 { return i; }
        u += c.len_utf16();
    }
    line.len()
}

pub fn path_to_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) { out.push(b); i += 3; continue; }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawDiag {
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// 1 错误 / 2 警告 / 3 信息 / 4 提示
    pub severity: u8,
    pub message: String,
    pub source: String,
}

pub fn parse_diagnostics(params: &Value) -> Option<(String, Vec<RawDiag>)> {
    let uri = params["uri"].as_str()?.to_string();
    let pos = |v: &Value| (v["line"].as_u64().unwrap_or(0) as usize, v["character"].as_u64().unwrap_or(0) as usize);
    let diags = params["diagnostics"].as_array()?.iter().map(|d| RawDiag {
        start: pos(&d["range"]["start"]),
        end: pos(&d["range"]["end"]),
        severity: d["severity"].as_u64().unwrap_or(1) as u8,
        message: d["message"].as_str().unwrap_or("").to_string(),
        source: d["source"].as_str().unwrap_or("").to_string(),
    }).collect();
    Some((uri, diags))
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: u8,
    pub detail: String,
    /// 要插入的文本（已去掉 snippet 占位符）
    pub insert: String,
    pub sort: String,
}

/// 去掉 snippet 语法：`${1:name}` → name，`$1` / `$0` → 空
pub fn strip_snippet(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '$' && i + 1 < cs.len() {
            if cs[i + 1] == '{' {
                if let Some(end) = cs[i..].iter().position(|&c| c == '}') {
                    let inner: String = cs[i + 2..i + end].iter().collect();
                    if let Some((_, def)) = inner.split_once(':') { out.push_str(def); }
                    i += end + 1;
                    continue;
                }
            } else if cs[i + 1].is_ascii_digit() {
                i += 2;
                while i < cs.len() && cs[i].is_ascii_digit() { i += 1; }
                continue;
            }
        }
        if cs[i] == '\\' && i + 1 < cs.len() { out.push(cs[i + 1]); i += 2; continue; }
        out.push(cs[i]);
        i += 1;
    }
    out
}

pub fn parse_completion(resp: &Value) -> Vec<CompletionItem> {
    let items = resp["items"].as_array().or_else(|| resp.as_array());
    let Some(items) = items else { return vec![] };
    let mut out: Vec<CompletionItem> = items.iter().filter_map(|it| {
        let label = it["label"].as_str()?.to_string();
        let raw = it["textEdit"]["newText"].as_str().or_else(|| it["insertText"].as_str()).unwrap_or(&label).to_string();
        let insert = if it["insertTextFormat"].as_u64() == Some(2) { strip_snippet(&raw) } else { raw };
        Some(CompletionItem {
            kind: it["kind"].as_u64().unwrap_or(1) as u8,
            detail: it["detail"].as_str().or_else(|| it["labelDetails"]["detail"].as_str()).unwrap_or("").to_string(),
            sort: it["sortText"].as_str().unwrap_or(&label).to_string(),
            insert,
            label,
        })
    }).collect();
    out.sort_by(|a, b| a.sort.cmp(&b.sort));
    out
}

/// 补全项类型的简短标签
pub fn kind_label(kind: u8) -> &'static str {
    match kind { 2 | 3 | 4 => "fn", 5 | 10 => "字段", 6 => "变量", 7 | 22 => "类型", 8 => "接口", 9 => "模块", 13 | 20 => "枚举", 14 => "关键字", 15 => "片段", 21 => "常量", _ => "文本" }
}

/// hover 内容 → Markdown 文本（兼容 string / MarkupContent / MarkedString[]）
pub fn parse_hover(resp: &Value) -> Option<String> {
    let c = &resp["contents"];
    let s = if let Some(s) = c.as_str() { s.to_string() }
        else if let Some(v) = c["value"].as_str() { if let Some(l) = c["language"].as_str() { format!("```{l}\n{v}\n```") } else { v.to_string() } }
        else if let Some(a) = c.as_array() {
            a.iter().filter_map(|x| x.as_str().map(String::from).or_else(|| x["value"].as_str().map(|v| match x["language"].as_str() { Some(l) => format!("```{l}\n{v}\n```"), None => v.to_string() }))).collect::<Vec<_>>().join("\n\n")
        } else { return None };
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// definition 响应 → (uri, 行, UTF-16 列)
pub fn parse_definition(resp: &Value) -> Option<(String, usize, usize)> {
    let loc = if resp.is_array() { resp.get(0)? } else { resp };
    if loc.is_null() { return None; }
    let uri = loc["uri"].as_str().or_else(|| loc["targetUri"].as_str())?.to_string();
    let r = if loc["range"].is_object() { &loc["range"] } else { &loc["targetSelectionRange"] };
    Some((uri, r["start"]["line"].as_u64()? as usize, r["start"]["character"].as_u64().unwrap_or(0) as usize))
}

// ---------- 客户端 ----------

#[derive(Debug)]
pub enum LspEvent {
    Diagnostics { uri: String, diags: Vec<RawDiag> },
    Ready(String),
    Exited(String),
}

type Pending = Arc<Mutex<HashMap<i64, async_channel::Sender<Result<Value, String>>>>>;

pub struct Client {
    out: mpsc::Sender<Value>,
    next_id: AtomicI64,
    pending: Pending,
    child: Mutex<Child>,
    pub ready: Arc<AtomicBool>,
}

impl Drop for Client {
    fn drop(&mut self) { let _ = self.child.lock().map(|mut c| c.kill()); }
}

impl Client {
    pub fn start(cmd: &str, args: &[String], root: &Path, tag: &str, events: async_channel::Sender<LspEvent>) -> Result<Arc<Client>, String> {
        let mut child = Command::new(cmd).args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("{cmd}: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let (out_tx, out_rx) = mpsc::channel::<Value>();
        let pending: Pending = Default::default();
        let ready = Arc::new(AtomicBool::new(false));
        let client = Arc::new(Client { out: out_tx.clone(), next_id: AtomicI64::new(2), pending: pending.clone(), child: Mutex::new(child), ready: ready.clone() });

        // 写线程：先发 initialize，等到响应后发 initialized，再放行排队的消息
        let (init_tx, init_rx) = mpsc::channel::<()>();
        let root_uri = path_to_uri(&root.to_string_lossy());
        let root_name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        std::thread::spawn(move || {
            let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "processId": std::process::id(), "rootUri": root_uri, "workspaceFolders": [{ "uri": root_uri, "name": root_name }],
                "clientInfo": { "name": "Quill" },
                "capabilities": {
                    "textDocument": {
                        "synchronization": { "didSave": true },
                        "publishDiagnostics": { "relatedInformation": false },
                        "completion": { "completionItem": { "snippetSupport": false, "documentationFormat": ["plaintext"] } },
                        "hover": { "contentFormat": ["markdown", "plaintext"] },
                        "definition": { "linkSupport": true }
                    },
                    "workspace": { "workspaceFolders": true, "configuration": true }
                } } });
            if stdin.write_all(&encode_frame(&init)).is_err() { return; }
            let _ = stdin.flush();
            // 等 initialize 响应（读线程收到 id=1 的响应后通知）
            if init_rx.recv_timeout(std::time::Duration::from_secs(60)).is_err() { return; }
            let _ = stdin.write_all(&encode_frame(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} })));
            let _ = stdin.flush();
            while let Ok(m) = out_rx.recv() {
                if stdin.write_all(&encode_frame(&m)).is_err() { break; }
                let _ = stdin.flush();
            }
        });

        // 读线程
        let reply_tx = out_tx;
        let tag = tag.to_string();
        std::thread::spawn(move || {
            let mut parser = FrameParser::default();
            let mut buf = [0u8; 16384];
            let mut init_tx = Some(init_tx);
            loop {
                let n = match stdout.read(&mut buf) { Ok(0) | Err(_) => break, Ok(n) => n };
                for msg in parser.push(&buf[..n]) {
                    let id = msg.get("id").cloned();
                    let method = msg["method"].as_str().map(String::from);
                    match (id, method) {
                        (Some(id), Some(m)) => {
                            // 服务端发来的请求：需要回应
                            let result = if m == "workspace/configuration" {
                                json!(msg["params"]["items"].as_array().map(|a| a.iter().map(|_| Value::Null).collect::<Vec<_>>()).unwrap_or_default())
                            } else { Value::Null };
                            let _ = reply_tx.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                        }
                        (Some(id), None) => {
                            if id == json!(1) {
                                ready.store(true, Ordering::SeqCst);
                                if let Some(t) = init_tx.take() { let _ = t.send(()); }
                                let _ = events.send_blocking(LspEvent::Ready(tag.clone()));
                            } else if let Some(i) = id.as_i64() {
                                if let Some(tx) = pending.lock().unwrap().remove(&i) {
                                    let r = if let Some(e) = msg.get("error") { Err(e["message"].as_str().unwrap_or("error").to_string()) } else { Ok(msg["result"].clone()) };
                                    let _ = tx.send_blocking(r);
                                }
                            }
                        }
                        (None, Some(m)) => {
                            if m == "textDocument/publishDiagnostics" {
                                if let Some((uri, diags)) = parse_diagnostics(&msg["params"]) { let _ = events.send_blocking(LspEvent::Diagnostics { uri, diags }); }
                            }
                        }
                        _ => {}
                    }
                }
            }
            let _ = events.send_blocking(LspEvent::Exited(tag));
        });
        Ok(client)
    }

    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.out.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn request(&self, method: &str, params: Value) -> async_channel::Receiver<Result<Value, String>> {
        let (tx, rx) = async_channel::bounded(1);
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().insert(id, tx);
        let _ = self.out.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }
}

/// 各语言默认的语言服务器命令；环境变量 QUILL_LSP_<名字> 可覆盖（整条命令按空格切分）
pub fn server_command(lang_key: &str) -> Option<(String, Vec<String>)> {
    let env_key = format!("QUILL_LSP_{}", lang_key.to_ascii_uppercase());
    let raw = std::env::var(&env_key).ok().or_else(|| match lang_key {
        "rust" => Some("rust-analyzer".into()),
        "typescript" | "javascript" => Some("typescript-language-server --stdio".into()),
        "python" => Some("pyright-langserver --stdio".into()),
        "go" => Some("gopls".into()),
        _ => None,
    })?;
    let mut parts = raw.split_whitespace().map(String::from);
    let cmd = parts.next()?;
    Some((cmd, parts.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 帧编码与任意切分的增量解析() {
        let a = encode_frame(&json!({"id":1,"result":"你好"}));
        let b = encode_frame(&json!({"method":"x"}));
        let all: Vec<u8> = [a, b].concat();
        let mut p = FrameParser::default();
        let mut got = vec![];
        for chunk in all.chunks(7) { got.extend(p.push(chunk)); }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["result"], "你好");
        assert_eq!(got[1]["method"], "x");
    }

    #[test]
    fn utf16_列换算_中文与_emoji() {
        let line = "a你😀b";
        assert_eq!(utf16_col(line, 0), 0);
        assert_eq!(utf16_col(line, 1), 1);
        assert_eq!(utf16_col(line, 4), 2); // 你 = 3 字节 / 1 个 UTF-16 单元
        assert_eq!(utf16_col(line, 8), 4); // 😀 = 4 字节 / 2 个单元
        assert_eq!(byte_col(line, 2), 4);
        assert_eq!(byte_col(line, 4), 8);
        assert_eq!(byte_col(line, 99), line.len());
        // 往返
        for (i, _) in line.char_indices() { assert_eq!(byte_col(line, utf16_col(line, i)), i); }
    }

    #[test]
    fn uri_与路径互转() {
        let p = "/Users/me/my proj/src/汉字.rs";
        let u = path_to_uri(p);
        assert!(u.starts_with("file:///Users/me/my%20proj/"));
        assert_eq!(uri_to_path(&u).unwrap(), p);
        assert_eq!(uri_to_path("http://x"), None);
    }

    #[test]
    fn 解析诊断() {
        let p = json!({"uri":"file:///a.rs","diagnostics":[{"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":9}},"severity":2,"message":"unused","source":"rustc"}]});
        let (uri, d) = parse_diagnostics(&p).unwrap();
        assert_eq!(uri, "file:///a.rs");
        assert_eq!(d[0], RawDiag { start: (2, 4), end: (2, 9), severity: 2, message: "unused".into(), source: "rustc".into() });
    }

    #[test]
    fn snippet_占位符被还原() {
        assert_eq!(strip_snippet("println!(\"${1:msg}\"$0)"), "println!(\"msg\")");
        assert_eq!(strip_snippet("a$1b"), "ab");
        assert_eq!(strip_snippet("cost \\$5"), "cost $5");
    }

    #[test]
    fn 解析补全_按_sortText_排序() {
        let r = json!({"isIncomplete":false,"items":[
            {"label":"zeta","kind":3,"sortText":"2"},
            {"label":"alpha","kind":6,"detail":"i32","sortText":"1","insertText":"alpha_x"},
            {"label":"snip","kind":15,"insertTextFormat":2,"textEdit":{"newText":"f(${1:a})","range":{}}}]});
        let c = parse_completion(&r);
        assert_eq!(c.iter().map(|x| x.label.as_str()).collect::<Vec<_>>(), vec!["alpha", "zeta", "snip"]);
        assert_eq!(c[0].insert, "alpha_x");
        assert_eq!(c[2].insert, "f(a)");
        assert_eq!(parse_completion(&json!([{"label":"x"}])).len(), 1);
        assert!(parse_completion(&json!(null)).is_empty());
    }

    #[test]
    fn 解析悬停与定义() {
        assert_eq!(parse_hover(&json!({"contents":{"kind":"markdown","value":"```rust\nfn a()\n```"}})).unwrap(), "```rust\nfn a()\n```");
        assert_eq!(parse_hover(&json!({"contents":["x",{"language":"rust","value":"fn b()"}]})).unwrap(), "x\n\n```rust\nfn b()\n```");
        assert_eq!(parse_hover(&json!({"contents":""})), None);
        let d = parse_definition(&json!([{"targetUri":"file:///b.rs","targetSelectionRange":{"start":{"line":3,"character":1}}}])).unwrap();
        assert_eq!(d, ("file:///b.rs".into(), 3, 1));
        let d2 = parse_definition(&json!({"uri":"file:///c.rs","range":{"start":{"line":0,"character":0}}})).unwrap();
        assert_eq!(d2.0, "file:///c.rs");
        assert_eq!(parse_definition(&json!(null)), None);
    }
}

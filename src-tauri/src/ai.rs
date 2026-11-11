//! AI 流式对话：Anthropic / OpenAI 兼容 / 本地 mock（无 Key 时演示与测试）。
//! Key 只存在 Rust 端，不进前端。
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::task::AbortHandle;

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    /// "mock" | "anthropic" | "openai"
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { provider: "mock".into(), api_key: String::new(), base_url: String::new(), model: "claude-sonnet-5-5".into() }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Default)]
pub struct Streams(Mutex<HashMap<String, AbortHandle>>);

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    load_settings(&app)
}

#[tauri::command]
pub fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let p = settings_path(&app)?;
    std::fs::write(p, serde_json::to_string_pretty(&settings).unwrap()).map_err(|e| e.to_string())
}

/// 解析一行 SSE，取出文本增量。
pub fn parse_sse_line(provider: &str, line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if provider == "anthropic" {
        if v["type"] == "content_block_delta" {
            return v["delta"]["text"].as_str().map(String::from);
        }
        None
    } else {
        v["choices"][0]["delta"]["content"].as_str().map(String::from)
    }
}

/// 把字节块拼成完整行（跨块的行留在 buf 里）。
pub fn drain_lines(buf: &mut String, chunk: &str) -> Vec<String> {
    buf.push_str(chunk);
    let mut out = Vec::new();
    while let Some(i) = buf.find('\n') {
        out.push(buf[..i].trim_end_matches('\r').to_string());
        buf.drain(..=i);
    }
    out
}

/// mock 回复：若上下文里带了 <file path="..">，就回一个同路径的代码块，用来验证「应用」流程。
pub fn mock_reply(messages: &[Msg]) -> String {
    let last = messages.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
    // Inline Assist：消息里带「选中的代码」围栏，就把选中内容加一行注释后原样回传
    if let Some(i) = last.find("选中的代码") {
        let rest = &last[i..];
        if let (Some(a), Some(b)) = (rest.find("```\n"), rest.rfind("\n```")) {
            if b >= a + 4 {
                return format!("```\n// edited by Quill\n{}\n```", &rest[a + 4..b]);
            }
        }
        return "```\n// edited by Quill\n```".into();
    }
    if let Some(start) = last.find("<file path=\"") {
        let rest = &last[start + 12..];
        if let Some(end) = rest.find('"') {
            let path = &rest[..end];
            let body_start = rest.find('>').map(|i| i + 1).unwrap_or(0);
            let body_end = rest.find("</file>").unwrap_or(rest.len());
            let body = rest[body_start..body_end].trim_matches('\n');
            let lang = path.rsplit('.').next().unwrap_or("txt");
            return format!(
                "（演示模式，未配置 API Key）我在文件头部加了一行注释，其余保持不变：\n\n```{lang} path={path}\n// edited by Quill\n{body}\n```\n\n在设置里填入 Key 即可使用真实模型。"
            );
        }
    }
    "（演示模式，未配置 API Key）你好，我是 Quill。把文件用 @ 引用进来，我就能给出可一键应用的修改。".into()
}

fn emit_chunk(app: &AppHandle, id: &str, delta: &str) {
    let _ = app.emit("ai-chunk", serde_json::json!({ "id": id, "delta": delta }));
}

async fn run(app: AppHandle, id: String, system: String, messages: Vec<Msg>) -> Result<(), String> {
    let s = load_settings(&app);
    if s.provider == "mock" {
        for piece in mock_reply(&messages).chars().collect::<Vec<_>>().chunks(3) {
            emit_chunk(&app, &id, &piece.iter().collect::<String>());
            tokio::time::sleep(Duration::from_millis(12)).await;
        }
        return Ok(());
    }
    if s.api_key.is_empty() {
        return Err("未配置 API Key，请在设置中填写".into());
    }
    let client = reqwest::Client::new();
    let req = if s.provider == "anthropic" {
        let base = if s.base_url.is_empty() { "https://api.anthropic.com" } else { s.base_url.trim_end_matches('/') };
        client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", &s.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({ "model": s.model, "max_tokens": 8192, "stream": true, "system": system, "messages": messages }))
    } else {
        let base = if s.base_url.is_empty() { "https://api.openai.com/v1" } else { s.base_url.trim_end_matches('/') };
        let mut all = vec![Msg { role: "system".into(), content: system }];
        all.extend(messages);
        client
            .post(format!("{base}/chat/completions"))
            .bearer_auth(&s.api_key)
            .json(&serde_json::json!({ "model": s.model, "stream": true, "messages": all }))
    };
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{code}: {}", body.chars().take(300).collect::<String>()));
    }
    let mut stream = resp.bytes_stream();
    let mut line_buf = String::new();
    let mut byte_buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        byte_buf.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        let text = crate::pty::split_utf8(&mut byte_buf);
        for line in drain_lines(&mut line_buf, &text) {
            if let Some(d) = parse_sse_line(&s.provider, &line) {
                emit_chunk(&app, &id, &d);
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn ai_stream(app: AppHandle, streams: State<Streams>, id: String, system: String, messages: Vec<Msg>) {
    let (a, i) = (app.clone(), id.clone());
    let handle = tauri::async_runtime::spawn(async move {
        let r = run(a.clone(), i.clone(), system, messages).await;
        match r {
            Ok(()) => {
                let _ = a.emit("ai-done", serde_json::json!({ "id": i }));
            }
            Err(m) => {
                let _ = a.emit("ai-error", serde_json::json!({ "id": i, "message": m }));
            }
        }
        a.state::<Streams>().0.lock().unwrap().remove(&i);
    });
    streams.0.lock().unwrap().insert(id, handle.inner().abort_handle());
}

#[tauri::command]
pub fn ai_cancel(streams: State<Streams>, id: String) {
    if let Some(h) = streams.0.lock().unwrap().remove(&id) {
        h.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析_anthropic_增量() {
        let l = r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"你好"}}"#;
        assert_eq!(parse_sse_line("anthropic", l).as_deref(), Some("你好"));
        assert_eq!(parse_sse_line("anthropic", r#"data: {"type":"message_start"}"#), None);
    }

    #[test]
    fn 解析_openai_增量与结束() {
        let l = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert_eq!(parse_sse_line("openai", l).as_deref(), Some("hi"));
        assert_eq!(parse_sse_line("openai", "data: [DONE]"), None);
        assert_eq!(parse_sse_line("openai", "event: ping"), None);
    }

    #[test]
    fn 跨块的行能拼回() {
        let mut buf = String::new();
        assert!(drain_lines(&mut buf, "data: {\"a\"").is_empty());
        let r = drain_lines(&mut buf, ":1}\r\ndata: x\n");
        assert_eq!(r, vec!["data: {\"a\":1}", "data: x"]);
    }

    #[test]
    fn mock_行内改写回传选中内容() {
        let m = vec![Msg { role: "user".into(), content: "改名\n\n选中的代码：\n```\nlet a = 1\n```".into() }];
        assert_eq!(mock_reply(&m), "```\n// edited by Quill\nlet a = 1\n```");
    }

    #[test]
    fn mock_带文件时回同路径代码块() {
        let m = vec![Msg { role: "user".into(), content: "改\n<file path=\"src/a.ts\">\nlet x=1\n</file>".into() }];
        let r = mock_reply(&m);
        assert!(r.contains("```ts path=src/a.ts"));
        assert!(r.contains("let x=1"));
    }
}

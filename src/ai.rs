//! AI 流式对话：Anthropic / OpenAI 兼容 / 本地 mock（无 Key 时演示与测试）。
//! 网络请求是阻塞式的，放在后台线程里跑，事件经 channel 送回界面线程。
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Settings {
    /// "mock" | "anthropic" | "openai"
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// "system" | "dark" | "light"
    pub theme: String,
    /// 界面布局（下次启动恢复）
    #[serde(default = "d_side")]
    pub side_w: f32,
    #[serde(default = "d_agent")]
    pub agent_w: f32,
    #[serde(default = "d_term")]
    pub term_h: f32,
    #[serde(default = "d_true")]
    pub sidebar: bool,
    #[serde(default = "d_true")]
    pub agent_open: bool,
}

fn d_side() -> f32 { 264.0 }
fn d_agent() -> f32 { 440.0 }
fn d_term() -> f32 { 280.0 }
fn d_true() -> bool { true }

impl Default for Settings {
    fn default() -> Self {
        Self { provider: "mock".into(), api_key: String::new(), base_url: String::new(), model: "claude-sonnet-5-5".into(), theme: "system".into(), side_w: d_side(), agent_w: d_agent(), term_h: d_term(), sidebar: true, agent_open: true }
    }
}

fn settings_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join("Library/Application Support/Quill/settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        settings_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let p = settings_path().ok_or("找不到 HOME")?;
        if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
        std::fs::write(p, serde_json::to_string_pretty(self).unwrap()).map_err(|e| e.to_string())
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

#[derive(Debug)]
pub enum StreamEvent {
    Chunk(String),
    Done,
    Error(String),
}

/// 解析一行 SSE，取出文本增量。
pub fn parse_sse_line(provider: &str, line: &str) -> Option<String> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" { return None; }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if provider == "anthropic" {
        if v["type"] == "content_block_delta" { return v["delta"]["text"].as_str().map(String::from); }
        None
    } else {
        v["choices"][0]["delta"]["content"].as_str().map(String::from)
    }
}

/// mock 回复：带文件上下文就回一个同路径代码块（验证「应用」流程）；带选中代码就原样回传并加一行注释（验证行内改写）。
pub fn mock_reply(messages: &[Msg]) -> String {
    let last = messages.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str()).unwrap_or("");
    if let Some(i) = last.find("选中的代码") {
        let rest = &last[i..];
        if let (Some(a), Some(b)) = (rest.find("```\n"), rest.rfind("\n```")) {
            if b >= a + 4 { return format!("```\n// edited by Quill\n{}\n```", &rest[a + 4..b]); }
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
            let comment = if matches!(lang, "py" | "sh") { "#" } else { "//" };
            return format!(
                "（演示模式，未配置 API Key）我在文件头部加了一行注释，其余保持不变：\n\n```{lang} path={path}\n{comment} edited by Quill\n{body}\n```\n\n在设置里填入 Key 即可使用真实模型。"
            );
        }
    }
    "（演示模式，未配置 API Key）你好，我是 Quill。用 @ 把文件引用进来，我就能给出可一键应用、可逐块审阅的修改。".into()
}

/// 阻塞地跑完一次流式请求，把事件发给 `tx`。`cancel` 置位后尽快退出。
pub fn run_stream(s: Settings, system: String, messages: Vec<Msg>, tx: async_channel::Sender<StreamEvent>, cancel: Arc<AtomicBool>) {
    let send = |e: StreamEvent| { let _ = tx.send_blocking(e); };
    if s.provider == "mock" {
        let text: Vec<char> = mock_reply(&messages).chars().collect();
        std::thread::sleep(Duration::from_millis(250));
        for piece in text.chunks(3) {
            if cancel.load(Ordering::Relaxed) { return; }
            send(StreamEvent::Chunk(piece.iter().collect()));
            std::thread::sleep(Duration::from_millis(12));
        }
        send(StreamEvent::Done);
        return;
    }
    if s.api_key.is_empty() { send(StreamEvent::Error("未配置 API Key，请在设置中填写".into())); return; }
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(600))).build().into();
    let resp = if s.provider == "anthropic" {
        let base = if s.base_url.is_empty() { "https://api.anthropic.com".to_string() } else { s.base_url.trim_end_matches('/').to_string() };
        let body = serde_json::json!({ "model": s.model, "max_tokens": 8192, "stream": true, "system": system, "messages": messages });
        agent.post(format!("{base}/v1/messages")).header("x-api-key", &s.api_key).header("anthropic-version", "2023-06-01").send_json(body)
    } else {
        let base = if s.base_url.is_empty() { "https://api.openai.com/v1".to_string() } else { s.base_url.trim_end_matches('/').to_string() };
        let mut all = vec![Msg { role: "system".into(), content: system }];
        all.extend(messages);
        let body = serde_json::json!({ "model": s.model, "stream": true, "messages": all });
        agent.post(format!("{base}/chat/completions")).header("authorization", format!("Bearer {}", s.api_key)).send_json(body)
    };
    let mut resp = match resp { Ok(r) => r, Err(e) => { send(StreamEvent::Error(e.to_string())); return; } };
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        send(StreamEvent::Error(format!("{code}: {}", body.chars().take(300).collect::<String>())));
        return;
    }
    let reader = std::io::BufReader::new(resp.into_body().into_reader());
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) { return; }
        match line {
            Ok(l) => { if let Some(d) = parse_sse_line(&s.provider, l.trim_end_matches('\r')) { send(StreamEvent::Chunk(d)); } }
            Err(e) => { send(StreamEvent::Error(e.to_string())); return; }
        }
    }
    send(StreamEvent::Done);
}

#[derive(Debug)]
pub enum AgentStreamEvent {
    Chunk(String),
    ToolUse { id: String, name: String, input: serde_json::Value },
    /// 停止原因：end_turn / tool_use …
    Done(String),
    Error(String),
}

fn flatten_text_msgs(messages: &[serde_json::Value]) -> Vec<Msg> {
    messages.iter().filter_map(|m| Some(Msg { role: m["role"].as_str()?.to_string(), content: m["content"].as_str()?.to_string() })).collect()
}

/// 带工具调用的流式请求（Agent 模式）。Anthropic 与 mock 支持工具；OpenAI 兼容退回纯文本。
pub fn run_agent_stream(s: Settings, system: String, messages: Vec<serde_json::Value>, tools: bool, tx: async_channel::Sender<AgentStreamEvent>, cancel: Arc<AtomicBool>) {
    use crate::agent_proto::{ProtoEvent, SseParser};
    let send = |e: AgentStreamEvent| { let _ = tx.send_blocking(e); };
    let emit = |ev: ProtoEvent, stop: &mut String| match ev {
        ProtoEvent::Text(t) => send(AgentStreamEvent::Chunk(t)),
        ProtoEvent::ToolUse { id, name, input } => send(AgentStreamEvent::ToolUse { id, name, input }),
        ProtoEvent::Stop(r) => *stop = r,
    };
    if s.provider == "mock" {
        std::thread::sleep(Duration::from_millis(250));
        let events = crate::agent_proto::mock_turn(&messages).or_else(|| crate::agent_proto::mock_cmd_turn(&messages)).unwrap_or_else(|| vec![ProtoEvent::Text(mock_reply(&flatten_text_msgs(&messages))), ProtoEvent::Stop("end_turn".into())]);
        let mut stop = String::from("end_turn");
        for ev in events {
            if cancel.load(Ordering::Relaxed) { return; }
            match ev {
                ProtoEvent::Text(t) => {
                    for piece in t.chars().collect::<Vec<_>>().chunks(3) {
                        if cancel.load(Ordering::Relaxed) { return; }
                        send(AgentStreamEvent::Chunk(piece.iter().collect()));
                        std::thread::sleep(Duration::from_millis(12));
                    }
                }
                other => emit(other, &mut stop),
            }
        }
        send(AgentStreamEvent::Done(stop));
        return;
    }
    if s.provider != "anthropic" {
        // OpenAI 兼容：没有工具，复用纯文本流
        let (itx, irx) = async_channel::unbounded();
        let (s2, c2, msgs) = (s.clone(), cancel.clone(), flatten_text_msgs(&messages));
        std::thread::spawn(move || run_stream(s2, system, msgs, itx, c2));
        while let Ok(e) = irx.recv_blocking() {
            match e {
                StreamEvent::Chunk(c) => send(AgentStreamEvent::Chunk(c)),
                StreamEvent::Done => { send(AgentStreamEvent::Done("end_turn".into())); return; }
                StreamEvent::Error(m) => { send(AgentStreamEvent::Error(m)); return; }
            }
        }
        return;
    }
    if s.api_key.is_empty() { send(AgentStreamEvent::Error("未配置 API Key，请在设置中填写".into())); return; }
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(600))).build().into();
    let base = if s.base_url.is_empty() { "https://api.anthropic.com".to_string() } else { s.base_url.trim_end_matches('/').to_string() };
    let mut body = serde_json::json!({ "model": s.model, "max_tokens": 8192, "stream": true, "system": system, "messages": messages });
    if tools { body["tools"] = crate::tools::defs(); }
    let resp = agent.post(format!("{base}/v1/messages")).header("x-api-key", &s.api_key).header("anthropic-version", "2023-06-01").send_json(body);
    let mut resp = match resp { Ok(r) => r, Err(e) => { send(AgentStreamEvent::Error(e.to_string())); return; } };
    if !resp.status().is_success() {
        let code = resp.status();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        send(AgentStreamEvent::Error(format!("{code}: {}", text.chars().take(300).collect::<String>())));
        return;
    }
    let reader = std::io::BufReader::new(resp.into_body().into_reader());
    let (mut parser, mut stop) = (SseParser::default(), String::from("end_turn"));
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) { return; }
        match line {
            Ok(l) => { for ev in parser.feed(l.trim_end_matches('\r')) { emit(ev, &mut stop); } }
            Err(e) => { send(AgentStreamEvent::Error(e.to_string())); return; }
        }
    }
    send(AgentStreamEvent::Done(stop));
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
    fn mock_带文件时回同路径代码块() {
        let m = vec![Msg { role: "user".into(), content: "改\n\n<file path=\"src/a.rs\">\nfn a() {}\n</file>".into() }];
        let r = mock_reply(&m);
        assert!(r.contains("```rs path=src/a.rs"));
        assert!(r.contains("fn a() {}"));
    }

    #[test]
    fn mock_行内改写回传选中内容() {
        let m = vec![Msg { role: "user".into(), content: "改名\n\n选中的代码：\n```\nlet a = 1\n```".into() }];
        assert_eq!(mock_reply(&m), "```\n// edited by Quill\nlet a = 1\n```");
    }

    #[test]
    fn mock_流式会发出完整内容并结束() {
        let (tx, rx) = async_channel::unbounded();
        let s = Settings::default();
        run_stream(s, String::new(), vec![Msg { role: "user".into(), content: "hi".into() }], tx, Arc::new(AtomicBool::new(false)));
        let mut text = String::new();
        let mut done = false;
        while let Ok(e) = rx.try_recv() { match e { StreamEvent::Chunk(c) => text.push_str(&c), StreamEvent::Done => done = true, StreamEvent::Error(e) => panic!("{e}") } }
        assert!(done);
        assert!(text.contains("Quill"));
    }
}

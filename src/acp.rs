//! ACP（Agent Client Protocol）纯逻辑（#7）：Quill 当 Client，本机 Claude Code 当 Agent，stdio 上逐行 JSON-RPC。
//! 这一层不碰进程和界面，只管「怎么编码请求、怎么解析收到的每一行、工具调用状态怎么合并」。
//! 错了表现为：握手失败、消息不出字、工具卡片状态停在「进行中」、权限卡片点了没反应、读文件把整份内容塞给 Agent。
//! 协议字段对照官方 schema（protocolVersion 整数；sessionUpdate 判别串；权限 outcome 形状），未知变体一律宽容地忽略，
//! 因为适配器版本还在变（v2 草案）。

use serde_json::{json, Value};

pub const PROTOCOL_VERSION: u32 = 1;

// ---------- 编码（Client → Agent）----------

pub fn initialize(id: u64) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
        "protocolVersion": PROTOCOL_VERSION,
        // 读写文件走 Quill（能读到未保存的修改、写入进审阅）；terminal 一期不声明，Agent 用自带的 Bash
        "clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":false},
        "clientInfo":{"name":"quill","title":"Quill","version":env!("CARGO_PKG_VERSION")}
    }}).to_string()
}

pub fn new_session(id: u64, cwd: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}}).to_string()
}

pub fn prompt(id: u64, session_id: &str, text: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{
        "sessionId":session_id,"prompt":[{"type":"text","text":text}]
    }}).to_string()
}

/// 取消是通知（没有 id、没有响应）
pub fn cancel(session_id: &str) -> String {
    json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session_id}}).to_string()
}

pub fn respond(id: &Value, result: Value) -> String {
    json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()
}

pub fn respond_err(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}).to_string()
}

/// 权限请求的响应：选了某个选项，或（None）取消
pub fn permission_result(option_id: Option<&str>) -> Value {
    match option_id {
        Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
        None => json!({"outcome":{"outcome":"cancelled"}}),
    }
}

// ---------- 解码（Agent → Client）----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolKind { Read, Edit, Delete, Move, Search, Execute, Think, Fetch, SwitchMode, Other }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status { Pending, InProgress, Completed, Failed }

#[derive(Debug, Clone, PartialEq)]
pub enum ToolContent {
    Text(String),
    Diff { path: String, old: Option<String>, new: String },
    Terminal(String),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: Status,
    pub content: Vec<ToolContent>,
    pub paths: Vec<String>,
    pub raw_input: Option<Value>,
}

/// `tool_call_update`：只带变化的字段
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolPatch {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<Status>,
    pub content: Option<Vec<ToolContent>>,
    pub paths: Option<Vec<String>>,
    pub raw_input: Option<Value>,
}

impl ToolCall {
    pub fn apply(&mut self, p: &ToolPatch) {
        if let Some(v) = &p.title { self.title = v.clone(); }
        if let Some(v) = p.kind { self.kind = v; }
        if let Some(v) = p.status { self.status = v; }
        if let Some(v) = &p.content { self.content = v.clone(); }
        if let Some(v) = &p.paths { self.paths = v.clone(); }
        if let Some(v) = &p.raw_input { self.raw_input = Some(v.clone()); }
    }
    pub fn from_patch(p: &ToolPatch) -> Self {
        let mut t = ToolCall { id: p.id.clone(), title: String::new(), kind: ToolKind::Other, status: Status::Pending, content: vec![], paths: vec![], raw_input: None };
        t.apply(p);
        t
    }
    pub fn is_done(&self) -> bool { matches!(self.status, Status::Completed | Status::Failed) }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanEntry { pub text: String, pub status: Status }

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Text(String),
    Thought(String),
    ToolCall(ToolCall),
    ToolUpdate(ToolPatch),
    Plan(Vec<PlanEntry>),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermKind { AllowOnce, AllowAlways, RejectOnce, RejectAlways }

#[derive(Debug, Clone, PartialEq)]
pub struct PermOption { pub id: String, pub name: String, pub kind: PermKind }

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Permission { tool: ToolPatch, options: Vec<PermOption> },
    ReadFile { path: String, line: Option<usize>, limit: Option<usize> },
    WriteFile { path: String, content: String },
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 对我们某个请求的响应（id 是我们发的数字）
    Response { id: u64, result: Result<Value, String> },
    Update(Update),
    /// Agent 发来的请求，需要回应；id 原样带回（可能是数字也可能是字符串）
    Request { id: Value, req: Request },
    Ignore,
}

fn s(v: &Value, k: &str) -> Option<String> { v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string()) }

fn kind_of(v: &str) -> ToolKind {
    match v {
        "read" => ToolKind::Read, "edit" => ToolKind::Edit, "delete" => ToolKind::Delete, "move" => ToolKind::Move,
        "search" => ToolKind::Search, "execute" => ToolKind::Execute, "think" => ToolKind::Think,
        "fetch" => ToolKind::Fetch, "switch_mode" => ToolKind::SwitchMode, _ => ToolKind::Other,
    }
}

fn status_of(v: &str) -> Status {
    match v { "in_progress" => Status::InProgress, "completed" => Status::Completed, "failed" => Status::Failed, _ => Status::Pending }
}

fn content_of(v: &Value) -> ToolContent {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("content") => match v.pointer("/content/text").and_then(|t| t.as_str()) {
            Some(t) => ToolContent::Text(t.to_string()),
            None => ToolContent::Other,
        },
        Some("diff") => ToolContent::Diff {
            path: s(v, "path").unwrap_or_default(),
            old: s(v, "oldText"),
            new: s(v, "newText").unwrap_or_default(),
        },
        Some("terminal") => ToolContent::Terminal(s(v, "terminalId").unwrap_or_default()),
        _ => ToolContent::Other,
    }
}

fn patch_of(v: &Value) -> ToolPatch {
    ToolPatch {
        id: s(v, "toolCallId").unwrap_or_default(),
        title: s(v, "title"),
        kind: v.get("kind").and_then(|k| k.as_str()).map(kind_of),
        status: v.get("status").and_then(|k| k.as_str()).map(status_of),
        content: v.get("content").and_then(|c| c.as_array()).map(|a| a.iter().map(content_of).collect()),
        paths: v.get("locations").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|l| s(l, "path")).collect()),
        raw_input: v.get("rawInput").filter(|x| !x.is_null()).cloned(),
    }
}

fn text_chunk(u: &Value) -> Option<String> {
    let c = u.get("content")?;
    if c.get("type").and_then(|t| t.as_str()) != Some("text") { return None; }
    s(c, "text")
}

fn update_of(u: &Value) -> Update {
    match u.get("sessionUpdate").and_then(|x| x.as_str()) {
        Some("agent_message_chunk") => text_chunk(u).map(Update::Text).unwrap_or(Update::Other),
        Some("agent_thought_chunk") => text_chunk(u).map(Update::Thought).unwrap_or(Update::Other),
        Some("tool_call") => Update::ToolCall(ToolCall::from_patch(&patch_of(u))),
        Some("tool_call_update") => Update::ToolUpdate(patch_of(u)),
        Some("plan") => Update::Plan(
            u.get("entries").and_then(|e| e.as_array()).map(|a| a.iter().map(|e| PlanEntry {
                text: s(e, "content").unwrap_or_default(),
                status: status_of(e.get("status").and_then(|x| x.as_str()).unwrap_or("pending")),
            }).collect()).unwrap_or_default()),
        _ => Update::Other,
    }
}

fn perm_kind(k: &str) -> PermKind {
    match k { "allow_always" => PermKind::AllowAlways, "reject_once" => PermKind::RejectOnce, "reject_always" => PermKind::RejectAlways, _ => PermKind::AllowOnce }
}

fn request_of(method: &str, p: &Value) -> Request {
    match method {
        "session/request_permission" => Request::Permission {
            tool: p.get("toolCall").map(patch_of).unwrap_or_default(),
            options: p.get("options").and_then(|o| o.as_array()).map(|a| a.iter().map(|o| PermOption {
                id: s(o, "optionId").unwrap_or_default(),
                name: s(o, "name").unwrap_or_default(),
                kind: perm_kind(o.get("kind").and_then(|k| k.as_str()).unwrap_or("allow_once")),
            }).collect()).unwrap_or_default(),
        },
        "fs/read_text_file" => Request::ReadFile {
            path: s(p, "path").unwrap_or_default(),
            line: p.get("line").and_then(|x| x.as_u64()).map(|x| x as usize),
            limit: p.get("limit").and_then(|x| x.as_u64()).map(|x| x as usize),
        },
        "fs/write_text_file" => Request::WriteFile { path: s(p, "path").unwrap_or_default(), content: s(p, "content").unwrap_or_default() },
        other => Request::Unknown(other.to_string()),
    }
}

/// 解析 Agent 发来的一行。不是合法 JSON / 不认识的形状一律 Ignore，不报错——协议还在演进。
pub fn parse_line(line: &str) -> Incoming {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { return Incoming::Ignore };
    let method = v.get("method").and_then(|m| m.as_str());
    let id = v.get("id").filter(|i| !i.is_null()).cloned();
    match (method, id) {
        (Some("session/update"), None) => {
            let u = v.pointer("/params/update").map(update_of).unwrap_or(Update::Other);
            Incoming::Update(u)
        }
        (Some(m), Some(id)) => Incoming::Request { id, req: request_of(m, v.get("params").unwrap_or(&Value::Null)) },
        (None, Some(id)) => {
            let Some(n) = id.as_u64() else { return Incoming::Ignore };
            let result = match v.get("error") {
                Some(e) => Err(s(e, "message").unwrap_or_else(|| e.to_string())),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            Incoming::Response { id: n, result }
        }
        _ => Incoming::Ignore,
    }
}

// ---------- 小工具 ----------

pub enum Choice { Allow, AllowAlways, Reject }

/// 在 Agent 给的选项里挑出符合用户意图的那个；没有精确对应的种类时退到最接近的，实在没有返回 None（按取消处理）
pub fn pick_option(options: &[PermOption], c: Choice) -> Option<&PermOption> {
    let find = |k: PermKind| options.iter().find(|o| o.kind == k);
    match c {
        Choice::Allow => find(PermKind::AllowOnce).or_else(|| find(PermKind::AllowAlways)),
        Choice::AllowAlways => find(PermKind::AllowAlways).or_else(|| find(PermKind::AllowOnce)),
        Choice::Reject => find(PermKind::RejectOnce).or_else(|| find(PermKind::RejectAlways)),
    }
}

/// `fs/read_text_file` 的 line（1 起）/ limit：只返回请求的那一段，而不是整份文件
pub fn slice_lines(content: &str, line: Option<usize>, limit: Option<usize>) -> String {
    if line.is_none() && limit.is_none() { return content.to_string(); }
    let start = line.unwrap_or(1).max(1) - 1;
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let end = limit.map(|l| (start + l).min(lines.len())).unwrap_or(lines.len());
    if start >= lines.len() { return String::new(); }
    lines[start..end].concat()
}

/// 一次回合结束的原因，转成给用户看的话；正常结束返回 None
pub fn stop_note(reason: &str) -> Option<&'static str> {
    match reason {
        "end_turn" => None,
        "cancelled" => Some("已取消"),
        "max_tokens" => Some("输出达到长度上限，回答被截断"),
        "max_turn_requests" => Some("本轮工具调用次数达到上限，已停止"),
        "refusal" => Some("模型拒绝了这个请求"),
        _ => Some("回合已结束"),
    }
}

// ---------- 投影成面板里的工具卡片 ----------

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Read => "read", ToolKind::Edit => "edit", ToolKind::Delete => "delete", ToolKind::Move => "move",
            ToolKind::Search => "search", ToolKind::Execute => "execute", ToolKind::Think => "think", ToolKind::Fetch => "fetch",
            ToolKind::SwitchMode => "switch_mode", ToolKind::Other => "other",
        }
    }
}

/// 工具输出文字上限（字符）。取末尾：命令输出的结论通常在最后
const MAX_RESULT_CHARS: usize = 6000;

fn tail_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max { return s.to_string(); }
    let skip = n - max;
    let cut = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    format!("…{}", &s[cut..])
}

/// 把一次工具调用投影成面板的 (名称, 输入, 结果)：名称统一带 `acp:` 前缀 + 种类；
/// 结果在进行中是 None，完成是正文（可能为空），失败是「错误：…」。
pub fn project(tc: &ToolCall) -> (String, Value, Option<String>) {
    let body = tc.content.iter().filter_map(|c| match c {
        ToolContent::Text(t) if !t.trim().is_empty() => Some(t.clone()),
        ToolContent::Diff { path, .. } => Some(format!("修改 {path}")),
        _ => None,
    }).collect::<Vec<_>>().join("\n");
    let body = tail_chars(&body, MAX_RESULT_CHARS);
    let result = match tc.status {
        Status::Pending | Status::InProgress => None,
        Status::Completed => Some(body),
        Status::Failed => Some(format!("错误：{}", if body.trim().is_empty() { "失败".to_string() } else { body })),
    };
    (format!("acp:{}", tc.kind.as_str()), json!({"title": tc.title, "paths": tc.paths, "raw": tc.raw_input}), result)
}

/// 计划（TODO）：作为一条特殊的「工具调用」展示；全部完成后才算有结果
pub fn project_plan(entries: &[PlanEntry]) -> (String, Value, Option<String>) {
    let list: Vec<Value> = entries.iter().map(|e| json!({"text": e.text, "status": match e.status {
        Status::Pending => "pending", Status::InProgress => "in_progress", Status::Completed => "completed", Status::Failed => "failed" }})).collect();
    let done = !entries.is_empty() && entries.iter().all(|e| matches!(e.status, Status::Completed));
    ("acp:plan".to_string(), json!({"entries": list}), if done { Some(String::new()) } else { None })
}

/// 绝对路径 → 相对项目根的路径；不在项目内返回 None
pub fn rel_path(root: &std::path::Path, abs: &str) -> Option<String> {
    let rel = std::path::Path::new(abs).strip_prefix(root).ok()?;
    let s = rel.to_string_lossy().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value { serde_json::from_str(s).unwrap() }

    #[test]
    fn 握手请求带协议版本和文件能力() {
        let m = v(&initialize(1));
        assert_eq!(m["method"], "initialize");
        assert_eq!(m["id"], 1);
        assert_eq!(m["params"]["protocolVersion"], 1);
        assert_eq!(m["params"]["clientCapabilities"]["fs"]["readTextFile"], true);
        assert_eq!(m["params"]["clientCapabilities"]["fs"]["writeTextFile"], true);
        assert_eq!(m["params"]["clientCapabilities"]["terminal"], false);
        assert_eq!(m["jsonrpc"], "2.0");
    }

    #[test]
    fn 编码的请求是单行() {
        for line in [initialize(1), new_session(2, "/p"), prompt(3, "s", "你好\n第二行"), cancel("s")] {
            assert!(!line.contains('\n'), "{line}");
        }
        let m = v(&prompt(3, "sess-1", "你好\n第二行"));
        assert_eq!(m["params"]["sessionId"], "sess-1");
        assert_eq!(m["params"]["prompt"][0]["type"], "text");
        assert_eq!(m["params"]["prompt"][0]["text"], "你好\n第二行");
        assert_eq!(v(&new_session(2, "/proj"))["params"]["cwd"], "/proj");
    }

    #[test]
    fn 取消是没有_id_的通知() {
        let m = v(&cancel("s1"));
        assert_eq!(m["method"], "session/cancel");
        assert!(m.get("id").is_none());
        assert_eq!(m["params"]["sessionId"], "s1");
    }

    #[test]
    fn 权限响应的形状() {
        assert_eq!(permission_result(Some("allow")), v(r#"{"outcome":{"outcome":"selected","optionId":"allow"}}"#));
        assert_eq!(permission_result(None), v(r#"{"outcome":{"outcome":"cancelled"}}"#));
        // id 原样带回，字符串 id 不被改成数字
        let r = v(&respond(&Value::String("abc".into()), json!({})));
        assert_eq!(r["id"], "abc");
        assert_eq!(v(&respond_err(&json!(7), -32601, "nope"))["error"]["code"], -32601);
    }

    #[test]
    fn 解析响应_成功与错误() {
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}"#),
            Incoming::Response { id: 2, result: Ok(json!({"sessionId":"s1"})) });
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"需要登录"}}"#),
            Incoming::Response { id: 2, result: Err("需要登录".into()) });
    }

    #[test]
    fn 解析文字与思考块() {
        let t = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"你好"}}}}"#);
        assert_eq!(t, Incoming::Update(Update::Text("你好".into())));
        let t = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"想想"}}}}"#);
        assert_eq!(t, Incoming::Update(Update::Thought("想想".into())));
        // 非文字内容块（图片等）不当文字
        let t = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"x","mimeType":"image/png"}}}}"#);
        assert_eq!(t, Incoming::Update(Update::Other));
    }

    #[test]
    fn 解析工具调用与更新_并合并() {
        let call = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"Read src/a.rs","kind":"read","status":"pending","locations":[{"path":"/p/src/a.rs","line":3}],"rawInput":{"file_path":"/p/src/a.rs"}}}}"#);
        let Incoming::Update(Update::ToolCall(mut t)) = call else { panic!("{call:?}") };
        assert_eq!((t.id.as_str(), t.title.as_str(), t.kind, t.status), ("t1", "Read src/a.rs", ToolKind::Read, Status::Pending));
        assert_eq!(t.paths, vec!["/p/src/a.rs"]);
        assert!(t.raw_input.is_some());

        // 更新只带 status 和 content：其余字段保持原值
        let up = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"fn main() {}"}}]}}}"#);
        let Incoming::Update(Update::ToolUpdate(p)) = up else { panic!("{up:?}") };
        assert_eq!(p.title, None);
        assert!(!t.is_done());
        t.apply(&p);
        assert!(t.is_done());
        assert_eq!(t.title, "Read src/a.rs");
        assert_eq!(t.paths, vec!["/p/src/a.rs"]);
        assert_eq!(t.content, vec![ToolContent::Text("fn main() {}".into())]);
    }

    #[test]
    fn 解析_diff_内容() {
        let up = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"t2","status":"in_progress","kind":"edit","content":[{"type":"diff","path":"/p/a.rs","oldText":"a","newText":"b"},{"type":"diff","path":"/p/new.rs","oldText":null,"newText":"x"}]}}}"#);
        let Incoming::Update(Update::ToolUpdate(p)) = up else { panic!() };
        assert_eq!(p.kind, Some(ToolKind::Edit));
        assert_eq!(p.content.unwrap(), vec![
            ToolContent::Diff { path: "/p/a.rs".into(), old: Some("a".into()), new: "b".into() },
            ToolContent::Diff { path: "/p/new.rs".into(), old: None, new: "x".into() },
        ]);
    }

    #[test]
    fn 解析计划() {
        let p = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"plan","entries":[{"content":"读文件","priority":"high","status":"completed"},{"content":"改文件","priority":"medium","status":"in_progress"},{"content":"测试","priority":"low","status":"pending"}]}}}"#);
        assert_eq!(p, Incoming::Update(Update::Plan(vec![
            PlanEntry { text: "读文件".into(), status: Status::Completed },
            PlanEntry { text: "改文件".into(), status: Status::InProgress },
            PlanEntry { text: "测试".into(), status: Status::Pending },
        ])));
    }

    #[test]
    fn 未知变体和坏数据宽容忽略() {
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"usage_update","used":1,"size":2}}}"#), Incoming::Update(Update::Other));
        assert_eq!(parse_line("not json"), Incoming::Ignore);
        assert_eq!(parse_line(""), Incoming::Ignore);
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0"}"#), Incoming::Ignore);
        // 没有 params 的 session/update 不崩
        assert_eq!(parse_line(r#"{"jsonrpc":"2.0","method":"session/update"}"#), Incoming::Update(Update::Other));
    }

    #[test]
    fn 解析权限请求() {
        let r = parse_line(r#"{"jsonrpc":"2.0","id":"p-1","method":"session/request_permission","params":{"sessionId":"s","toolCall":{"toolCallId":"t3","title":"Run `ls`","kind":"execute","status":"pending","rawInput":{"command":"ls"}},"options":[{"optionId":"a","name":"Allow always","kind":"allow_always"},{"optionId":"o","name":"Allow","kind":"allow_once"},{"optionId":"r","name":"Reject","kind":"reject_once"}]}}"#);
        let Incoming::Request { id, req: Request::Permission { tool, options } } = r else { panic!("{r:?}") };
        assert_eq!(id, json!("p-1"));
        assert_eq!(tool.title.as_deref(), Some("Run `ls`"));
        assert_eq!(tool.kind, Some(ToolKind::Execute));
        assert_eq!(options.len(), 3);
        assert_eq!(pick_option(&options, Choice::Allow).unwrap().id, "o");
        assert_eq!(pick_option(&options, Choice::AllowAlways).unwrap().id, "a");
        assert_eq!(pick_option(&options, Choice::Reject).unwrap().id, "r");
    }

    #[test]
    fn 选项缺种类时退到最接近的() {
        let only_always = vec![PermOption { id: "a".into(), name: "".into(), kind: PermKind::AllowAlways }];
        assert_eq!(pick_option(&only_always, Choice::Allow).unwrap().id, "a");
        assert!(pick_option(&only_always, Choice::Reject).is_none()); // 没有拒绝项 → 调用方按取消处理
        let only_once = vec![PermOption { id: "o".into(), name: "".into(), kind: PermKind::AllowOnce }];
        assert_eq!(pick_option(&only_once, Choice::AllowAlways).unwrap().id, "o");
        let rej_always = vec![PermOption { id: "x".into(), name: "".into(), kind: PermKind::RejectAlways }];
        assert_eq!(pick_option(&rej_always, Choice::Reject).unwrap().id, "x");
        assert!(pick_option(&[], Choice::Allow).is_none());
    }

    #[test]
    fn 解析文件读写请求() {
        let r = parse_line(r#"{"jsonrpc":"2.0","id":5,"method":"fs/read_text_file","params":{"sessionId":"s","path":"/p/a.rs","line":2,"limit":3}}"#);
        assert_eq!(r, Incoming::Request { id: json!(5), req: Request::ReadFile { path: "/p/a.rs".into(), line: Some(2), limit: Some(3) } });
        let r = parse_line(r#"{"jsonrpc":"2.0","id":6,"method":"fs/write_text_file","params":{"sessionId":"s","path":"/p/a.rs","content":"x"}}"#);
        assert_eq!(r, Incoming::Request { id: json!(6), req: Request::WriteFile { path: "/p/a.rs".into(), content: "x".into() } });
        // 不认识的请求也要带 id 回来，调用方才能回「不支持」而不是让 Agent 干等
        let r = parse_line(r#"{"jsonrpc":"2.0","id":9,"method":"terminal/create","params":{}}"#);
        assert_eq!(r, Incoming::Request { id: json!(9), req: Request::Unknown("terminal/create".into()) });
    }

    #[test]
    fn 按行读取切片() {
        let c = "a\nb\nc\nd\n";
        assert_eq!(slice_lines(c, None, None), c);
        assert_eq!(slice_lines(c, Some(2), None), "b\nc\nd\n");
        assert_eq!(slice_lines(c, Some(2), Some(2)), "b\nc\n");
        assert_eq!(slice_lines(c, None, Some(1)), "a\n");
        assert_eq!(slice_lines(c, Some(3), Some(100)), "c\nd\n"); // limit 超出文件末尾
        assert_eq!(slice_lines(c, Some(99), Some(1)), "");
        assert_eq!(slice_lines(c, Some(0), Some(1)), "a\n"); // 0 当 1
        assert_eq!(slice_lines("x", Some(1), Some(1)), "x"); // 没有结尾换行
    }

    #[test]
    fn 回合结束原因() {
        assert_eq!(stop_note("end_turn"), None);
        assert_eq!(stop_note("cancelled"), Some("已取消"));
        assert!(stop_note("max_tokens").unwrap().contains("截断"));
        assert!(stop_note("something_new").is_some());
    }

    #[test]
    fn 工具调用投影_进行中无结果_完成有正文_失败带错误前缀() {
        let mut tc = ToolCall { id: "t".into(), title: "Run `ls`".into(), kind: ToolKind::Execute, status: Status::InProgress, content: vec![], paths: vec![], raw_input: Some(json!({"command":"ls"})) };
        let (name, input, res) = project(&tc);
        assert_eq!(name, "acp:execute");
        assert_eq!(input["title"], "Run `ls`");
        assert_eq!(input["raw"]["command"], "ls");
        assert_eq!(res, None);
        tc.status = Status::Completed;
        tc.content = vec![ToolContent::Text("a\nb".into()), ToolContent::Other, ToolContent::Text("  ".into())];
        assert_eq!(project(&tc).2, Some("a\nb".into()));
        tc.status = Status::Failed;
        tc.content = vec![];
        assert_eq!(project(&tc).2, Some("错误：失败".into()));
        tc.content = vec![ToolContent::Text("用户拒绝".into())];
        assert_eq!(project(&tc).2, Some("错误：用户拒绝".into()));
    }

    #[test]
    fn 工具输出过长取末尾() {
        let tc = ToolCall { id: "t".into(), title: "".into(), kind: ToolKind::Execute, status: Status::Completed,
            content: vec![ToolContent::Text(format!("{}END", "x".repeat(10_000)))], paths: vec![], raw_input: None };
        let r = project(&tc).2.unwrap();
        assert!(r.chars().count() <= 6001 + 1);
        assert!(r.starts_with('…') && r.ends_with("END"));
        // 多字节字符不会被切坏
        let tc2 = ToolCall { content: vec![ToolContent::Text("中".repeat(9000))], ..tc };
        assert!(project(&tc2).2.unwrap().chars().all(|c| c == '中' || c == '…'));
    }

    #[test]
    fn 计划投影_全部完成才有结果() {
        let es = vec![PlanEntry { text: "a".into(), status: Status::Completed }, PlanEntry { text: "b".into(), status: Status::InProgress }];
        let (name, input, res) = project_plan(&es);
        assert_eq!(name, "acp:plan");
        assert_eq!(input["entries"][1]["status"], "in_progress");
        assert_eq!(res, None);
        let all = vec![PlanEntry { text: "a".into(), status: Status::Completed }];
        assert_eq!(project_plan(&all).2, Some(String::new()));
        assert_eq!(project_plan(&[]).2, None);
    }

    #[test]
    fn 绝对路径换相对路径() {
        let root = std::path::Path::new("/p/proj");
        assert_eq!(rel_path(root, "/p/proj/src/a.rs"), Some("src/a.rs".into()));
        assert_eq!(rel_path(root, "/p/other/a.rs"), None);
        assert_eq!(rel_path(root, "/p/proj"), None);
        assert_eq!(rel_path(root, "/p/project/a.rs"), None); // 前缀相同但不是子目录
        assert_eq!(rel_path(root, "relative/a.rs"), None);
    }
}

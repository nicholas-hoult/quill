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
        // 读写文件走 Quill（能读到未保存的修改、写入进审阅）；terminal：Agent 的命令由 Quill 起进程，输出实时显示在卡片里（对标 Zed）
        "clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true},
        "clientInfo":{"name":"quill","title":"Quill","version":env!("CARGO_PKG_VERSION")}
    }}).to_string()
}

pub fn new_session(id: u64, cwd: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}}).to_string()
}

/// 恢复之前的会话（Agent 声明了 loadSession 才能用）。Agent 会把历史重放成 session/update，Client 要忽略它们
pub fn load_session(id: u64, session_id: &str, cwd: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"session/load","params":{"sessionId":session_id,"cwd":cwd,"mcpServers":[]}}).to_string()
}

/// `initialize` 响应里的 agentCapabilities.loadSession
pub fn supports_load(init_result: &Value) -> bool {
    init_result["agentCapabilities"]["loadSession"].as_bool() == Some(true)
}

/// 切换会话模式（`session/set_mode`）；响应是空对象
pub fn set_mode(id: u64, session_id: &str, mode_id: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"method":"session/set_mode","params":{"sessionId":session_id,"modeId":mode_id}}).to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mode { pub id: String, pub name: String, pub description: String }

/// 会话模式（#9）：由 Agent 在 `session/new` / `session/load` 的结果里声明，Quill 不写死列表
#[derive(Debug, Clone, PartialEq)]
pub struct Modes { pub current: String, pub available: Vec<Mode> }

/// 从 `session/new` / `session/load` 的结果里取模式；没有或列表为空就是 None（界面不显示选择器）
pub fn parse_modes(result: &Value) -> Option<Modes> {
    let m = result.get("modes")?;
    let available: Vec<Mode> = m.get("availableModes")?.as_array()?.iter().filter_map(|x| Some(Mode {
        id: s(x, "id")?,
        name: s(x, "name").unwrap_or_default(),
        description: s(x, "description").unwrap_or_default(),
    })).collect();
    if available.is_empty() { return None; }
    Some(Modes { current: s(m, "currentModeId").unwrap_or_default(), available })
}

/// 模式的显示名：已知 id 用中文，未知的用 Agent 给的名字
pub fn mode_label(id: &str, name: &str) -> String {
    match id {
        "default" => "手动确认", "acceptEdits" => "自动接受编辑", "plan" => "计划", "auto" => "自动", "bypassPermissions" => "跳过所有确认",
        _ => return name.to_string(),
    }.to_string()
}

/// 模式的说明：已知 id 用中文，未知的用 Agent 给的原文
pub fn mode_hint(id: &str, description: &str) -> String {
    match id {
        "default" => "每次修改前都询问", "acceptEdits" => "自动接受所有文件编辑", "plan" => "先出计划，不改文件",
        "auto" => "由 Claude 判断哪些操作需要确认", "bypassPermissions" => "全部放行，不再询问",
        _ => return description.to_string(),
    }.to_string()
}

/// Shift+Tab 的下一个模式：按 Agent 给的顺序循环，**跳过「跳过所有确认」**（误触代价太大，只能从菜单选）
pub fn cycle_next(m: &Modes) -> Option<String> {
    let ring: Vec<&Mode> = m.available.iter().filter(|x| x.id != "bypassPermissions").collect();
    if ring.len() < 2 { return None; }
    let next = match ring.iter().position(|x| x.id == m.current) { Some(i) => ring[(i + 1) % ring.len()], None => ring[0] };
    Some(next.id.clone())
}

/// 哪些模式值得跨会话记住：计划是「这件事先别动手」、跳过所有确认太危险，都只在当前会话有效
pub fn persistable_mode(id: &str) -> bool { matches!(id, "default" | "acceptEdits" | "auto") }

/// 会话建好后要不要补发 `session/set_mode`：记住的模式存在、可记、Agent 支持、且和当前不同
pub fn mode_to_apply(pref: &str, m: &Modes) -> Option<String> {
    if !persistable_mode(pref) || pref == m.current || !m.available.iter().any(|x| x.id == pref) { return None; }
    Some(pref.to_string())
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
    /// Agent 自己切换了会话模式（如退出计划模式）：新的 modeId（#9）
    Mode(String),
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
    TermCreate { command: String, args: Vec<String>, env: Vec<(String, String)>, cwd: Option<String>, limit: Option<usize> },
    TermOutput { id: String },
    TermWait { id: String },
    TermKill { id: String },
    TermRelease { id: String },
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
        Some("current_mode_update") => s(u, "currentModeId").map(Update::Mode).unwrap_or(Update::Other),
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
        "terminal/create" => Request::TermCreate {
            command: s(p, "command").unwrap_or_default(),
            args: p.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            env: p.get("env").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|e| Some((s(e, "name")?, s(e, "value").unwrap_or_default()))).collect()).unwrap_or_default(),
            cwd: s(p, "cwd"),
            limit: p.get("outputByteLimit").and_then(|x| x.as_u64()).map(|x| x as usize),
        },
        "terminal/output" => Request::TermOutput { id: s(p, "terminalId").unwrap_or_default() },
        "terminal/wait_for_exit" => Request::TermWait { id: s(p, "terminalId").unwrap_or_default() },
        "terminal/kill" => Request::TermKill { id: s(p, "terminalId").unwrap_or_default() },
        "terminal/release" => Request::TermRelease { id: s(p, "terminalId").unwrap_or_default() },
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

/// `fs/read_text_file` 的 line（1 起）/ limit：只返回请求的那一段，而不是整份文件。
/// 对标 Zed：起始行超出文件末尾是错误（行数 = 换行数 + 1，文件末尾的空行也算一行）。
pub fn slice_lines(content: &str, line: Option<usize>, limit: Option<usize>) -> Result<String, String> {
    let start = line.unwrap_or(1).max(1) - 1;
    let rows = content.matches('\n').count() + 1;
    if start >= rows { return Err(format!("起始行超出文件末尾（文件共 {rows} 行，请求第 {} 行）", start + 1)); }
    if line.is_none() && limit.is_none() { return Ok(content.to_string()); }
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let end = limit.map(|l| (start + l).min(lines.len())).unwrap_or(lines.len());
    if start >= lines.len() { return Ok(String::new()); } // 恰好是末尾那个空行
    Ok(lines[start..end].concat())
}

/// 等待用户确认的权限请求队列。Agent 可能并行调用工具，同时发来多个请求：
/// 只有一个槽位的话后来的会把前面的覆盖掉，被覆盖的那个永远得不到回应，Agent 就一直卡住。
/// 对标 Zed：按请求排队；同一个工具调用再来一个请求，旧的作废（调用方要对旧的回「取消」）。
pub struct PermQueue<T> { items: std::collections::VecDeque<(String, T)> }

impl<T> Default for PermQueue<T> { fn default() -> Self { Self { items: Default::default() } } }

impl<T> PermQueue<T> {
    /// 入队；如果同一工具调用已有排队的请求，把旧的取出来还给调用方
    pub fn push(&mut self, tool_id: &str, item: T) -> Option<T> {
        let old = self.items.iter().position(|(id, _)| id == tool_id).and_then(|i| self.items.remove(i)).map(|(_, t)| t);
        self.items.push_back((tool_id.to_string(), item));
        old
    }
    pub fn front(&self) -> Option<&T> { self.items.front().map(|(_, t)| t) }
    pub fn pop_front(&mut self) -> Option<T> { self.items.pop_front().map(|(_, t)| t) }
    /// 取出全部（停止 / 出错时统一回应「取消」）
    pub fn drain(&mut self) -> Vec<T> { self.items.drain(..).map(|(_, t)| t).collect() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
    pub fn len(&self) -> usize { self.items.len() }
    /// 是不是排在最前面的那个（界面上只给它显示确认按钮）
    pub fn is_front(&self, f: impl Fn(&T) -> bool) -> bool { self.front().map(f).unwrap_or(false) }
    pub fn contains(&self, f: impl Fn(&T) -> bool) -> bool { self.items.iter().any(|(_, t)| f(t)) }
}

/// 有没有使用 Claude Code 的意图（#8）：输入框开始有字，或聊天面板开着。
/// 只点一下输入框、切换对话不算——它们很频繁，每次起一个 node + claude 太浪费；面板关着也不起
pub fn prewarm_intent(composer_empty: bool, panel_open: bool) -> bool { !composer_empty || panel_open }

/// 是否该提前把 Agent 拉起来（#8）：启动进程 + 握手 + 建会话实测要好几秒（建会话 4–5s 是 Claude Code 自己的启动），
/// 等用户按下发送再起会让他干等「思考中…」
pub fn should_prewarm(provider: &str, has_agent: bool, has_root: bool, busy: bool, intent: bool) -> bool {
    provider == "claude-code" && !has_agent && has_root && !busy && intent
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
        ToolContent::Text(t) if !t.trim().is_empty() => Some(strip_fence(t)),
        ToolContent::Diff { path, .. } => Some(format!("修改 {path}")),
        _ => None,
    }).collect::<Vec<_>>().join("\n");
    let body = tail_chars(&body, MAX_RESULT_CHARS);
    let result = match tc.status {
        Status::Pending | Status::InProgress => None,
        Status::Completed => Some(body),
        Status::Failed => Some(format!("错误：{}", if body.trim().is_empty() { "失败".to_string() } else { body })),
    };
    let diffs: Vec<Value> = tc.content.iter().filter_map(|c| match c {
        ToolContent::Diff { path, old, new } => Some(json!({"path": path, "preview": diff_preview(old.as_deref().unwrap_or(""), new, PREVIEW_LINES)
            .into_iter().map(|(m, l)| json!([m.to_string(), l])).collect::<Vec<_>>()})),
        _ => None,
    }).collect();
    let terminal = tc.content.iter().find_map(|c| if let ToolContent::Terminal(id) = c { Some(id.clone()) } else { None });
    (format!("acp:{}", tc.kind.as_str()), json!({"title": tc.title, "paths": tc.paths, "raw": tc.raw_input, "diffs": diffs, "terminal": terminal}), result)
}

/// 计划（TODO）：作为一条特殊的「工具调用」展示；全部完成后才算有结果
pub fn project_plan(entries: &[PlanEntry]) -> (String, Value, Option<String>) {
    let list: Vec<Value> = entries.iter().map(|e| json!({"text": e.text, "status": match e.status {
        Status::Pending => "pending", Status::InProgress => "in_progress", Status::Completed => "completed", Status::Failed => "failed" }})).collect();
    let done = !entries.is_empty() && entries.iter().all(|e| matches!(e.status, Status::Completed));
    ("acp:plan".to_string(), json!({"entries": list}), if done { Some(String::new()) } else { None })
}

/// Claude Code 把命令输出包在代码围栏里（```console … ```），在卡片里显示时去掉围栏
pub fn strip_fence(text: &str) -> String {
    let t = text.trim_matches('\n');
    let mut lines: Vec<&str> = t.lines().collect();
    if lines.len() >= 2 && lines[0].trim_start().starts_with("```") && lines[lines.len() - 1].trim() == "```" {
        lines.remove(0);
        lines.pop();
        return lines.join("\n");
    }
    text.to_string()
}

/// 卡片里 diff 预览最多显示的行数（删除 + 新增合计）
pub const PREVIEW_LINES: usize = 14;

/// 新旧文本的差异预览：去掉相同的开头和结尾行，只留中间不同的部分——先是删除（'-'），再是新增（'+'），
/// 超过 `max` 行时截断并补一行 '…'。权限卡片里用它让用户在批准前看到「要改什么」。
pub fn diff_preview(old: &str, new: &str, max: usize) -> Vec<(char, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let pre = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let removed = &a[pre..a.len() - suf];
    let added = &b[pre..b.len() - suf];
    let mut out: Vec<(char, String)> = removed.iter().map(|l| ('-', l.to_string())).chain(added.iter().map(|l| ('+', l.to_string()))).collect();
    if out.len() > max { out.truncate(max); out.push(('…', String::new())); }
    out
}

/// Agent 改完文件（工具完成）后要进入审阅的文件：(相对路径, 改动前的内容, 改动后的内容)。
/// 只取项目内的；`old` 为 None 表示新建的文件。对标 Zed：改动已经在磁盘上，审阅是事后的，拒绝时回写。
pub fn review_targets(tc: &ToolCall, root: &std::path::Path) -> Vec<(String, Option<String>, String)> {
    tc.content.iter().filter_map(|c| match c {
        ToolContent::Diff { path, old, new } => Some((rel_path(root, path)?, old.clone(), new.clone())),
        _ => None,
    }).collect()
}

/// Agent 的 diff 与磁盘内容对账：真实适配器给的整文件 diff 常常少了结尾换行，而磁盘上有。
/// 忽略结尾换行比较「磁盘 == diff 的新文本」，不相等（片段级 diff / 之后又被改过）返回 None；
/// 相等时返回补齐结尾换行的改动前内容（磁盘以换行结尾而 before 没有时补上，避免审阅里多出一个假改动）。
pub fn reconcile_before(before: Option<&str>, after: &str, disk: &str) -> Option<String> {
    if disk.trim_end_matches('\n') != after.trim_end_matches('\n') { return None; }
    let mut b = before.unwrap_or("").to_string();
    if !b.is_empty() && disk.ends_with('\n') && !b.ends_with('\n') { b.push('\n'); }
    Some(b)
}

// ---------- 终端（Agent 的命令由 Quill 起进程）----------

/// 命令输出缓冲：超过上限时从**开头**丢（ACP 约定），在字符边界上截断；
/// 管道一次读到的字节可能把一个多字节字符切成两半，所以不完整的结尾先留着，下次凑齐了再转。
pub struct OutputBuf { text: String, limit: usize, truncated: bool, pending: Vec<u8> }

/// 没指定 outputByteLimit 时的上限
pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;

impl OutputBuf {
    pub fn new(limit: Option<usize>) -> Self { Self { text: String::new(), limit: limit.unwrap_or(DEFAULT_OUTPUT_LIMIT).max(1), truncated: false, pending: vec![] } }

    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        // 取出能完整转成 UTF-8 的前缀；结尾不完整的字符留到下次
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(), // 只是结尾被切断
            Err(_) => self.pending.len(),                          // 真的有非法字节：整体宽容转换
        };
        let chunk: Vec<u8> = self.pending.drain(..valid).collect();
        self.text.push_str(&String::from_utf8_lossy(&chunk));
        if self.text.len() > self.limit {
            let mut cut = self.text.len() - self.limit;
            while !self.text.is_char_boundary(cut) { cut += 1; }
            self.text.drain(..cut);
            self.truncated = true;
        }
    }

    pub fn text(&self) -> &str { &self.text }
    pub fn truncated(&self) -> bool { self.truncated }
}

/// 进程退出状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Exit { pub code: Option<i32>, pub signal: Option<i32> }

impl Exit {
    pub fn json(&self) -> Value {
        json!({"exitCode": self.code, "signal": self.signal.map(|s| signal_name(s))})
    }
}

fn signal_name(n: i32) -> &'static str {
    match n { 1 => "SIGHUP", 2 => "SIGINT", 3 => "SIGQUIT", 6 => "SIGABRT", 9 => "SIGKILL", 13 => "SIGPIPE", 15 => "SIGTERM", _ => "SIGNAL" }
}

/// `terminal/output` 的响应
pub fn terminal_output_result(output: &str, truncated: bool, exit: Option<Exit>) -> Value {
    let mut v = json!({"output": output, "truncated": truncated});
    if let Some(e) = exit { v["exitStatus"] = e.json(); }
    v
}

/// 实际执行的 argv：ACP 约定 command 是可执行文件、args 是参数；但有的 Agent 把整条命令行塞进 command，
/// 没有 args 且 command 里有空白时按 shell 命令行处理。stdout 和 stderr 合并（`2>&1`），和终端里看到的一致。
/// 返回传给 `sh -c` 的 (脚本, $0 之后的参数)。
pub fn shell_invocation(command: &str, args: &[String]) -> (String, Vec<String>) {
    if args.is_empty() && command.trim().contains(char::is_whitespace) {
        (format!("exec {} 2>&1", command.trim()), vec![])
    } else {
        // "$0" 是命令，"$@" 是参数，原样传递，不经过 shell 的二次解析
        ("exec \"$0\" \"$@\" 2>&1".to_string(), std::iter::once(command.to_string()).chain(args.iter().cloned()).collect())
    }
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
        assert_eq!(m["params"]["clientCapabilities"]["terminal"], true); // 终端由 Quill 起进程（acp_term）
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
    fn 恢复会话请求与能力判断() {
        let m = v(&load_session(5, "sess-1", "/proj"));
        assert_eq!(m["method"], "session/load");
        assert_eq!(m["params"]["sessionId"], "sess-1");
        assert_eq!(m["params"]["cwd"], "/proj");
        assert!(m["params"]["mcpServers"].is_array()); // 协议要求必须有
        assert!(!v(&load_session(5, "s", "/p")).to_string().contains('\n'));
        assert!(supports_load(&json!({"agentCapabilities": {"loadSession": true}})));
        assert!(!supports_load(&json!({"agentCapabilities": {"loadSession": false}})));
        assert!(!supports_load(&json!({"agentCapabilities": {}})));
        assert!(!supports_load(&json!({})));
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
        let r = parse_line(r#"{"jsonrpc":"2.0","id":9,"method":"workspace/unknown_thing","params":{}}"#);
        assert_eq!(r, Incoming::Request { id: json!(9), req: Request::Unknown("workspace/unknown_thing".into()) });
    }

    #[test]
    fn 按行读取切片() {
        let c = "a\nb\nc\nd\n";
        assert_eq!(slice_lines(c, None, None).unwrap(), c);
        assert_eq!(slice_lines(c, Some(2), None).unwrap(), "b\nc\nd\n");
        assert_eq!(slice_lines(c, Some(2), Some(2)).unwrap(), "b\nc\n");
        assert_eq!(slice_lines(c, None, Some(1)).unwrap(), "a\n");
        assert_eq!(slice_lines(c, Some(3), Some(100)).unwrap(), "c\nd\n"); // limit 超出文件末尾
        assert_eq!(slice_lines(c, Some(0), Some(1)).unwrap(), "a\n"); // 0 当 1
        assert_eq!(slice_lines("x", Some(1), Some(1)).unwrap(), "x"); // 没有结尾换行
    }

    #[test]
    fn 读取越界是错误_对标_zed() {
        let c = "a\nb\n"; // 3 行：a、b、末尾空行
        assert_eq!(slice_lines(c, Some(3), None).unwrap(), ""); // 末尾那个空行本身可以读
        let e = slice_lines(c, Some(4), Some(1)).unwrap_err();
        assert!(e.contains("超出文件末尾") && e.contains('3'), "{e}");
        assert!(slice_lines(c, Some(99), None).is_err());
        assert!(slice_lines("", Some(2), None).is_err()); // 空文件只有 1 行
        assert_eq!(slice_lines("", None, None).unwrap(), "");
    }

    #[test]
    fn 权限队列_并发请求不丢() {
        let mut q: PermQueue<&str> = PermQueue::default();
        assert!(q.is_empty());
        assert_eq!(q.push("t1", "第一个"), None);
        assert_eq!(q.push("t2", "第二个"), None);
        assert_eq!(q.len(), 2);
        // 前面的先处理，后面的不会被覆盖
        assert_eq!(q.front(), Some(&"第一个"));
        assert!(q.is_front(|s| *s == "第一个"));
        assert!(!q.is_front(|s| *s == "第二个"));
        assert!(q.contains(|s| *s == "第二个"));
        assert_eq!(q.pop_front(), Some("第一个"));
        assert_eq!(q.front(), Some(&"第二个"));
        assert_eq!(q.pop_front(), Some("第二个"));
        assert_eq!(q.pop_front(), None);
    }

    #[test]
    fn 权限队列_同一工具调用再来请求_旧的作废并还给调用方() {
        let mut q: PermQueue<&str> = PermQueue::default();
        q.push("t1", "旧");
        q.push("t2", "别的");
        assert_eq!(q.push("t1", "新"), Some("旧")); // 调用方要对「旧」回取消
        assert_eq!(q.len(), 2);
        assert_eq!(q.drain(), vec!["别的", "新"]); // 新的排到后面，顺序按到达时间
        assert!(q.is_empty());
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

    #[test]
    fn 去掉命令输出的代码围栏() {
        assert_eq!(strip_fence("```console\nacp_out.txt\nREADME.md\n```"), "acp_out.txt\nREADME.md");
        assert_eq!(strip_fence("```\n1\tline1\n```"), "1\tline1");
        assert_eq!(strip_fence("没有围栏\n第二行"), "没有围栏\n第二行");
        assert_eq!(strip_fence("```rust\nfn a(){}"), "```rust\nfn a(){}"); // 没有结束围栏：原样
        assert_eq!(strip_fence("```\n```"), "");
    }

    #[test]
    fn diff_预览只留不同的中间部分() {
        let p = diff_preview("line1\nhello\nline3\n", "line1\nworld\nline3\n", 14);
        assert_eq!(p, vec![('-', "hello".to_string()), ('+', "world".to_string())]);
        // 新建文件：全是新增
        let p = diff_preview("", "a\nb\n", 14);
        assert_eq!(p, vec![('+', "a".to_string()), ('+', "b".to_string())]);
        // 删除
        assert_eq!(diff_preview("a\nb\nc", "a\nc", 14), vec![('-', "b".to_string())]);
        // 没有差异
        assert!(diff_preview("same\n", "same\n", 14).is_empty());
        // 前后都有相同行时，中间多行替换
        let p = diff_preview("h\nx\ny\nt", "h\nz\nt", 14);
        assert_eq!(p, vec![('-', "x".into()), ('-', "y".into()), ('+', "z".into())]);
    }

    #[test]
    fn diff_预览超长截断() {
        let new: String = (0..100).map(|i| format!("l{i}\n")).collect();
        let p = diff_preview("", &new, 5);
        assert_eq!(p.len(), 6);
        assert_eq!(p[4], ('+', "l4".to_string()));
        assert_eq!(p[5].0, '…');
    }

    #[test]
    fn 投影带出_diff_预览() {
        let tc = ToolCall { id: "t".into(), title: "Edit a.txt".into(), kind: ToolKind::Edit, status: Status::Pending,
            content: vec![ToolContent::Diff { path: "/p/a.txt".into(), old: Some("hello".into()), new: "world".into() }], paths: vec![], raw_input: None };
        let (_, input, _) = project(&tc);
        assert_eq!(input["diffs"][0]["path"], "/p/a.txt");
        assert_eq!(input["diffs"][0]["preview"][0], json!(["-", "hello"]));
        assert_eq!(input["diffs"][0]["preview"][1], json!(["+", "world"]));
    }

    #[test]
    fn 审阅目标_只取项目内的_新建文件旧内容为空() {
        let root = std::path::Path::new("/p/proj");
        let tc = ToolCall { id: "t".into(), title: "".into(), kind: ToolKind::Edit, status: Status::Completed, paths: vec![], raw_input: None,
            content: vec![
                ToolContent::Diff { path: "/p/proj/a.txt".into(), old: Some("old".into()), new: "new".into() },
                ToolContent::Diff { path: "/p/proj/src/n.rs".into(), old: None, new: "fresh".into() },
                ToolContent::Diff { path: "/etc/hosts".into(), old: Some("x".into()), new: "y".into() },
                ToolContent::Text("忽略".into()),
            ] };
        assert_eq!(review_targets(&tc, root), vec![
            ("a.txt".to_string(), Some("old".to_string()), "new".to_string()),
            ("src/n.rs".to_string(), None, "fresh".to_string()),
        ]);
    }

    #[test]
    fn 对账_结尾换行不一致也算同一份() {
        // 真实适配器：diff 的新旧文本都没有结尾换行，磁盘上有
        let b = reconcile_before(Some("line1\nhello\nline3"), "line1\nworld\nline3", "line1\nworld\nline3\n");
        assert_eq!(b.as_deref(), Some("line1\nhello\nline3\n"));
        // 磁盘本来就没有结尾换行：不补
        assert_eq!(reconcile_before(Some("a"), "b", "b").as_deref(), Some("a"));
        // 新建文件：基线是空串
        assert_eq!(reconcile_before(None, "hello", "hello\n").as_deref(), Some(""));
        // 片段级 diff（磁盘比新文本长得多）/ 之后又被改过：对不上
        assert_eq!(reconcile_before(Some("hello"), "world", "line1\nworld\nline3\n"), None);
        assert_eq!(reconcile_before(Some("a"), "b", "c"), None);
    }

    #[test]
    fn 握手声明终端能力() {
        assert_eq!(v(&initialize(1))["params"]["clientCapabilities"]["terminal"], true);
    }

    #[test]
    fn 解析终端请求() {
        let r = parse_line(r#"{"jsonrpc":"2.0","id":3,"method":"terminal/create","params":{"sessionId":"s","command":"cargo","args":["test","--lib"],"env":[{"name":"A","value":"1"}],"cwd":"/p","outputByteLimit":2000}}"#);
        assert_eq!(r, Incoming::Request { id: json!(3), req: Request::TermCreate { command: "cargo".into(), args: vec!["test".into(), "--lib".into()], env: vec![("A".into(), "1".into())], cwd: Some("/p".into()), limit: Some(2000) } });
        let r = parse_line(r#"{"jsonrpc":"2.0","id":4,"method":"terminal/create","params":{"command":"ls"}}"#);
        assert_eq!(r, Incoming::Request { id: json!(4), req: Request::TermCreate { command: "ls".into(), args: vec![], env: vec![], cwd: None, limit: None } });
        for (m, want) in [("terminal/output", Request::TermOutput { id: "t1".into() }), ("terminal/wait_for_exit", Request::TermWait { id: "t1".into() }),
                          ("terminal/kill", Request::TermKill { id: "t1".into() }), ("terminal/release", Request::TermRelease { id: "t1".into() })] {
            let line = format!(r#"{{"jsonrpc":"2.0","id":8,"method":"{m}","params":{{"sessionId":"s","terminalId":"t1"}}}}"#);
            assert_eq!(parse_line(&line), Incoming::Request { id: json!(8), req: want }, "{m}");
        }
    }

    #[test]
    fn 输出缓冲_超限从开头丢并标记() {
        let mut b = OutputBuf::new(Some(10));
        b.push(b"0123456789");
        assert_eq!((b.text(), b.truncated()), ("0123456789", false));
        b.push(b"abc");
        assert_eq!((b.text(), b.truncated()), ("3456789abc", true));
    }

    #[test]
    fn 输出缓冲_多字节字符跨读取边界不乱码() {
        let bytes = "你好世界".as_bytes(); // 12 字节
        let mut b = OutputBuf::new(None);
        b.push(&bytes[..4]); // 切在第二个字的中间
        assert_eq!(b.text(), "你");
        b.push(&bytes[4..7]);
        assert_eq!(b.text(), "你好");
        b.push(&bytes[7..]);
        assert_eq!(b.text(), "你好世界");
        assert!(!b.text().contains('\u{fffd}'));
    }

    #[test]
    fn 输出缓冲_截断落在字符边界() {
        let mut b = OutputBuf::new(Some(7));
        b.push("中文字符".as_bytes()); // 12 字节，要丢 5 字节 → 退到字符边界
        assert!(b.text().len() <= 7);
        assert!(b.text().chars().all(|c| "中文字符".contains(c)));
        assert!(b.truncated());
    }

    #[test]
    fn 输出缓冲_非法字节宽容转换_不卡住() {
        let mut b = OutputBuf::new(None);
        b.push(&[b'a', 0xff, b'b']);
        assert!(b.text().starts_with('a') && b.text().ends_with('b'));
        b.push(b"c");
        assert!(b.text().ends_with("bc"));
    }

    #[test]
    fn 终端输出响应与退出状态() {
        assert_eq!(terminal_output_result("hi\n", false, None), json!({"output": "hi\n", "truncated": false}));
        let r = terminal_output_result("hi\n", true, Some(Exit { code: Some(0), signal: None }));
        assert_eq!(r["truncated"], true);
        assert_eq!(r["exitStatus"], json!({"exitCode": 0, "signal": null}));
        let r = terminal_output_result("", false, Some(Exit { code: None, signal: Some(9) }));
        assert_eq!(r["exitStatus"], json!({"exitCode": null, "signal": "SIGKILL"}));
    }

    #[test]
    fn 命令行拼装() {
        // 标准写法：可执行文件 + 参数，原样传递，不会被 shell 二次解析
        let (script, argv) = shell_invocation("cargo", &["test".to_string(), "a b".to_string()]);
        assert_eq!(script, "exec \"$0\" \"$@\" 2>&1");
        assert_eq!(argv, vec!["cargo", "test", "a b"]);
        // 整条命令行塞进 command：按 shell 处理
        let (script, argv) = shell_invocation("ls -la | head", &[]);
        assert_eq!(script, "exec ls -la | head 2>&1");
        assert!(argv.is_empty());
        // 单个词、没有参数：当成可执行文件
        let (_, argv) = shell_invocation("pwd", &[]);
        assert_eq!(argv, vec!["pwd"]);
    }

    // ---- 预热（#8）：Agent 启动 + 握手 + 建会话要好几秒，有使用意图时就先拉起来，发送时已经就绪 ----

    #[test]
    fn 预热意图_打字或聊天面板开着() {
        assert!(prewarm_intent(false, false), "输入框有字");
        assert!(prewarm_intent(true, true), "聊天面板开着（启动时面板就是开的）");
        assert!(prewarm_intent(false, true));
        // 面板关着、输入框也是空的：没有使用意图（只点一下输入框、切换对话都不算）
        assert!(!prewarm_intent(true, false));
    }

    #[test]
    fn 预热_条件() {
        // 用 Claude Code、有项目、没有进程、不忙、有意图：预热
        assert!(should_prewarm("claude-code", false, true, false, true));
        // 没有意图
        assert!(!should_prewarm("claude-code", false, true, false, false));
        // 已经有进程（预热过 / 正在用）：不重复起
        assert!(!should_prewarm("claude-code", true, true, false, true));
        // 没打开项目：Agent 没有工作目录可用
        assert!(!should_prewarm("claude-code", false, false, false, true));
        // 正在回复中：不碰
        assert!(!should_prewarm("claude-code", false, true, true, true));
        // 用 API Key 的后端：根本没有这个进程
        assert!(!should_prewarm("anthropic", false, true, false, true));
        assert!(!should_prewarm("", false, true, false, true));
    }

    // ---------- 会话模式（#9）----------
    // 为什么单独测：模式决定「每次改文件弹不弹确认」。解析错了 chip 不显示 / 显示错；
    // 循环顺序错了 Shift+Tab 会误切到「跳过所有确认」；记住规则错了计划模式会跨会话粘住，用户会困惑「怎么不改文件了」。

    fn real_modes() -> Value {
        v(r#"{"sessionId":"s","modes":{"currentModeId":"default","availableModes":[
            {"id":"default","name":"Manual","description":"Always ask before making changes"},
            {"id":"acceptEdits","name":"Accept edits","description":"Automatically accept all file edits"},
            {"id":"plan","name":"Plan","description":"Create a plan before making changes"},
            {"id":"auto","name":"Auto","description":"Claude handles permission decisions"},
            {"id":"bypassPermissions","name":"Bypass permissions","description":"Accepts all permissions"}]}}"#)
    }

    #[test]
    fn 从建会话结果解析模式() {
        let m = parse_modes(&real_modes()).unwrap();
        assert_eq!(m.current, "default");
        assert_eq!(m.available.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["default", "acceptEdits", "plan", "auto", "bypassPermissions"]);
        assert_eq!(m.available[1].name, "Accept edits");
        assert_eq!(m.available[1].description, "Automatically accept all file edits");
    }

    #[test]
    fn 没有模式或模式为空就是没有() {
        assert_eq!(parse_modes(&v(r#"{"sessionId":"s"}"#)), None);
        assert_eq!(parse_modes(&v(r#"{"modes":{"currentModeId":"x","availableModes":[]}}"#)), None);
        // 缺字段的条目跳过，不 panic
        let m = parse_modes(&v(r#"{"modes":{"currentModeId":"a","availableModes":[{"name":"无 id"},{"id":"a","name":"A"}]}}"#)).unwrap();
        assert_eq!(m.available.len(), 1);
    }

    #[test]
    fn 切换模式请求() {
        let m = v(&set_mode(7, "sess-1", "plan"));
        assert_eq!(m["method"], "session/set_mode");
        assert_eq!(m["id"], 7);
        assert_eq!(m["params"]["sessionId"], "sess-1");
        assert_eq!(m["params"]["modeId"], "plan");
    }

    #[test]
    fn agent_自己切换模式的通知() {
        let t = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"current_mode_update","currentModeId":"acceptEdits"}}}"#);
        assert_eq!(t, Incoming::Update(Update::Mode("acceptEdits".into())));
        // 缺 currentModeId：忽略
        let t = parse_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"current_mode_update"}}}"#);
        assert_eq!(t, Incoming::Update(Update::Other));
    }

    #[test]
    fn 已知模式有中文名未知的用适配器给的名字() {
        assert_eq!(mode_label("default", "Manual"), "手动确认");
        assert_eq!(mode_label("acceptEdits", "Accept edits"), "自动接受编辑");
        assert_eq!(mode_label("plan", "Plan"), "计划");
        assert_eq!(mode_label("auto", "Auto"), "自动");
        assert_eq!(mode_label("bypassPermissions", "Bypass permissions"), "跳过所有确认");
        assert_eq!(mode_label("dontAsk", "Don't ask"), "Don't ask");
    }

    #[test]
    fn 已知模式有中文说明未知的用原文() {
        assert_eq!(mode_hint("default", "Always ask before making changes"), "每次修改前都询问");
        assert_eq!(mode_hint("bypassPermissions", "x"), "全部放行，不再询问");
        assert_eq!(mode_hint("dontAsk", "Never ask"), "Never ask");
    }

    #[test]
    fn shift_tab_循环不会到跳过所有确认() {
        let mut m = parse_modes(&real_modes()).unwrap();
        let mut seen = vec![m.current.clone()];
        for _ in 0..4 {
            let n = cycle_next(&m).unwrap();
            m.current = n.clone();
            seen.push(n);
        }
        assert_eq!(seen, ["default", "acceptEdits", "plan", "auto", "default"]);
        // 当前就在「跳过所有确认」：下一个回到第一个，而不是卡住
        m.current = "bypassPermissions".into();
        assert_eq!(cycle_next(&m).as_deref(), Some("default"));
        // 只有一种模式：没得切
        let one = Modes { current: "default".into(), available: vec![m.available[0].clone()] };
        assert_eq!(cycle_next(&one), None);
    }

    #[test]
    fn 只记住常用模式() {
        assert!(persistable_mode("default"));
        assert!(persistable_mode("acceptEdits"));
        assert!(persistable_mode("auto"));
        assert!(!persistable_mode("plan"));
        assert!(!persistable_mode("bypassPermissions"));
        assert!(!persistable_mode(""));
    }

    #[test]
    fn 会话建好后按需补发记住的模式() {
        let m = parse_modes(&real_modes()).unwrap();
        assert_eq!(mode_to_apply("acceptEdits", &m).as_deref(), Some("acceptEdits"));
        // 已经是了：不发
        assert_eq!(mode_to_apply("default", &m), None);
        // 没记过：不发
        assert_eq!(mode_to_apply("", &m), None);
        // 适配器不支持这个模式（版本变了）：不发，不能让 Agent 报错
        assert_eq!(mode_to_apply("gone", &m), None);
        // 不该记的模式即使被写进了设置文件也不补发
        assert_eq!(mode_to_apply("bypassPermissions", &m), None);
    }
}

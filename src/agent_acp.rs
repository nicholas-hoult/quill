//! 对话面板的 Claude Code（ACP）后端（#7）：把 `acp_client` 送来的事件映射成面板里已有的消息 / 工具卡片 / 权限确认 / 审阅。
//! 是 `agent` 的子模块（所以能用它的私有字段）。协议语义在 `acp.rs`，进程在 `acp_client.rs`。
//! 错了表现为：发了消息没回音、工具卡片停在「…」、权限卡片点了 Agent 还在等、停止按钮不管用、切换对话后串了会话。
use super::*;
use crate::acp::{self, Choice, Incoming, Request, Update};
use crate::acp_client::{self, Event};
use crate::acp_term::Terminals;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) struct AcpPerm {
    pub(super) rpc_id: Value,
    pub(super) tool_id: String,
    pub(super) options: Vec<acp::PermOption>,
    pub(super) msg_id: u64,
    pub(super) call: usize,
}

pub(super) struct AcpState {
    handle: acp_client::Handle,
    pub(super) root: PathBuf,
    epoch: u64,
    next_rpc: u64,
    init_id: u64,
    new_id: u64,
    session: Option<String>,
    /// 正在进行的 prompt 的请求 id
    pub(super) prompt_id: Option<u64>,
    /// 被用户停止的 prompt：它的响应（cancelled）到了就丢弃，不能当成新一轮的结束
    ignored: HashSet<u64>,
    /// 等会话就绪 / 上一轮结束后再发的 prompt
    pub(super) queued: Option<String>,
    /// Agent 是否声明了 loadSession；以及正在恢复的会话 ID（恢复期间 Agent 重放的历史要忽略）
    can_load: bool,
    loading: Option<String>,
    /// 当前正在往里写内容的助手消息
    cur: Option<u64>,
    /// toolCallId → 合并后的完整工具调用
    tools: HashMap<String, acp::ToolCall>,
    /// 等待用户确认的权限请求（可能同时有多个：Agent 会并行调用工具）
    pub(super) perms: acp::PermQueue<AcpPerm>,
    /// Agent 的终端（`terminal/create`），以及正在等某个终端退出的 `terminal/wait_for_exit` 请求
    pub(super) terms: Terminals,
    term_waits: HashMap<String, Vec<Value>>,
    events: async_channel::Sender<Event>,
}

impl AgentPanel {
    fn msg_mut(&mut self, id: u64) -> Option<&mut Message> {
        self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == id)
    }

    pub(super) fn acp_perm_pending(&self) -> bool { self.acp.as_ref().map(|a| !a.perms.is_empty()).unwrap_or(false) }

    /// 关掉 Agent 进程（切换 / 新建 / 删除对话时调用：每个对话一个独立会话）
    pub(super) fn acp_reset(&mut self) { self.acp = None; }

    fn acp_start(&mut self, root: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let cmd = acp_client::command_line(&self.settings.acp_cmd);
        let tx2 = tx.clone();
        let handle = acp_client::spawn(&cmd, &root, tx)?;
        self.acp_epoch += 1;
        let epoch = self.acp_epoch;
        handle.send(acp::initialize(1));
        self.acp = Some(AcpState { handle, root, epoch, next_rpc: 3, init_id: 1, new_id: 0, session: None, can_load: false, loading: None, prompt_id: None, ignored: HashSet::new(), queued: None, cur: None, tools: HashMap::new(), perms: Default::default(), terms: Terminals::default(), term_waits: HashMap::new(), events: tx2 });
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |p, cx| p.on_acp(epoch, ev, cx)).is_err() { break; }
            }
        }).detach();
        Ok(())
    }

    /// 有使用意图时提前起 Agent、握手、建会话（#8）：输入框开始有字立即起；聊天面板开着则见 `acp_prewarm_soon`。
    /// 预热期间出任何错都不往对话里写：用户还没发消息，真正发送时会重新起一次，那时的错误才该让他看到。
    pub(super) fn acp_prewarm(&mut self, cx: &mut Context<Self>) {
        let empty = self.composer.read(cx).is_empty();
        let open = self.workspace.update(cx, |w, _| w.agent_is_open()).unwrap_or(false);
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        if !acp::should_prewarm(&self.settings.provider, self.acp.is_some(), root.is_some(), self.cancel.is_some(), acp::prewarm_intent(empty, open)) { return; }
        if let Some(root) = root { let _ = self.acp_start(root, cx); }
    }

    /// 项目加载好、聊天面板开着时，稍等一会再预热：让启动先把编辑器画出来，别和它抢 CPU。
    /// 反复调用无害（到点再检查一次条件，已经起了就什么都不做）。
    pub(super) fn acp_prewarm_soon(&mut self, cx: &mut Context<Self>) {
        if self.acp.is_some() || self.settings.provider != "claude-code" { return; }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update(cx, |p, cx| p.acp_prewarm(cx));
        }).detach();
    }

    /// 预热期间（没有在等回复）的失败：悄悄丢掉进程，下次发送会重来；在等回复时才走正常的报错
    fn acp_fail_or_drop(&mut self, err: String, cx: &mut Context<Self>) {
        if self.cancel.is_none() { self.acp = None; } else { self.acp_fail(err, cx); }
    }

    /// 发送一条消息给 Claude Code。会话没起来就先起（异步握手），消息排队等就绪
    pub(super) fn acp_send(&mut self, prompt: String, cx: &mut Context<Self>) {
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        let Some(root) = root else { self.acp_fail("没有打开项目：Claude Code 需要一个工作目录".into(), cx); return };
        if self.acp.as_ref().map(|a| a.root != root).unwrap_or(false) { self.acp = None; }
        if self.acp.is_none() {
            if let Err(e) = self.acp_start(root, cx) { self.acp_fail(e, cx); return; }
        }
        self.cancel = Some(Arc::new(AtomicBool::new(false)));
        if let Some(a) = self.acp.as_mut() { a.cur = None; a.queued = Some(prompt); }
        self.acp_target(false); // 先放一条空的助手消息，界面显示「思考中…」
        self.acp_flush();
        cx.notify();
    }

    fn acp_flush(&mut self) {
        let Some(a) = self.acp.as_mut() else { return };
        let (Some(sid), None) = (a.session.clone(), a.prompt_id) else { return };
        if let Some(q) = a.queued.take() {
            let id = a.next_rpc;
            a.next_rpc += 1;
            a.prompt_id = Some(id);
            a.handle.send(acp::prompt(id, &sid, &q));
        }
    }

    /// 内容该写进哪条助手消息：文字紧跟在工具调用之后时另起一条（这样界面上文字和工具卡片保持先后顺序）
    fn acp_target(&mut self, for_text: bool) -> u64 {
        let cur = self.acp.as_ref().and_then(|a| a.cur);
        if let Some(id) = cur {
            if self.msg_mut(id).map(|m| !(for_text && !m.calls.is_empty())).unwrap_or(false) { return id; }
            if let Some(m) = self.msg_mut(id) { m.status = Status::Done; }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.thread().messages.push(Message { id, calls: vec![], role: Role::Assistant, content: String::new(), refs: vec![], status: Status::Streaming, applied: HashSet::new(), snapshots: vec![] });
        if let Some(a) = self.acp.as_mut() { a.cur = Some(id); }
        id
    }

    /// 失败：正在流式的消息标红，结束忙状态
    fn acp_fail(&mut self, err: String, cx: &mut Context<Self>) {
        let hint = { let l = err.to_lowercase(); if l.contains("auth") || l.contains("login") || l.contains("登录") { "\n请先在终端运行 claude 完成登录。" } else { "" } };
        let text = format!("{err}{hint}");
        let mut any = false;
        for m in self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()) {
            if m.status == Status::Streaming { m.status = Status::Error(text.clone()); any = true; }
        }
        if !any {
            // 没有流式中的消息（启动前就失败）：补一条带错误的助手消息，让用户看得到
            let id = self.next_id;
            self.next_id += 1;
            self.thread().messages.push(Message { id, calls: vec![], role: Role::Assistant, content: String::new(), refs: vec![], status: Status::Error(text), applied: HashSet::new(), snapshots: vec![] });
        }
        self.cancel = None;
        if let Some(a) = self.acp.as_mut() { a.perms.drain(); a.queued = None; a.prompt_id = None; a.cur = None; }
        self.save();
        cx.notify();
    }

    fn acp_finish(&mut self, stop: &str, cx: &mut Context<Self>) {
        for m in self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()) {
            if m.status == Status::Streaming { m.status = Status::Done; }
        }
        if let Some(note) = acp::stop_note(stop) {
            let id = self.acp.as_ref().and_then(|a| a.cur);
            if let Some(m) = id.and_then(|id| self.msg_mut(id)) { m.content.push_str(&format!("\n\n（{note}）")); }
        }
        self.cancel = None;
        if let Some(a) = self.acp.as_mut() { a.perms.drain(); a.cur = None; }
        self.save();
        self.acp_flush();
        cx.notify();
    }

    fn on_acp(&mut self, epoch: u64, ev: Event, cx: &mut Context<Self>) {
        if self.acp.as_ref().map(|a| a.epoch != epoch).unwrap_or(true) { return; }
        match ev {
            Event::Exited(msg) => {
                let busy = self.cancel.is_some();
                self.acp = None;
                if busy { self.acp_fail(format!("Claude Code 进程已退出：{msg}"), cx); }
            }
            Event::Line(Incoming::Response { id, result }) => self.acp_response(id, result, cx),
            Event::Line(Incoming::Update(u)) => self.acp_update(u, cx),
            Event::Line(Incoming::Request { id, req }) => self.acp_request(id, req, cx),
            Event::Line(Incoming::Ignore) => {}
            Event::TermOutput => { if self.stick { self.scroll_end(); } cx.notify(); }
            Event::TermExit(id) => {
                let Some(a) = self.acp.as_mut() else { return };
                let exit = a.terms.exit(&id).flatten();
                for rpc in a.term_waits.remove(&id).unwrap_or_default() {
                    a.handle.send(acp::respond(&rpc, exit.map(|e| e.json()).unwrap_or(json!({}))));
                }
                cx.notify();
            }
        }
    }

    fn acp_response(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let stored = self.threads[self.active].acp_session.clone();
        let Some(a) = self.acp.as_mut() else { return };
        if a.ignored.remove(&id) { if a.prompt_id == Some(id) { a.prompt_id = None; } self.acp_flush(); return; }
        if id == a.init_id {
            match result {
                Ok(v) => {
                    a.can_load = acp::supports_load(&v);
                    let nid = a.next_rpc;
                    a.next_rpc += 1;
                    a.new_id = nid;
                    let cwd = a.root.to_string_lossy().to_string();
                    // 这个对话以前有过会话：恢复它，Agent 就记得之前的上下文
                    match stored.filter(|_| a.can_load) {
                        Some(sid) => { a.handle.send(acp::load_session(nid, &sid, &cwd)); a.loading = Some(sid); }
                        None => a.handle.send(acp::new_session(nid, &cwd)),
                    }
                }
                Err(e) => self.acp_fail_or_drop(format!("握手失败：{e}"), cx),
            }
        } else if id == a.new_id {
            if let Some(sid) = a.loading.take() {
                match result {
                    Ok(_) => { a.session = Some(sid); self.acp_flush(); }
                    Err(_) => {
                        // 恢复失败（会话已被清理 / 换了项目）：退回新建会话，并忘掉旧的 ID
                        let nid = a.next_rpc;
                        a.next_rpc += 1;
                        a.new_id = nid;
                        let cwd = a.root.to_string_lossy().to_string();
                        a.handle.send(acp::new_session(nid, &cwd));
                        self.threads[self.active].acp_session = None;
                    }
                }
                return;
            }
            match result {
                Ok(v) => match v["sessionId"].as_str() {
                    Some(s) => {
                        a.session = Some(s.to_string());
                        self.threads[self.active].acp_session = Some(s.to_string());
                        self.save();
                        self.acp_flush();
                    }
                    None => self.acp_fail_or_drop("Agent 没有返回会话 ID".into(), cx),
                },
                Err(e) => self.acp_fail_or_drop(format!("创建会话失败：{e}"), cx),
            }
        } else if a.prompt_id == Some(id) {
            a.prompt_id = None;
            match result {
                Ok(v) => self.acp_finish(v["stopReason"].as_str().unwrap_or("end_turn"), cx),
                Err(e) => self.acp_fail(e, cx),
            }
        }
    }

    fn acp_update(&mut self, u: Update, cx: &mut Context<Self>) {
        // 恢复会话时 Agent 会把历史重放一遍：界面上已经有了，忽略
        if self.acp.as_ref().map(|a| a.loading.is_some()).unwrap_or(false) { return; }
        match u {
            Update::Text(t) => {
                let id = self.acp_target(true);
                if let Some(m) = self.msg_mut(id) { m.content.push_str(&t); }
            }
            Update::ToolCall(tc) => { self.acp_set_tool(tc); }
            Update::ToolUpdate(p) => {
                let Some(a) = self.acp.as_mut() else { return };
                let was_done = a.tools.get(&p.id).map(|t| t.is_done()).unwrap_or(false);
                let tc = match a.tools.get_mut(&p.id) { Some(t) => { t.apply(&p); t.clone() } None => acp::ToolCall::from_patch(&p) };
                let finished = !was_done && tc.status == acp::Status::Completed;
                self.acp_set_tool(tc.clone());
                if finished { self.acp_review(&tc, cx); }
            }
            Update::Plan(es) => {
                let (name, input, result) = acp::project_plan(&es);
                self.acp_set_call("plan".into(), name, input, result);
            }
            Update::Thought(t) => {
                // 思考内容：每条助手消息一块，折叠显示；连续的思考片段累加
                let mid = self.acp_target(false);
                let id = format!("thought:{mid}");
                let prev = self.threads[self.active].messages.iter().flat_map(|m| m.calls.iter()).find(|c| c.id == id).and_then(|c| c.input["text"].as_str().map(str::to_string)).unwrap_or_default();
                self.acp_set_call(id, "acp:thought".into(), json!({"text": format!("{prev}{t}")}), Some(String::new()));
            }
            Update::Other => return,
        }
        if self.stick { self.scroll_end(); }
        cx.notify();
    }

    fn acp_set_tool(&mut self, tc: acp::ToolCall) -> Option<(u64, usize)> {
        let (name, input, result) = acp::project(&tc);
        let id = tc.id.clone();
        self.acp.as_mut()?.tools.insert(id.clone(), tc);
        self.acp_set_call(id, name, input, result)
    }

    /// 按 id 更新已有的工具卡片，没有就在当前消息里新增；返回 (消息 id, 序号)
    fn acp_set_call(&mut self, id: String, name: String, input: Value, result: Option<String>) -> Option<(u64, usize)> {
        let found = self.threads[self.active].messages.iter().find_map(|m| m.calls.iter().position(|c| c.id == id).map(|i| (m.id, i)));
        if let Some((mid, i)) = found {
            let c = &mut self.msg_mut(mid)?.calls[i];
            c.name = name;
            c.input = input;
            c.result = result;
            return Some((mid, i));
        }
        let mid = self.acp_target(false);
        let m = self.msg_mut(mid)?;
        m.calls.push(ToolCall { id, name, input, result });
        Some((mid, m.calls.len() - 1))
    }

    /// 工具完成且带整文件 diff：Agent 已经自己改了磁盘，把改动放进编辑器审阅（并记检查点）。
    /// 只在磁盘上的内容确实等于 diff 的新文本时才进入——片段级的 diff 没法当整文件基线。
    fn acp_review(&mut self, tc: &acp::ToolCall, cx: &mut Context<Self>) {
        let Some(root) = self.acp.as_ref().map(|a| a.root.clone()) else { return };
        let mid = self.acp.as_ref().and_then(|a| a.cur);
        for (rel, before, after) in acp::review_targets(tc, &root) {
            let Ok(disk) = std::fs::read_to_string(root.join(&rel)) else { continue };
            let Some(before) = acp::reconcile_before(before.as_deref(), &after, &disk) else { continue };
            if before == disk { continue; } // 没有实际改动
            let before = if before.is_empty() && !disk.is_empty() { None } else { Some(before) }; // 空基线 = 新建的文件
            if let Some(owner) = mid.and_then(|m| self.owner_user(m)) {
                if let Some(m) = self.msg_mut(owner) { crate::checkpoint::record(&mut m.snapshots, &rel, before.clone()); }
            }
            cx.emit(AgentEvent::ReviewFile { path: rel, before });
        }
    }

    fn acp_reply(&mut self, line: String) { if let Some(a) = self.acp.as_ref() { a.handle.send(line); } }

    fn acp_request(&mut self, id: Value, req: Request, cx: &mut Context<Self>) {
        match req {
            Request::Permission { tool, options } => {
                let Some(a) = self.acp.as_mut() else { return };
                let tc = match a.tools.get(&tool.id) { Some(t) => { let mut t = t.clone(); t.apply(&tool); t } None => acp::ToolCall::from_patch(&tool) };
                // 「本会话自动允许」打开时直接放行
                if self.always_allow {
                    let opt = acp::pick_option(&options, Choice::Allow).map(|o| o.id.clone());
                    self.acp_reply(acp::respond(&id, acp::permission_result(opt.as_deref())));
                    self.acp_set_tool(tc);
                } else if let Some((msg_id, call)) = self.acp_set_tool(tc) {
                    let tool_id = tool.id.clone();
                    // 同一个工具调用又来一个请求：旧的作废，要回「取消」，否则 Agent 会一直等它
                    let old = self.acp.as_mut().and_then(|a| a.perms.push(&tool_id, AcpPerm { rpc_id: id, tool_id: tool_id.clone(), options, msg_id, call }));
                    if let Some(old) = old { self.acp_reply(acp::respond(&old.rpc_id, acp::permission_result(None))); }
                    if self.stick { self.scroll_end(); }
                }
            }
            Request::ReadFile { path, line, limit } => {
                // 对标 Zed：只读项目内的文件（经编辑器缓冲区，含未保存的修改）；起始行越界是错误
                let root = self.acp.as_ref().map(|a| a.root.clone());
                let reply = if root.as_ref().and_then(|r| acp::rel_path(r, &path)).is_none() {
                    acp::respond_err(&id, -32002, &format!("资源不存在或不在项目内：{path}"))
                } else {
                    match self.workspace.update(cx, |w, cx| w.file_text(&path, cx)).ok().flatten() {
                        Some(t) => match acp::slice_lines(&t, line, limit) {
                            Ok(c) => acp::respond(&id, json!({"content": c})),
                            Err(e) => acp::respond_err(&id, -32602, &e),
                        },
                        None => acp::respond_err(&id, -32002, &format!("无法读取 {path}")),
                    }
                };
                self.acp_reply(reply);
            }
            Request::WriteFile { path, content } => {
                let root = self.acp.as_ref().map(|a| a.root.clone());
                match root.and_then(|r| acp::rel_path(&r, &path)) {
                    Some(rel) => {
                        // 对标 Zed：先写盘（Agent 之后用 Bash / 再读到的一定是新内容），审阅在事后：
                        // 编辑器里照常逐块接受 / 拒绝，拒绝会自动再存一次；并记检查点，可整体回退
                        let mid = self.acp_target(false);
                        self.emit_apply_ex(mid, Some(rel), content, true, cx);
                        self.acp_reply(acp::respond(&id, json!({})));
                    }
                    None => self.acp_reply(acp::respond_err(&id, -32602, &format!("路径不在项目内：{path}"))),
                }
            }
            Request::TermCreate { command, args, env, cwd, limit } => {
                let Some(a) = self.acp.as_mut() else { return };
                let dir = cwd.map(PathBuf::from).filter(|p| p.is_absolute() && p.is_dir()).unwrap_or_else(|| a.root.clone());
                let reply = match a.terms.create(&command, &args, &env, &dir, limit, a.events.clone()) {
                    Ok(tid) => acp::respond(&id, json!({"terminalId": tid})),
                    Err(e) => acp::respond_err(&id, -32000, &e),
                };
                self.acp_reply(reply);
            }
            Request::TermOutput { id: tid } => {
                let reply = match self.acp.as_ref().and_then(|a| a.terms.output(&tid)) {
                    Some((text, truncated, exit)) => acp::respond(&id, acp::terminal_output_result(&text, truncated, exit)),
                    None => acp::respond_err(&id, -32002, &format!("没有这个终端：{tid}")),
                };
                self.acp_reply(reply);
            }
            Request::TermWait { id: tid } => {
                let Some(a) = self.acp.as_mut() else { return };
                match a.terms.exit(&tid) {
                    None => { let l = acp::respond_err(&id, -32002, &format!("没有这个终端：{tid}")); a.handle.send(l); }
                    Some(Some(e)) => { let l = acp::respond(&id, e.json()); a.handle.send(l); }
                    Some(None) => a.term_waits.entry(tid).or_default().push(id), // 退出时由 Event::TermExit 回应
                }
            }
            Request::TermKill { id: tid } | Request::TermRelease { id: tid } => {
                // release 也只是终止；输出保留在内存里，卡片完成后还要显示
                if let Some(a) = self.acp.as_ref() { a.terms.kill(&tid); }
                self.acp_reply(acp::respond(&id, json!({})));
            }
            Request::Unknown(m) => self.acp_reply(acp::respond_err(&id, -32601, &format!("Quill 不支持 {m}"))),
        }
        cx.notify();
    }

    /// 用户对权限卡片的选择
    pub(super) fn acp_decide(&mut self, d: Decision, cx: &mut Context<Self>) {
        let Some(a) = self.acp.as_mut() else { return };
        let Some(p) = a.perms.pop_front() else { return };
        let choice = match d { Decision::Once => Choice::Allow, Decision::Always => Choice::AllowAlways, Decision::Deny => Choice::Reject };
        let opt = acp::pick_option(&p.options, choice).map(|o| o.id.clone());
        a.handle.send(acp::respond(&p.rpc_id, acp::permission_result(opt.as_deref())));
        cx.notify();
    }

    /// 停止按钮 / Esc：通知 Agent 取消，并回应还在等的权限请求
    pub(super) fn acp_stop(&mut self) {
        let Some(a) = self.acp.as_mut() else { return };
        for p in a.perms.drain() { a.handle.send(acp::respond(&p.rpc_id, acp::permission_result(None))); }
        if let (Some(sid), Some(pid)) = (a.session.clone(), a.prompt_id) {
            a.handle.send(acp::cancel(&sid));
            a.ignored.insert(pid);
        }
        a.queued = None;
        a.cur = None;
        a.terms.kill_all();
    }

    // ---------- 界面 ----------

    /// Claude Code 的一次工具调用 / 计划：一行摘要，有输出时展开成卡片，等待权限时带确认按钮
    pub(super) fn render_acp_call(&self, msg_id: u64, i: usize, c: &ToolCall, cx: &mut Context<Self>) -> Div {
        let t = cx.theme().clone();
        let kind = c.name.trim_start_matches("acp:").to_string();
        if kind == "plan" { return self.render_plan(c, &t); }
        if kind == "thought" { return self.render_thought(msg_id, i, c, cx); }
        let title = c.input["title"].as_str().unwrap_or("").to_string();
        let title = if title.is_empty() { kind.clone() } else { title };
        let here = |p: &AcpPerm| p.msg_id == msg_id && p.call == i;
        let perm = self.acp.as_ref().map(|a| a.perms.is_front(here)).unwrap_or(false);
        // 排在后面的请求：只显示「排队中」，轮到它时才出现按钮
        let queued = !perm && self.acp.as_ref().map(|a| a.perms.contains(here)).unwrap_or(false);
        let failed = c.result.as_deref().map(|r| r.starts_with("错误")).unwrap_or(false);
        let ic = match kind.as_str() { "execute" => "terminal", "search" => "search", "edit" | "delete" | "move" => "agent", "think" | "other" | "switch_mode" => "alert", _ => "file" };
        // 有终端时显示终端的实时输出，否则显示工具返回的文字
        let term = c.input["terminal"].as_str().and_then(|id| self.acp.as_ref().and_then(|a| a.terms.view(id)));
        let running = term.as_ref().map(|(_, r)| *r).unwrap_or(false);
        let body: Vec<String> = match &term {
            Some((text, _)) => text.lines().map(str::to_string).collect(),
            None => c.result.as_deref().filter(|_| !failed || kind == "execute").map(|r| r.lines().map(str::to_string).collect()).unwrap_or_default(),
        };
        const SHOWN: usize = 8;
        let hidden = body.len().saturating_sub(SHOWN);
        let shown: Vec<String> = body[hidden..].to_vec();
        let show_body = !shown.is_empty() && matches!(kind.as_str(), "execute" | "search" | "fetch" | "other");
        let uid = (msg_id * 100 + i as u64) as usize;
        let command = c.input["raw"]["command"].as_str().map(str::to_string);
        div().flex().flex_col().rounded(px(9.)).border_1().border_color(if perm { t.accent_line } else { t.border }).overflow_hidden()
            .child(
                div().flex().items_center().gap(px(8.)).h(px(30.)).px(px(10.)).text_size(px(12.5)).text_color(t.dim)
                    .child(icon(ic, 13., if perm { t.accent } else { t.faint }))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO_FONT).text_size(px(12.)).text_color(t.text).child(title))
                    .child(if perm { div().flex_none().text_size(px(11.5)).text_color(t.accent).child("等待确认").into_any_element() }
                        else if running { div().flex_none().text_size(px(11.5)).text_color(t.dim).child("运行中…").into_any_element() }
                        else if queued { div().flex_none().text_size(px(11.5)).text_color(t.faint).child("排队等待确认").into_any_element() }
                        else if failed { icon("x", 13., t.del).into_any_element() }
                        else if c.result.is_some() { icon("check", 13., t.add).into_any_element() }
                        else { div().text_color(t.faint).child("…").into_any_element() }),
            )
            .when_some(command.filter(|_| kind == "execute"), |d, cmd| d.child(
                div().flex().gap(px(7.)).px(px(11.)).py(px(6.)).border_t_1().border_color(t.border).bg(t.bg).font_family(MONO_FONT).text_size(px(12.)).line_height(px(18.))
                    .child(div().flex_none().text_color(t.accent).child("$")).child(div().flex_1().min_w_0().text_color(t.text).child(cmd)),
            ))
            .children(c.input["diffs"].as_array().cloned().unwrap_or_default().into_iter().take(3).map(|df| {
                let name = df["path"].as_str().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
                let lines: Vec<(String, String)> = df["preview"].as_array().map(|a| a.iter().map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
                div().flex().flex_col().border_t_1().border_color(t.border).bg(t.bg).font_family(MONO_FONT).text_size(px(11.5)).line_height(px(17.))
                    .child(div().px(px(11.)).py(px(4.)).text_color(t.faint).child(name))
                    .children(lines.into_iter().map(|(m, l)| {
                        let (bg, fg, mark) = match m.as_str() { "-" => (t.del_bg, t.del, "−"), "+" => (t.add_bg, t.add, "+"), _ => (t.bg, t.faint, "…") };
                        div().flex().gap(px(8.)).px(px(11.)).bg(bg).text_color(fg).child(div().flex_none().w(px(10.)).child(mark)).child(div().flex_1().min_w_0().child(if l.is_empty() { " ".to_string() } else { l }))
                    }))
            }))
            .when(show_body, |d| d.child(
                div().flex().flex_col().px(px(11.)).py(px(7.)).border_t_1().border_color(t.border).bg(t.bg).font_family(MONO_FONT).text_size(px(11.5)).line_height(px(17.)).text_color(if failed { t.del } else { t.dim })
                    .when(hidden > 0, |d| d.child(div().text_color(t.faint).child(format!("… 前面还有 {hidden} 行"))))
                    .children(shown.iter().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.clone() }))),
            ))
            .when(perm, |d| d.child(
                div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(8.)).border_t_1().border_color(t.border).bg(t.elev)
                    .child(div().id(("acp-always", uid)).text_size(px(12.)).text_color(t.faint).cursor_pointer().hover(|s| s.text_color(t.dim)).child("本会话始终允许")
                        .on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Always, cx))))
                    .child(div().flex_1())
                    .child(btn(("acp-deny", uid), "拒绝  Esc", false, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Deny, cx))))
                    .child(btn(("acp-allow", uid), "允许  ⌘↵", true, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Once, cx)))),
            ))
    }

    /// 思考内容：默认折叠成一行，点开看全文
    fn render_thought(&self, msg_id: u64, i: usize, c: &ToolCall, cx: &mut Context<Self>) -> Div {
        let t = cx.theme().clone();
        let text = c.input["text"].as_str().unwrap_or("").trim().to_string();
        let key = (msg_id, 20_000 + i);
        let open = self.expanded.contains(&key);
        let first = text.lines().next().unwrap_or("").to_string();
        div().flex().flex_col().gap(px(4.))
            .child(
                div().id(("thought", (msg_id * 100 + i as u64) as usize)).flex().items_center().gap(px(6.)).h(px(24.)).cursor_pointer().text_size(px(12.)).text_color(t.faint).hover(|s| s.text_color(t.dim))
                    .on_click(cx.listener(move |this, _, _, cx| { if !this.expanded.remove(&key) { this.expanded.insert(key); } cx.notify(); }))
                    .child(icon(if open { "chevron_down" } else { "chevron" }, 12., t.faint))
                    .child("思考")
                    .when(!open, |d| d.child(div().flex_1().min_w_0().truncate().child(first))),
            )
            .when(open, |d| d.child(div().pl(px(18.)).text_size(px(12.5)).line_height(px(19.)).text_color(t.dim).child(text)))
    }

    fn render_plan(&self, c: &ToolCall, t: &crate::theme::Theme) -> Div {
        let entries: Vec<(String, String)> = c.input["entries"].as_array().map(|a| a.iter().map(|e| (e["text"].as_str().unwrap_or("").to_string(), e["status"].as_str().unwrap_or("pending").to_string())).collect()).unwrap_or_default();
        div().flex().flex_col().gap(px(5.)).px(px(11.)).py(px(9.)).rounded(px(9.)).border_1().border_color(t.border).text_size(px(12.5))
            .child(div().text_size(px(11.)).text_color(t.faint).child("计划"))
            .children(entries.into_iter().map(|(text, st)| {
                let (mark, col) = match st.as_str() { "completed" => ("✓", t.add), "in_progress" => ("●", t.accent), _ => ("○", t.faint) };
                div().flex().gap(px(8.)).text_color(if st == "completed" { t.dim } else { t.text })
                    .child(div().flex_none().w(px(12.)).text_color(col).child(mark))
                    .child(div().flex_1().min_w_0().child(text))
            }))
    }
}

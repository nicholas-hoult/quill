//! 对话面板的 Claude Code（ACP）后端（#7）：把 `acp_client` 送来的事件映射成面板里已有的消息 / 工具卡片 / 权限确认 / 审阅。
//! 是 `agent` 的子模块（所以能用它的私有字段）。协议语义在 `acp.rs`，进程在 `acp_client.rs`。
//! 错了表现为：发了消息没回音、工具卡片停在「…」、权限卡片点了 Agent 还在等、停止按钮不管用、切换对话后串了会话。
use super::*;
use crate::acp::{self, Choice, Incoming, Request, Update};
use crate::acp_client::{self, Event};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) struct AcpPerm {
    pub(super) rpc_id: Value,
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
    /// 当前正在往里写内容的助手消息
    cur: Option<u64>,
    /// toolCallId → 合并后的完整工具调用
    tools: HashMap<String, acp::ToolCall>,
    pub(super) perm: Option<AcpPerm>,
}

impl AgentPanel {
    fn msg_mut(&mut self, id: u64) -> Option<&mut Message> {
        self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == id)
    }

    pub(super) fn acp_perm_pending(&self) -> bool { self.acp.as_ref().map(|a| a.perm.is_some()).unwrap_or(false) }

    /// 关掉 Agent 进程（切换 / 新建 / 删除对话时调用：每个对话一个独立会话）
    pub(super) fn acp_reset(&mut self) { self.acp = None; }

    fn acp_start(&mut self, root: PathBuf, cx: &mut Context<Self>) -> Result<(), String> {
        let (tx, rx) = async_channel::unbounded();
        let cmd = acp_client::command_line(&self.settings.acp_cmd);
        let handle = acp_client::spawn(&cmd, &root, tx)?;
        self.acp_epoch += 1;
        let epoch = self.acp_epoch;
        handle.send(acp::initialize(1));
        self.acp = Some(AcpState { handle, root, epoch, next_rpc: 3, init_id: 1, new_id: 0, session: None, prompt_id: None, ignored: HashSet::new(), queued: None, cur: None, tools: HashMap::new(), perm: None });
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |p, cx| p.on_acp(epoch, ev, cx)).is_err() { break; }
            }
        }).detach();
        Ok(())
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
        if let Some(a) = self.acp.as_mut() { a.perm = None; a.queued = None; a.prompt_id = None; a.cur = None; }
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
        if let Some(a) = self.acp.as_mut() { a.perm = None; a.cur = None; }
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
        }
    }

    fn acp_response(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(a) = self.acp.as_mut() else { return };
        if a.ignored.remove(&id) { if a.prompt_id == Some(id) { a.prompt_id = None; } self.acp_flush(); return; }
        if id == a.init_id {
            match result {
                Ok(_) => {
                    let nid = a.next_rpc;
                    a.next_rpc += 1;
                    a.new_id = nid;
                    let cwd = a.root.to_string_lossy().to_string();
                    a.handle.send(acp::new_session(nid, &cwd));
                }
                Err(e) => self.acp_fail(format!("握手失败：{e}"), cx),
            }
        } else if id == a.new_id {
            match result {
                Ok(v) => match v["sessionId"].as_str() {
                    Some(s) => { a.session = Some(s.to_string()); self.acp_flush(); }
                    None => self.acp_fail("Agent 没有返回会话 ID".into(), cx),
                },
                Err(e) => self.acp_fail(format!("创建会话失败：{e}"), cx),
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
        match u {
            Update::Text(t) => {
                let id = self.acp_target(true);
                if let Some(m) = self.msg_mut(id) { m.content.push_str(&t); }
            }
            Update::ToolCall(tc) => { self.acp_set_tool(tc); }
            Update::ToolUpdate(p) => {
                let Some(a) = self.acp.as_mut() else { return };
                let tc = match a.tools.get_mut(&p.id) { Some(t) => { t.apply(&p); t.clone() } None => acp::ToolCall::from_patch(&p) };
                self.acp_set_tool(tc);
            }
            Update::Plan(es) => {
                let (name, input, result) = acp::project_plan(&es);
                self.acp_set_call("plan".into(), name, input, result);
            }
            Update::Thought(_) | Update::Other => return,
        }
        if self.stick { self.scroll.scroll_to_bottom(); }
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
                    if let Some(a) = self.acp.as_mut() { a.perm = Some(AcpPerm { rpc_id: id, options, msg_id, call }); }
                    if self.stick { self.scroll.scroll_to_bottom(); }
                }
            }
            Request::ReadFile { path, line, limit } => {
                let text = self.workspace.update(cx, |w, cx| w.file_text(&path, cx)).ok().flatten();
                let line = match text {
                    Some(t) => acp::respond(&id, json!({"content": acp::slice_lines(&t, line, limit)})),
                    None => acp::respond_err(&id, -32002, &format!("无法读取 {path}")),
                };
                self.acp_reply(line);
            }
            Request::WriteFile { path, content } => {
                let root = self.acp.as_ref().map(|a| a.root.clone());
                match root.and_then(|r| acp::rel_path(&r, &path)) {
                    Some(rel) => {
                        // 和 edit_file 一样走逐块审阅（并记检查点）；Agent 要立刻拿到成功才会继续，所以先回应
                        let mid = self.acp_target(false);
                        self.emit_apply(mid, Some(rel), content, cx);
                        self.acp_reply(acp::respond(&id, json!({})));
                    }
                    None => self.acp_reply(acp::respond_err(&id, -32602, &format!("路径不在项目内：{path}"))),
                }
            }
            Request::Unknown(m) => self.acp_reply(acp::respond_err(&id, -32601, &format!("Quill 不支持 {m}"))),
        }
        cx.notify();
    }

    /// 用户对权限卡片的选择
    pub(super) fn acp_decide(&mut self, d: Decision, cx: &mut Context<Self>) {
        let Some(a) = self.acp.as_mut() else { return };
        let Some(p) = a.perm.take() else { return };
        let choice = match d { Decision::Once => Choice::Allow, Decision::Always => Choice::AllowAlways, Decision::Deny => Choice::Reject };
        let opt = acp::pick_option(&p.options, choice).map(|o| o.id.clone());
        a.handle.send(acp::respond(&p.rpc_id, acp::permission_result(opt.as_deref())));
        cx.notify();
    }

    /// 停止按钮 / Esc：通知 Agent 取消，并回应还在等的权限请求
    pub(super) fn acp_stop(&mut self) {
        let Some(a) = self.acp.as_mut() else { return };
        if let Some(p) = a.perm.take() { a.handle.send(acp::respond(&p.rpc_id, acp::permission_result(None))); }
        if let (Some(sid), Some(pid)) = (a.session.clone(), a.prompt_id) {
            a.handle.send(acp::cancel(&sid));
            a.ignored.insert(pid);
        }
        a.queued = None;
        a.cur = None;
    }

    // ---------- 界面 ----------

    /// Claude Code 的一次工具调用 / 计划：一行摘要，有输出时展开成卡片，等待权限时带确认按钮
    pub(super) fn render_acp_call(&self, msg_id: u64, i: usize, c: &ToolCall, cx: &mut Context<Self>) -> Div {
        let t = cx.theme().clone();
        let kind = c.name.trim_start_matches("acp:").to_string();
        if kind == "plan" { return self.render_plan(c, &t); }
        let title = c.input["title"].as_str().unwrap_or("").to_string();
        let title = if title.is_empty() { kind.clone() } else { title };
        let perm = self.acp.as_ref().and_then(|a| a.perm.as_ref()).map(|p| p.msg_id == msg_id && p.call == i).unwrap_or(false);
        let failed = c.result.as_deref().map(|r| r.starts_with("错误")).unwrap_or(false);
        let ic = match kind.as_str() { "execute" => "terminal", "search" => "search", "edit" | "delete" | "move" => "agent", "think" | "other" | "switch_mode" => "alert", _ => "file" };
        let body: Vec<String> = c.result.as_deref().filter(|_| !failed || kind == "execute").map(|r| r.lines().map(str::to_string).collect()).unwrap_or_default();
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
                        else if failed { icon("x", 13., t.del).into_any_element() }
                        else if c.result.is_some() { icon("check", 13., t.add).into_any_element() }
                        else { div().text_color(t.faint).child("…").into_any_element() }),
            )
            .when_some(command.filter(|_| kind == "execute"), |d, cmd| d.child(
                div().flex().gap(px(7.)).px(px(11.)).py(px(6.)).border_t_1().border_color(t.border).bg(t.bg).font_family(MONO_FONT).text_size(px(12.)).line_height(px(18.))
                    .child(div().flex_none().text_color(t.accent).child("$")).child(div().flex_1().min_w_0().text_color(t.text).child(cmd)),
            ))
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

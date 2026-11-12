//! Agent 对话面板：线程、流式回复、@文件引用、代码块一键应用。
use crate::agent_proto::{HistCall, HistMsg, api_messages};
use crate::ai::{self, AgentStreamEvent, Settings};
use crate::tools;
use crate::blocks::{self, CodeBlock, Segment};
use crate::context::{self, build_user_message, find_mentions, strip_context};
use crate::editor::{Editor, EditorEvent, EditorMode};
use crate::fuzzy::fuzzy_filter;
use crate::md::{self, Block};
use crate::syntax;
use crate::theme::{ActiveTheme, MONO_FONT};
use crate::ui::{btn, file_icon, icon, icon_button};
use crate::workspace::Workspace;
use gpui::{prelude::*, *};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub enum AgentEvent {
    Apply { path: Option<String>, code: String },
    OpenSettings,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Debug)]
pub enum Role { User, Assistant }

#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub enum Status { Streaming, Done, Error(String) }

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub result: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Message {
    pub id: u64,
    #[serde(default)]
    pub calls: Vec<ToolCall>,
    pub role: Role,
    pub content: String,
    pub refs: Vec<String>,
    pub status: Status,
    pub applied: HashSet<usize>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Thread {
    pub title: String,
    pub messages: Vec<Message>,
}

impl Thread {
    fn new() -> Self { Self { title: "新对话".into(), messages: vec![] } }
}

fn threads_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var("HOME").ok()?).join("Library/Application Support/Quill/threads.json"))
}

fn load_threads() -> Vec<Thread> {
    threads_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<Vec<Thread>>(&s).ok()).filter(|v| !v.is_empty()).unwrap_or_else(|| vec![Thread::new()])
}

struct Mention { items: Vec<String>, sel: usize }

pub struct AgentPanel {
    workspace: WeakEntity<Workspace>,
    pub composer: Entity<Editor>,
    threads: Vec<Thread>,
    active: usize,
    cancel: Option<Arc<AtomicBool>>,
    scroll: ScrollHandle,
    stick: bool,
    pub files: Vec<String>,
    pub active_file: Option<String>,
    dropped: Vec<String>,
    extra: Vec<String>,
    mention: Option<Mention>,
    menu: bool,
    pub settings: Settings,
    next_id: u64,
    rounds: usize,
    expanded: HashSet<(u64, usize)>,
    /// 代码块语法高亮缓存：(内容哈希) → 区间。流式回复时每个分片都会重绘整个面板，不能每次都重新解析
    hl_cache: std::cell::RefCell<std::collections::HashMap<u64, Vec<(std::ops::Range<usize>, syntax::Kind)>>>,
    _sub: Subscription,
}

impl EventEmitter<AgentEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(workspace: WeakEntity<Workspace>, settings: Settings, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| Editor::new(EditorMode::composer("向 Quill 提问，或描述要做的修改…"), cx));
        let sub = cx.subscribe(&composer, |this, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Submit => this.send(cx),
            EditorEvent::Changed => this.update_mention(cx),
            EditorEvent::Cancel => { if this.cancel.is_some() { this.stop(cx); } }
            _ => {}
        });
        let threads = load_threads();
        let active = 0;
        let next_id = threads.iter().flat_map(|t| t.messages.iter().map(|m| m.id)).max().unwrap_or(0) + 1;
        Self { workspace, composer, threads, active, cancel: None, scroll: ScrollHandle::new(), stick: true, files: vec![], active_file: None, dropped: vec![], extra: vec![], mention: None, menu: false, settings, next_id, rounds: 0, expanded: HashSet::new(), hl_cache: Default::default(), _sub: sub }
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &App) { window.focus(&self.composer.read(cx).focus); }

    fn save(&self) {
        if let Some(p) = threads_path() {
            if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
            let keep: Vec<&Thread> = self.threads.iter().take(40).collect();
            let _ = std::fs::write(p, serde_json::to_string(&keep).unwrap_or_default());
        }
    }

    fn thread(&mut self) -> &mut Thread { &mut self.threads[self.active] }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        if self.threads[self.active].messages.is_empty() { return; }
        self.threads.insert(0, Thread::new());
        self.active = 0;
        self.menu = false;
        cx.notify();
    }

    fn attachments(&self, text: &str) -> Vec<String> {
        let mut v: Vec<String> = vec![];
        if let Some(a) = &self.active_file { if !self.dropped.contains(a) { v.push(a.clone()); } }
        for e in &self.extra { if !v.contains(e) { v.push(e.clone()); } }
        for m in find_mentions(text, &self.files) { if !v.contains(&m) { v.push(m); } }
        v
    }

    fn update_mention(&mut self, cx: &mut Context<Self>) {
        let (text, cur) = { let c = self.composer.read(cx); (c.text(), c.cursor()) };
        let before = &text[..cur.min(text.len())];
        let m = before.rfind('@').and_then(|i| {
            let prev_ok = i == 0 || !before[..i].chars().last().map(|c| c.is_ascii_alphanumeric() || c == '_').unwrap_or(false);
            let q = &before[i + 1..];
            (prev_ok && q.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))).then(|| q.to_string())
        });
        self.mention = m.map(|q| {
            let items = fuzzy_filter(&self.files, &q, |s| s.as_str(), 7).into_iter().map(|(f, _)| f.clone()).collect::<Vec<_>>();
            Mention { items, sel: 0 }
        }).filter(|m| !m.items.is_empty());
        let open = self.mention.is_some();
        self.composer.update(cx, |c, _| c.intercept = open);
        cx.notify();
    }

    fn pick_mention(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.mention.take() else { return };
        let Some(file) = m.items.get(m.sel).cloned() else { return };
        let (text, cur) = { let c = self.composer.read(cx); (c.text(), c.cursor()) };
        if let Some(i) = text[..cur.min(text.len())].rfind('@') {
            let new = format!("{}@{} {}", &text[..i], file, &text[cur.min(text.len())..]);
            let pos = i + 1 + file.len() + 1;
            self.composer.update(cx, |c, cx| { c.set_text(&new, cx); c.move_to(pos, cx); c.intercept = false; });
        }
        if !self.extra.contains(&file) { self.extra.push(file); }
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.cancel.is_some() { return; }
        let text = self.composer.read(cx).text().trim().to_string();
        if text.is_empty() { return; }
        let atts = self.attachments(&text);
        let ws = self.workspace.clone();
        let files: Vec<(String, String)> = atts.iter().filter_map(|rel| {
            let t = ws.update(cx, |w, cx| w.file_text_rel(rel, cx)).ok().flatten()?;
            Some((rel.clone(), t))
        }).collect();
        self.composer.update(cx, |c, cx| { c.clear(cx); c.intercept = false; });
        self.mention = None;
        self.extra.clear();
        let user = Message { id: self.next_id, calls: vec![], role: Role::User, content: build_user_message(&text, &files), refs: files.iter().map(|f| f.0.clone()).collect(), status: Status::Done, applied: HashSet::new() };
        self.next_id += 1;
        let first = self.threads[self.active].messages.is_empty();
        if first { self.thread().title = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(26).collect(); }
        self.thread().messages.push(user);
        self.rounds = 0;
        self.stick = true;
        self.start_round(cx);
    }

    /// 发起一轮模型请求（一次对话里遇到工具调用会连续发起多轮）
    fn start_round(&mut self, cx: &mut Context<Self>) {
        let ai_id = self.next_id;
        self.next_id += 1;
        self.thread().messages.push(Message { id: ai_id, calls: vec![], role: Role::Assistant, content: String::new(), refs: vec![], status: Status::Streaming, applied: HashSet::new() });
        let msgs = &self.threads[self.active].messages;
        let last_user = msgs.iter().rposition(|m| m.role == Role::User);
        let hist: Vec<HistMsg> = msgs.iter().enumerate()
            .filter(|(_, m)| m.id != ai_id && !matches!(m.status, Status::Error(_)))
            .map(|(i, m)| HistMsg {
                role: if m.role == Role::User { "user" } else { "assistant" },
                text: if m.role == Role::User && Some(i) != last_user { strip_context(&m.content).to_string() } else { m.content.clone() },
                calls: m.calls.iter().filter_map(|c| Some(HistCall { id: c.id.clone(), name: c.name.clone(), input: c.input.clone(), result: c.result.clone()? })).collect(),
            })
            .collect();
        let messages = api_messages(&hist);
        let tools_on = self.settings.provider != "openai";
        let system = if tools_on { format!("{}\n{}", context::SYSTEM_PROMPT, context::TOOL_HINT) } else { context::SYSTEM_PROMPT.to_string() };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded();
        let settings = self.settings.clone();
        cx.background_executor().spawn(async move { ai::run_agent_stream(settings, system, messages, tools_on, tx, cancel); }).detach();
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                let end = matches!(ev, AgentStreamEvent::Done(_) | AgentStreamEvent::Error(_));
                if this.update(cx, |p, cx| p.on_event(ai_id, ev, cx)).is_err() { break; }
                if end { break; }
            }
        }).detach();
        cx.notify();
    }

    fn on_event(&mut self, id: u64, ev: AgentStreamEvent, cx: &mut Context<Self>) {
        let msg = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == id);
        let Some(m) = msg else { return };
        let mut run_tools = false;
        match ev {
            AgentStreamEvent::Chunk(c) => m.content.push_str(&c),
            AgentStreamEvent::ToolUse { id, name, input } => m.calls.push(ToolCall { id, name, input, result: None }),
            AgentStreamEvent::Done(stop) => { m.status = Status::Done; self.cancel = None; run_tools = stop == "tool_use" && m.calls.iter().any(|c| c.result.is_none()); self.save(); }
            AgentStreamEvent::Error(e) => { m.status = Status::Error(e); self.cancel = None; self.save(); }
        }
        if self.stick { self.scroll.scroll_to_bottom(); }
        cx.notify();
        if run_tools { self.run_tools(id, cx); }
    }

    /// 执行本轮的工具调用，把结果回填后继续下一轮。edit_file 不直接写盘，交给工作区走审阅。
    fn run_tools(&mut self, msg_id: u64, cx: &mut Context<Self>) {
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        let pending: Vec<(usize, String, serde_json::Value)> = self.threads.iter().flat_map(|t| t.messages.iter()).find(|m| m.id == msg_id)
            .map(|m| m.calls.iter().enumerate().filter(|(_, c)| c.result.is_none()).map(|(i, c)| (i, c.name.clone(), c.input.clone())).collect()).unwrap_or_default();
        for (i, name, input) in pending {
            let result = if name == "edit_file" {
                match (input["path"].as_str(), input["content"].as_str()) {
                    (Some(p), Some(c)) => { cx.emit(AgentEvent::Apply { path: Some(p.to_string()), code: c.to_string() }); "修改已放进编辑器，等待用户审阅（接受或拒绝）。".to_string() }
                    _ => "错误：edit_file 需要 path 和 content".to_string(),
                }
            } else {
                match &root { Some(r) => tools::run(r, &name, &input).unwrap_or_else(|e| format!("错误：{e}")), None => "错误：没有打开项目".to_string() }
            };
            if let Some(c) = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == msg_id).and_then(|m| m.calls.get_mut(i)) { c.result = Some(result); }
        }
        self.rounds += 1;
        if self.rounds >= 8 {
            if let Some(m) = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == msg_id) { m.content.push_str("\n\n（已达到单次对话的工具调用上限，先停在这里。）"); }
            self.save();
            cx.notify();
            return;
        }
        self.start_round(cx);
    }

    /// 应用最近一条回复里还没应用的代码块（⌘⇧↵）
    pub fn apply_latest(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.threads[self.active].messages.iter_mut().rev().find(|m| m.role == Role::Assistant && m.status == Status::Done) else { return };
        let segs = blocks::parse_segments(&m.content);
        let mut todo = vec![];
        for (i, s) in segs.iter().enumerate() {
            if let Segment::Code { block, closed: true } = s { if !m.applied.contains(&i) && (block.path.is_some() || todo.is_empty()) { todo.push((i, block.clone())); } }
        }
        for (i, b) in todo {
            m.applied.insert(i);
            cx.emit(AgentEvent::Apply { path: b.path.clone(), code: b.code.clone() });
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
            for m in self.threads[self.active].messages.iter_mut() { if m.status == Status::Streaming { m.status = Status::Done; } }
            self.save();
            cx.notify();
        }
    }

    fn render_text(&self, text: &str, cx: &App) -> Div {
        let t = cx.theme();
        let mut col = div().flex().flex_col().gap(px(8.));
        for b in md::parse_blocks(text) {
            let inline = |s: &str| -> StyledText {
                let (plain, spans) = md::parse_inline(s);
                let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = spans.into_iter().map(|sp| {
                    let st = if sp.code { HighlightStyle { color: Some(t.accent), background_color: Some(t.hover), ..Default::default() } } else { HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() } };
                    (sp.range, st)
                }).collect();
                StyledText::new(plain).with_highlights(hl)
            };
            col = col.child(match b {
                Block::Heading(_, s) => div().font_weight(FontWeight::BOLD).text_size(px(14.5)).pt(px(4.)).child(inline(&s)),
                Block::Bullet(s) => div().flex().gap(px(8.)).pl(px(4.)).child(div().flex_none().text_color(t.faint).child("•")).child(div().flex_1().min_w_0().child(inline(&s))),
                Block::Numbered(n, s) => div().flex().gap(px(8.)).pl(px(2.)).child(div().flex_none().text_color(t.faint).child(n)).child(div().flex_1().min_w_0().child(inline(&s))),
                Block::Paragraph(s) => div().child(inline(&s)),
            });
        }
        col
    }

    fn render_code(&self, msg_id: u64, ix: usize, block: &CodeBlock, closed: bool, applied: bool, cx: &mut Context<Self>) -> Div {
        let t = cx.theme().clone();
        let lang = syntax::lang_for_name(&block.lang).or_else(|| block.path.as_deref().and_then(syntax::lang_for_path));
        let spans = match lang {
            Some(l) if closed || block.code.len() < 20_000 => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (block.code.as_str(), block.lang.as_str(), block.path.as_deref()).hash(&mut h);
                let key = h.finish();
                let mut cache = self.hl_cache.borrow_mut();
                if cache.len() > 300 { cache.clear(); }
                cache.entry(key).or_insert_with(|| syntax::highlight(l, &block.code)).clone()
            }
            _ => vec![],
        };
        let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = spans.into_iter().map(|(r, k)| (r, HighlightStyle { color: Some(t.syntax(k)), ..Default::default() })).collect();
        let lines = block.code.lines().count();
        let expanded = self.expanded.contains(&(msg_id, ix));
        let long = lines > 16;
        let title = block.path.clone().unwrap_or_else(|| if block.lang.is_empty() { "代码".into() } else { block.lang.clone() });
        let code = block.code.clone();
        let apply_block = block.clone();
        div()
            .flex().flex_col()
            .rounded(px(11.))
            .border_1()
            .border_color(if closed { t.border_strong } else { t.accent_line })
            .bg(t.bg)
            .overflow_hidden()
            .child(
                div().h(px(32.)).px(px(10.)).flex().items_center().gap(px(8.)).bg(t.elev).border_b_1().border_color(t.border).font_family(MONO_FONT).text_size(px(12.)).text_color(t.dim)
                    .when_some(block.path.clone(), |d, p| d.child(file_icon(p.rsplit('/').next().unwrap_or(&p), cx)))
                    .child(div().text_color(t.text).truncate().child(title))
                    .child(div().flex_1())
                    .child(btn(("copy", (msg_id * 100 + ix as u64) as usize), "复制", false, cx).h(px(24.)).px(px(8.)).text_size(px(12.)).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))))
                    .child(if applied {
                        div().flex().items_center().gap(px(4.)).text_color(t.add).font_family(crate::theme::UI_FONT).child(icon("check", 13., t.add)).child("已应用").into_any_element()
                    } else {
                        btn(("apply", (msg_id * 100 + ix as u64) as usize), "应用", true, cx).h(px(24.)).px(px(10.)).text_size(px(12.)).when(!closed, |d| d.opacity(0.4))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !closed { return; }
                                for m in this.threads.iter_mut().flat_map(|t| t.messages.iter_mut()) { if m.id == msg_id { m.applied.insert(ix); } }
                                cx.emit(AgentEvent::Apply { path: apply_block.path.clone(), code: apply_block.code.clone() });
                                cx.notify();
                            })).into_any_element()
                    }),
            )
            .child(
                div().id(("code", (msg_id * 100 + ix as u64) as usize)).px(px(12.)).py(px(9.)).font_family(MONO_FONT).text_size(px(12.)).line_height(px(19.)).text_color(t.text)
                    .max_h(if expanded { px(640.) } else { px(330.) }).overflow_hidden()
                    .child(StyledText::new(block.code.clone()).with_highlights(hl)),
            )
            .when(long, |d| d.child(
                div().id(("more", (msg_id * 100 + ix as u64) as usize)).h(px(26.)).flex().items_center().justify_center().border_t_1().border_color(t.border).text_size(px(12.)).text_color(t.faint).cursor_pointer()
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(move |this, _, _, cx| { if !this.expanded.remove(&(msg_id, ix)) { this.expanded.insert((msg_id, ix)); } cx.notify(); }))
                    .child(if expanded { "收起".to_string() } else { format!("展开全部 {lines} 行") }),
            ))
    }

    fn render_message(&self, m: &Message, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        match m.role {
            Role::User => div().flex().flex_col().items_end().gap(px(6.)).mb(px(18.))
                .when(!m.refs.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(5.)).justify_end().children(m.refs.iter().map(|r| chip(r, cx)))))
                .child(div().max_w(px(340.)).px(px(13.)).py(px(9.)).rounded_tl(px(14.)).rounded_tr(px(14.)).rounded_bl(px(14.)).rounded_br(px(4.)).bg(t.elev).border_1().border_color(t.border).child(strip_context(&m.content).to_string()))
                .into_any_element(),
            Role::Assistant => {
                let streaming = m.status == Status::Streaming;
                let segs = blocks::parse_segments(&m.content);
                let pending: Vec<(usize, CodeBlock)> = segs.iter().enumerate().filter_map(|(i, s)| match s { Segment::Code { block, closed: true } if block.path.is_some() && !m.applied.contains(&i) => Some((i, block.clone())), _ => None }).collect();
                let mid = m.id;
                div().flex().gap(px(11.)).mb(px(20.))
                    .child(div().flex_none().size(px(22.)).mt(px(1.)).rounded(px(7.)).bg(t.accent).flex().items_center().justify_center().child(icon("agent", 13., t.accent_ink)).when(streaming, |d| d.opacity(0.85)))
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().gap(px(8.)).line_height(px(21.))
                            .when(segs.is_empty() && m.calls.is_empty() && streaming, |d| d.child(div().text_color(t.faint).child("思考中…")))
                            .children(segs.iter().enumerate().map(|(i, s)| match s {
                                Segment::Text(tx) => self.render_text(tx, cx).into_any_element(),
                                Segment::Code { block, closed } => self.render_code(mid, i, block, *closed, m.applied.contains(&i), cx).into_any_element(),
                            }))
                            .children(m.calls.iter().map(|c| tool_row(c, cx)))
                            .when(streaming && !segs.is_empty(), |d| d.child(div().w(px(7.)).h(px(14.)).rounded(px(2.)).bg(t.accent)))
                            .when_some(if let Status::Error(e) = &m.status { Some(e.clone()) } else { None }, |d, e| d.child(div().px(px(12.)).py(px(8.)).rounded(px(9.)).bg(t.del_bg).text_color(t.del).child(format!("出错了：{e}"))))
                            .when(!streaming && pending.len() > 1, |d| d.child(
                                div().flex().items_center().gap(px(8.)).px(px(12.)).py(px(8.)).rounded(px(11.)).bg(t.accent_soft).border_1().border_color(t.accent_line)
                                    .child(div().flex_1().child(format!("{} 个文件有修改建议", pending.len())))
                                    .child(btn(("apply-all", mid as usize), "全部应用", true, cx).h(px(24.)).on_click(cx.listener(move |this, _, _, cx| {
                                        for (i, b) in pending.clone() {
                                            for mm in this.threads.iter_mut().flat_map(|t| t.messages.iter_mut()) { if mm.id == mid { mm.applied.insert(i); } }
                                            cx.emit(AgentEvent::Apply { path: b.path.clone(), code: b.code.clone() });
                                        }
                                        cx.notify();
                                    }))),
                            )),
                    )
                    .into_any_element()
            }
        }
    }
}

fn tool_row(c: &ToolCall, cx: &App) -> Div {
    let t = cx.theme();
    let done = c.result.is_some();
    let failed = c.result.as_deref().map(|r| r.starts_with("错误")).unwrap_or(false);
    div().flex().items_center().gap(px(8.)).h(px(28.)).px(px(10.)).rounded(px(8.)).bg(t.hover).border_1().border_color(t.border).text_size(px(12.5)).text_color(t.dim)
        .child(icon(match c.name.as_str() { "search" => "search", "list_dir" => "folder", "edit_file" => "agent", _ => "file" }, 13., t.dim))
        .child(div().flex_1().min_w_0().truncate().child(tools::summary(&c.name, &c.input)))
        .child(if failed { icon("x", 13., t.del).into_any_element() } else if done { icon("check", 13., t.add).into_any_element() } else { div().text_color(t.faint).child("…").into_any_element() })
}

fn chip(path: &str, cx: &App) -> Div {
    let t = cx.theme();
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    div().flex().items_center().gap(px(5.)).h(px(22.)).px(px(8.)).rounded(px(7.)).bg(t.hover).border_1().border_color(t.border).text_size(px(12.)).font_family(MONO_FONT).text_color(t.dim).child(file_icon(&name, cx)).child(name)
}

const SUGGESTS: [&str; 4] = ["解释一下当前文件做了什么", "找出这个文件里潜在的 bug", "为当前文件补充单元测试", "把这段代码重构得更简洁"];

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let streaming = self.cancel.is_some();
        let thread = self.threads[self.active].clone();
        let atts = { let text = self.composer.read(cx).text(); self.attachments(&text) };
        let can_send = !self.composer.read(cx).is_empty() && !streaming;
        let _ = window;
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(t.panel)
            .on_action(cx.listener(|this, _: &crate::actions::MoveUp, _, cx| { if let Some(m) = &mut this.mention { m.sel = (m.sel + m.items.len() - 1) % m.items.len(); cx.notify(); } }))
            .on_action(cx.listener(|this, _: &crate::actions::MoveDown, _, cx| { if let Some(m) = &mut this.mention { m.sel = (m.sel + 1) % m.items.len(); cx.notify(); } }))
            .on_action(cx.listener(|this, _: &crate::actions::Enter, _, cx| this.pick_mention(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::Tab, _, cx| this.pick_mention(cx)))
            .child(
                div().h(px(44.)).flex_none().px(px(12.)).flex().items_center().gap(px(6.)).border_b_1().border_color(t.border)
                    .child(
                        div().id("thread-btn").flex().items_center().gap(px(5.)).px(px(8.)).h(px(28.)).rounded(px(8.)).cursor_pointer().font_weight(FontWeight::SEMIBOLD).max_w(px(250.))
                            .hover(|s| s.bg(t.hover)).on_click(cx.listener(|this, _, _, cx| { this.menu = !this.menu; cx.notify(); }))
                            .child(div().truncate().child(thread.title.clone())).child(icon("chevron_down", 13., t.faint)),
                    )
                    .child(div().flex_1())
                    .child(icon_button("new-thread", "plus", false, cx).on_click(cx.listener(|this, _, _, cx| this.new_thread(cx)))),
            )
            .child(
                div().id("agent-msgs").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).px(px(16.)).pt(px(18.)).pb(px(8.))
                    .on_scroll_wheel(cx.listener(|this, _, _, _| { this.stick = false; }))
                    .when(thread.messages.is_empty(), |d| d.child(
                        div().pt(px(30.)).flex().flex_col().gap(px(8.))
                            .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).child("想改点什么？"))
                            .child(div().text_color(t.dim).mb(px(10.)).child("直接描述需求。用 @ 引用文件，回复里的代码块可以一键应用，并逐块审阅。"))
                            .children(SUGGESTS.iter().enumerate().map(|(i, s)| {
                                div().id(("sg", i)).px(px(13.)).py(px(10.)).rounded(px(11.)).bg(t.elev).border_1().border_color(t.border).text_color(t.dim).cursor_pointer()
                                    .hover(|st| st.border_color(t.accent_line).text_color(t.text))
                                    .on_click(cx.listener(move |this, _, _, cx| { let s = SUGGESTS[i]; this.composer.update(cx, |c, cx| { c.set_text(s, cx); }); }))
                                    .child(*s)
                            }))
                    ))
                    .children(thread.messages.iter().map(|m| self.render_message(m, cx))),
            )
            .child(
                div().flex_none().mx(px(12.)).mb(px(12.)).mt(px(6.)).relative().rounded(px(14.)).bg(t.elev).border_1().border_color(t.border_strong)
                    .when_some(self.mention.as_ref().map(|m| (m.items.clone(), m.sel)), |d, (items, sel)| d.child(
                        div().absolute().left(px(8.)).right(px(8.)).bottom(px(100.)).p(px(5.)).rounded(px(11.)).bg(t.elev).border_1().border_color(t.border_strong)
                            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.) }])
                            .children(items.into_iter().enumerate().map(|(i, f)| {
                                let name = f.rsplit('/').next().unwrap_or(&f).to_string();
                                div().flex().items_center().gap(px(8.)).px(px(9.)).py(px(5.)).rounded(px(7.)).font_family(MONO_FONT).text_size(px(12.)).text_color(if i == sel { t.text } else { t.dim })
                                    .when(i == sel, |d| d.bg(t.accent_soft)).child(file_icon(&name, cx)).child(f)
                            })),
                    ))
                    .when(!atts.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(5.)).px(px(10.)).pt(px(9.)).children(atts.iter().map(|a| chip(a, cx)))))
                    .child(div().px(px(12.)).py(px(8.)).child(self.composer.clone()))
                    .child(
                        div().flex().items_center().gap(px(6.)).px(px(8.)).pb(px(8.)).pl(px(12.)).text_size(px(12.)).text_color(t.faint)
                            .child("↵ 发送 · ⇧↵ 换行")
                            .child(div().flex_1())
                            .child(if streaming {
                                div().id("stop").size(px(28.)).rounded(px(9.)).flex().items_center().justify_center().bg(t.press).cursor_pointer().child(icon("stop", 14., t.text))
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))).into_any_element()
                            } else {
                                div().id("send").size(px(28.)).rounded(px(9.)).flex().items_center().justify_center().bg(t.accent).cursor_pointer().when(!can_send, |d| d.opacity(0.3)).child(icon("send", 15., t.accent_ink))
                                    .on_click(cx.listener(|this, _, _, cx| this.send(cx))).into_any_element()
                            }),
                    ),
            )
            .when(self.menu, |d| d.child(
                div().absolute().inset_0().id("thread-scrim").on_click(cx.listener(|this, _, _, cx| { this.menu = false; cx.notify(); })).child(
                    div().absolute().top(px(42.)).left(px(10.)).w(px(300.)).max_h(px(340.)).p(px(6.)).rounded(px(12.)).bg(t.elev).border_1().border_color(t.border_strong).overflow_hidden()
                        .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.45), offset: point(px(0.), px(14.)), blur_radius: px(40.), spread_radius: px(0.) }])
                        .children(self.threads.iter().enumerate().map(|(i, th)| {
                            div().id(("th", i)).flex().items_center().px(px(10.)).py(px(7.)).rounded(px(8.)).cursor_pointer().gap(px(8.))
                                .text_color(if i == self.active { t.text } else { t.dim }).when(i == self.active, |d| d.bg(t.accent_soft)).hover(|s| s.bg(t.hover))
                                .on_click(cx.listener(move |this, _, _, cx| { this.active = i; this.menu = false; this.stick = true; cx.notify(); }))
                                .child(div().flex_1().truncate().child(th.title.clone()))
                                .child(div().id(("thdel", i)).child(icon("trash", 13., t.faint)).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.threads.remove(i);
                                    if this.threads.is_empty() { this.threads.push(Thread::new()); }
                                    this.active = this.active.min(this.threads.len() - 1);
                                    this.save();
                                    cx.notify();
                                })))
                        })),
                ),
            ))
    }
}

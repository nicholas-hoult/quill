//! Agent 对话面板：线程、流式回复、@文件引用、代码块一键应用。
use crate::agent_proto::{HistCall, HistMsg, api_messages};
use crate::ai::{self, AgentStreamEvent, Settings};
use crate::tools;
use crate::blocks::{self, CodeBlock, Segment};
use crate::context::{self, build_user_message_with, find_mentions, strip_context, Block as CtxBlock};
use crate::mentions::{self, Cand, SymRef, Token};
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
use std::time::Duration;

#[path = "agent_acp.rs"]
mod acp_impl;

pub enum AgentEvent {
    /// `write_through`：先写盘再审阅（Claude Code 后端用，Agent 之后读到的必须是新内容）；否则只放进编辑器等审阅
    Apply { path: Option<String>, code: String, write_through: bool },
    /// Agent 已经自己改好了磁盘上的文件（路径相对项目根）：打开并以改动前的内容为基线进入审阅，拒绝时回写
    ReviewFile { path: String, before: Option<String> },
    OpenSettings,
    /// 在对话面板里切换了模型：工作区据此同步并保存设置
    ModelChanged { model: String, models: Vec<String> },
    /// 在对话面板里切换了 Claude Code 的会话模式（只发常用的、值得记住的）：工作区据此保存设置（#9）
    ModeChanged { mode: String },
    /// 检查点回退：按计划还原 / 删除这些文件（路径相对项目根）
    Restore(Vec<(String, crate::checkpoint::Action)>),
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
    /// 检查点（#5）：这一轮里 Agent 改动过的文件在改动前的内容。只在内存里，不写进 threads.json
    #[serde(skip)]
    pub snapshots: Vec<crate::checkpoint::Snapshot>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Thread {
    pub title: String,
    pub messages: Vec<Message>,
    /// Claude Code（ACP）后端的会话 ID：下次发消息时用 `session/load` 恢复，Agent 就记得之前的上下文（#7）
    #[serde(default)]
    pub acp_session: Option<String>,
}

impl Thread {
    fn new() -> Self { Self { title: "新对话".into(), messages: vec![], acp_session: None } }
}

fn threads_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var("HOME").ok()?).join("Library/Application Support/Quill/threads.json"))
}

fn load_threads() -> Vec<Thread> {
    threads_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str::<Vec<Thread>>(&s).ok()).filter(|v| !v.is_empty()).unwrap_or_else(|| vec![Thread::new()])
}

struct Mention { items: Vec<Cand>, sel: usize }

pub struct AgentPanel {
    workspace: WeakEntity<Workspace>,
    pub composer: Entity<Editor>,
    threads: Vec<Thread>,
    active: usize,
    cancel: Option<Arc<AtomicBool>>,
    /// 消息列表（虚拟化：只构建 / 排版可见的消息）。`msgs_key` 是上次同步给它的数据状态，见 msglist.rs
    msgs: ListState,
    msgs_key: Option<crate::msglist::Key>,
    stick: bool,
    pub files: Vec<String>,
    /// 项目符号索引（#6），后台线程按文件建；`@文件#符号` 的候选和源码行都来自它
    symbols: Vec<SymRef>,
    sym_cache: Arc<std::sync::Mutex<crate::symindex::Cache>>,
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
    /// 当前项目生效的规则文件名（#2），以及它是按哪个项目根读出来的
    rules_file: Option<String>,
    rules_root: Option<std::path::PathBuf>,
    /// 正在等用户确认的工具调用：(消息 id, 调用序号)。同一时刻只会有一个，按顺序逐个确认
    awaiting: Option<(u64, usize)>,
    /// 正在后台运行的命令
    running: Option<(u64, usize)>,
    /// 本会话内「始终允许运行命令」。只存内存，重启即失效
    always_allow: bool,
    /// 模型选择器是否展开，以及键盘选中的项
    model_menu: bool,
    /// Claude Code 会话模式的选择浮层（#9）
    mode_menu: bool,
    model_sel: usize,
    /// 正在确认回退的那条用户消息；以及已经回退过的（只用来显示「已恢复」）
    restoring: Option<u64>,
    restored: HashSet<u64>,
    /// Claude Code（ACP）后端的会话（#7）；每个对话一个，切换 / 新建对话时关掉
    acp: Option<acp_impl::AcpState>,
    acp_epoch: u64,
    _sub: Subscription,
}

#[derive(Clone, Copy, PartialEq)]
enum Decision { Once, Always, Deny }

/// 单次对话里工具调用的轮数上限（跑测试 → 修 → 再跑 需要几轮）
const MAX_ROUNDS: usize = 12;

impl EventEmitter<AgentEvent> for AgentPanel {}

impl AgentPanel {
    pub fn new(workspace: WeakEntity<Workspace>, settings: Settings, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| Editor::new(EditorMode::composer("向 Quill 提问，或描述要做的修改…"), cx));
        let sub = cx.subscribe(&composer, |this, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Submit => this.send(cx),
            EditorEvent::Changed => { this.update_mention(cx); this.acp_prewarm(cx); }
            EditorEvent::Cancel => { if this.restoring.is_some() { this.cancel_restore(cx); } else if this.model_menu { this.close_model_menu(cx); } else if this.mode_menu { this.mode_menu = false; cx.notify(); } else if this.mention.is_some() { this.mention = None; this.composer.update(cx, |c, _| c.intercept = false); cx.notify(); } else if this.awaiting.is_some() || this.acp_perm_pending() { this.decide(Decision::Deny, cx); } else if this.cancel.is_some() { this.stop(cx); } }
            _ => {}
        });
        let threads = load_threads();
        let active = 0;
        let next_id = threads.iter().flat_map(|t| t.messages.iter().map(|m| m.id)).max().unwrap_or(0) + 1;
        Self { workspace, composer, threads, active, cancel: None, msgs: ListState::new(0, ListAlignment::Top, px(500.)), msgs_key: None, stick: true, files: vec![], symbols: vec![], sym_cache: Default::default(), active_file: None, dropped: vec![], extra: vec![], mention: None, menu: false, settings, next_id, rounds: 0, expanded: HashSet::new(), hl_cache: Default::default(), rules_file: None, rules_root: None, awaiting: None, running: None, always_allow: false, model_menu: false, mode_menu: false, model_sel: 0, restoring: None, restored: HashSet::new(), acp: None, acp_epoch: 0, _sub: sub }
    }

    /// 项目文件列表更新了：记下来，并在后台线程里（增量）重建符号索引
    pub fn set_files(&mut self, files: Vec<String>, root: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.files = files.clone();
        let Some(root) = root else { return };
        self.acp_prewarm_soon(cx);
        let cache = self.sym_cache.clone();
        cx.spawn(async move |this, cx| {
            let syms = cx.background_executor().spawn(async move {
                let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
                crate::symindex::build(&root, &files, &mut c)
            }).await;
            let _ = this.update(cx, |p, cx| { p.symbols = syms; cx.notify(); });
        }).detach();
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &App) { window.focus(&self.composer.read(cx).focus); }

    fn save(&self) {
        if let Some(p) = threads_path() {
            if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
            let keep: Vec<&Thread> = self.threads.iter().take(40).collect();
            let _ = std::fs::write(p, serde_json::to_string(&keep).unwrap_or_default());
        }
    }

    // ---------- 检查点（#5） ----------

    /// 这条回复属于哪一轮：它之前最近的一条用户消息（用户消息自己就是自己的一轮）
    fn owner_user(&self, msg_id: u64) -> Option<u64> {
        let t = self.threads.iter().find(|t| t.messages.iter().any(|m| m.id == msg_id))?;
        let i = t.messages.iter().position(|m| m.id == msg_id)?;
        t.messages[..=i].iter().rev().find(|m| m.role == Role::User).map(|m| m.id)
    }

    /// 应用 Agent 的改动前，先把目标文件现在的内容记到所属那一轮的检查点里，再发出 Apply
    fn emit_apply(&mut self, msg_id: u64, path: Option<String>, code: String, cx: &mut Context<Self>) { self.emit_apply_ex(msg_id, path, code, false, cx) }

    fn emit_apply_ex(&mut self, msg_id: u64, path: Option<String>, code: String, write_through: bool, cx: &mut Context<Self>) {
        if let (Some(rel), Some(owner)) = (path.clone().or_else(|| self.active_file.clone()), self.owner_user(msg_id)) {
            let before = self.workspace.update(cx, |w, cx| {
                let root = w.root_path();
                let text = w.file_text_rel(&rel, cx);
                // 读不到内容但文件存在（不是文本 / 没权限）：不能当成「新建」记下，否则回退会把它删掉
                // 路径非法（会被 apply_code 拒绝）的不记检查点
                let legal = root.as_ref().map(|r| crate::tools::apply_target(r, &rel).is_ok()).unwrap_or(false);
                let exists = root.as_ref().and_then(|r| crate::tools::apply_target(r, &rel).ok()).map(|p| p.exists()).unwrap_or(true);
                (if legal { text } else { None }, exists || !legal)
            }).ok();
            if let Some((text, exists)) = before {
                if text.is_some() || !exists {
                    if let Some(m) = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == owner) {
                        crate::checkpoint::record(&mut m.snapshots, &rel, text);
                    }
                }
            }
        }
        cx.emit(AgentEvent::Apply { path, code, write_through });
    }

    /// 回退到某条用户消息之前要做的事：这条消息及之后各轮改过的文件，每个取最早的改动前内容
    fn restore_plan(&self, user_id: u64) -> Vec<(String, crate::checkpoint::Action)> {
        let Some(t) = self.threads.iter().find(|t| t.messages.iter().any(|m| m.id == user_id)) else { return vec![] };
        let Some(start) = t.messages.iter().position(|m| m.id == user_id) else { return vec![] };
        let turns: Vec<&[crate::checkpoint::Snapshot]> = t.messages[start..].iter().filter(|m| m.role == Role::User).map(|m| m.snapshots.as_slice()).collect();
        crate::checkpoint::plan(&turns)
    }

    fn busy(&self) -> bool { self.cancel.is_some() || self.awaiting.is_some() || self.acp_perm_pending() }

    fn begin_restore(&mut self, user_id: u64, cx: &mut Context<Self>) {
        if self.busy() || self.restore_plan(user_id).is_empty() { return; }
        self.model_menu = false;
        self.mention = None;
        self.restoring = Some(user_id);
        self.composer.update(cx, |c, _| c.intercept = true);
        cx.notify();
    }

    /// 命令面板：对最近一条改过文件的用户消息展开回退确认
    pub fn restore_latest(&mut self, cx: &mut Context<Self>) {
        let last = self.threads[self.active].messages.iter().rev().find(|m| m.role == Role::User && !m.snapshots.is_empty()).map(|m| m.id);
        if let Some(id) = last { self.begin_restore(id, cx); }
    }

    fn cancel_restore(&mut self, cx: &mut Context<Self>) {
        self.restoring = None;
        self.composer.update(cx, |c, _| c.intercept = false);
        cx.notify();
    }

    fn confirm_restore(&mut self, cx: &mut Context<Self>) {
        let Some(uid) = self.restoring.take() else { return };
        self.composer.update(cx, |c, _| c.intercept = false);
        let plan = self.restore_plan(uid);
        if plan.is_empty() { cx.notify(); return; }
        // 回退之后，这条消息及之后各轮的快照描述的状态已不存在，清掉
        if let Some(t) = self.threads.iter_mut().find(|t| t.messages.iter().any(|m| m.id == uid)) {
            if let Some(start) = t.messages.iter().position(|m| m.id == uid) { for m in t.messages[start..].iter_mut() { m.snapshots.clear(); } }
        }
        self.restored.insert(uid);
        cx.emit(AgentEvent::Restore(plan));
        cx.notify();
    }

    /// 用户消息下方：「恢复到这条消息之前」按钮 / 确认卡片 / 「已恢复」标记
    fn restore_row(&self, m: &Message, cx: &mut Context<Self>) -> Option<AnyElement> {
        if m.role != Role::User { return None; }
        let t = cx.theme().clone();
        let uid = m.id;
        if self.restoring == Some(uid) {
            let plan = self.restore_plan(uid);
            let n = plan.len();
            let rows = plan.iter().take(6).map(|(p, a)| {
                let delete = matches!(a, crate::checkpoint::Action::Delete);
                div().flex().items_center().gap(px(8.)).h(px(22.))
                    .child(div().flex_none().px(px(6.)).h(px(17.)).flex().items_center().rounded(px(5.)).text_size(px(10.5)).bg(if delete { t.del_bg } else { t.press }).text_color(if delete { t.del } else { t.dim }).child(if delete { "删除" } else { "还原" }))
                    .child(div().flex_1().min_w_0().truncate().font_family(MONO_FONT).text_size(px(12.)).text_color(t.text).child(p.clone()))
            }).collect::<Vec<_>>();
            return Some(
                div().w_full().flex().flex_col().rounded(px(11.)).border_1().border_color(t.accent_line).bg(t.bg).overflow_hidden()
                    .child(
                        div().h(px(32.)).px(px(10.)).flex().items_center().gap(px(8.)).bg(t.elev).border_b_1().border_color(t.border).text_size(px(12.))
                            .child(icon("undo", 14., t.accent))
                            .child(div().text_color(t.text).font_weight(FontWeight::MEDIUM).child("恢复到这条消息之前"))
                            .child(div().flex_1())
                            .child(div().text_color(t.faint).child(format!("{n} 个文件"))),
                    )
                    .child(div().flex().flex_col().px(px(11.)).pt(px(8.)).children(rows).when(n > 6, |d| d.child(div().h(px(22.)).flex().items_center().text_size(px(12.)).text_color(t.faint).child(format!("…还有 {} 个", n - 6)))))
                    .child(div().px(px(11.)).pt(px(6.)).pb(px(9.)).text_size(px(12.)).line_height(px(18.)).text_color(t.dim).child("这些文件会被还原并保存，包括你之后手动改过的部分；新建的文件会被删除。编辑器里可以用 ⌘Z 撤销。"))
                    .child(
                        div().flex().items_center().justify_end().gap(px(8.)).px(px(10.)).py(px(8.)).border_t_1().border_color(t.border).bg(t.elev)
                            .child(btn(("restore-no", uid as usize), "取消  Esc", false, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.cancel_restore(cx))))
                            .child(btn(("restore-yes", uid as usize), "恢复  ↵", true, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.confirm_restore(cx)))),
                    )
                    .into_any_element(),
            );
        }
        if !m.snapshots.is_empty() && !self.busy() {
            let n = self.restore_plan(uid).len();
            let (faint, dim, hover) = (t.faint, t.dim, t.hover);
            return Some(
                div().id(("restore", uid as usize)).flex().items_center().gap(px(5.)).h(px(22.)).px(px(7.)).rounded(px(6.)).cursor_pointer().text_size(px(11.5)).text_color(faint)
                    .hover(move |s| s.bg(hover).text_color(dim))
                    .on_click(cx.listener(move |this, _, _, cx| this.begin_restore(uid, cx)))
                    .child(icon("undo", 12., faint))
                    .child(format!("恢复到这条消息之前 · {n} 个文件"))
                    .into_any_element(),
            );
        }
        if self.restored.contains(&uid) {
            return Some(div().flex().items_center().gap(px(5.)).h(px(22.)).px(px(7.)).text_size(px(11.5)).text_color(t.faint).child(icon("check", 12., t.faint)).child("已恢复到这条消息之前").into_any_element());
        }
        None
    }

    fn model_choices(&self) -> Vec<crate::models::ModelChoice> {
        crate::models::choices(&self.settings.provider, &self.settings.model, &self.settings.models)
    }

    /// 打开 / 关闭模型选择器。演示模式没有可选的模型，直接带去设置页填 Key。
    pub fn toggle_model_menu(&mut self, cx: &mut Context<Self>) {
        if self.model_menu { self.close_model_menu(cx); return; }
        let choices = self.model_choices();
        if choices.is_empty() { cx.emit(AgentEvent::OpenSettings); return; }
        self.model_sel = choices.iter().position(|c| c.id == self.settings.model.trim()).unwrap_or(0);
        self.model_menu = true;
        self.mention = None;
        self.composer.update(cx, |c, _| c.intercept = true);
        cx.notify();
    }

    fn close_model_menu(&mut self, cx: &mut Context<Self>) {
        self.model_menu = false;
        self.composer.update(cx, |c, _| c.intercept = false);
        cx.notify();
    }

    fn move_model_sel(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.model_choices().len();
        if n == 0 { return; }
        self.model_sel = (self.model_sel as isize + delta).rem_euclid(n as isize) as usize;
        cx.notify();
    }

    /// 选中一个模型：立即生效（之后发出的请求用它），并写入设置
    fn choose_model(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(c) = self.model_choices().get(i).cloned() else { return };
        self.settings.model = c.id.clone();
        crate::models::remember(&mut self.settings.models, &c.id);
        cx.emit(AgentEvent::ModelChanged { model: c.id, models: self.settings.models.clone() });
        self.close_model_menu(cx);
    }

    /// 项目换了就重新找规则文件（渲染时调用，只在项目根变化时读盘）。
    /// 渲染中父视图可能正被占用，读不到就跳过，下一帧再试。
    fn sync_rules(&mut self, cx: &mut Context<Self>) {
        let Ok(root) = self.workspace.read_with(cx, |w, _| w.root_path()) else { return };
        if root == self.rules_root { return; }
        self.rules_file = root.as_ref().and_then(|r| crate::rules::load(r)).map(|r| r.file);
        self.rules_root = root;
    }

    fn thread(&mut self) -> &mut Thread { &mut self.threads[self.active] }

    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.abort_pending(cx);
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

    /// 其他对话的标题（`@对话[…]` 的候选）；空对话不算
    fn other_thread_titles(&self) -> Vec<String> {
        self.threads.iter().enumerate().filter(|(i, t)| *i != self.active && !t.messages.is_empty()).map(|(_, t)| t.title.clone()).collect()
    }

    fn update_mention(&mut self, cx: &mut Context<Self>) {
        let (text, cur) = { let c = self.composer.read(cx); (c.text(), c.cursor()) };
        // 行首的 `/`：Claude Code 的斜杠命令（#10）；否则看光标前有没有 `@` 引用
        let cmds: Option<Vec<Cand>> = crate::acp::command_query(&text, cur).zip(self.acp.as_ref()).map(|(q, a)| crate::acp::filter_commands(&a.commands, q, 8).into_iter().map(Cand::Command).collect());
        if let Some(items) = cmds {
            self.mention = Some(Mention { items, sel: 0 }).filter(|m| !m.items.is_empty());
            let open = self.mention.is_some();
            self.composer.update(cx, |c, _| c.intercept = open);
            cx.notify();
            return;
        }
        let before = &text[..cur.min(text.len())];
        let q = before.rfind('@').and_then(|i| {
            let prev_ok = i == 0 || !before[..i].chars().last().map(|c| c.is_ascii_alphanumeric() || c == '_' || c == '@').unwrap_or(false);
            let q = &before[i + 1..];
            (prev_ok && mentions::query_open(q)).then(|| q.to_string())
        });
        self.mention = q.map(|q| Mention { items: mentions::candidates(&q, &self.files, &self.symbols, &self.other_thread_titles()), sel: 0 }).filter(|m| !m.items.is_empty());
        let open = self.mention.is_some();
        self.composer.update(cx, |c, _| c.intercept = open);
        cx.notify();
    }

    fn pick_mention(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.mention.take() else { return };
        let Some(cand) = m.items.get(m.sel).cloned() else { return };
        if let Cand::Command(c) = &cand {
            // 斜杠命令：把行首的 `/词` 换成 `/名字 `
            let text = self.composer.read(cx).text();
            let (new, pos) = crate::acp::apply_command(&text, &c.name);
            self.composer.update(cx, |e, cx| { e.set_text(&new, cx); e.move_to(pos, cx); e.intercept = false; });
            cx.notify();
            return;
        }
        let ins = cand.insert_text();
        let (text, cur) = { let c = self.composer.read(cx); (c.text(), c.cursor()) };
        if let Some(i) = text[..cur.min(text.len())].rfind('@') {
            let new = format!("{}@{} {}", &text[..i], ins, &text[cur.min(text.len())..]);
            let pos = i + 1 + ins.len() + 1;
            self.composer.update(cx, |c, cx| { c.set_text(&new, cx); c.move_to(pos, cx); c.intercept = false; });
        }
        if let Cand::File(file) = cand { if !self.extra.contains(&file) { self.extra.push(file); } }
        cx.notify();
    }

    /// 文字里的 `@符号` / `@对话[…]` 引用整理成给模型的资料块；`@网页` 要联网，单独返回地址由调用方抓取。
    /// 返回 (资料块, 输入框下方 / 消息上方显示的引用标签, 待抓取的网址)
    fn collect_refs(&mut self, text: &str, cx: &mut Context<Self>) -> (Vec<CtxBlock>, Vec<String>, Vec<String>) {
        let (mut blocks, mut refs, mut urls) = (vec![], vec![], vec![]);
        for t in mentions::parse(text) {
            match t {
                Token::Symbol { path, name } => {
                    let Some(rel) = mentions::resolve_path(&path, &self.files).cloned() else { continue };
                    let Some(sym) = self.symbols.iter().find(|s| s.path == rel && s.name == name).cloned() else { continue };
                    let Some(src) = self.workspace.update(cx, |w, cx| w.file_text_rel(&rel, cx)).ok().flatten() else { continue };
                    let (code, start, end) = mentions::symbol_snippet(&src, sym.line, sym.end_line, 300);
                    blocks.push(CtxBlock::Symbol { path: rel.clone(), name: name.clone(), start, end, code });
                    refs.push(format!("{rel}#{name}"));
                }
                Token::Thread(title) => {
                    let Some(th) = self.threads.iter().enumerate().find(|(i, t)| *i != self.active && t.title == title).map(|(_, t)| t) else { continue };
                    let turns: Vec<(bool, String)> = th.messages.iter().filter(|m| !matches!(m.status, Status::Error(_))).map(|m| (m.role == Role::User, if m.role == Role::User { strip_context(&m.content).to_string() } else { m.content.clone() })).collect();
                    blocks.push(CtxBlock::Thread { title: title.clone(), text: mentions::thread_text(&turns, 8000) });
                    refs.push(format!("对话[{title}]"));
                }
                Token::Url(u) => { urls.push(u.clone()); refs.push(u); }
                Token::Path(_) => {} // 文件走 attachments
            }
        }
        (blocks, refs, urls)
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.cancel.is_some() || self.awaiting.is_some() { return; }
        let text = self.composer.read(cx).text().trim().to_string();
        if text.is_empty() { return; }
        // 斜杠命令（#10）：原样发出，不附当前文件 / 引用，否则它们会被当成命令的参数
        let slash = self.settings.provider == "claude-code" && crate::acp::is_slash_command(&text, self.acp.as_ref().map(|a| a.commands.as_slice()).unwrap_or(&[]));
        let atts = if slash { vec![] } else { self.attachments(&text) };
        let ws = self.workspace.clone();
        let files: Vec<(String, String)> = atts.iter().filter_map(|rel| {
            let t = ws.update(cx, |w, cx| w.file_text_rel(rel, cx)).ok().flatten()?;
            Some((rel.clone(), t))
        }).collect();
        let (extra_blocks, extra_refs, urls) = if slash { (vec![], vec![], vec![]) } else { self.collect_refs(&text, cx) };
        let mut blocks: Vec<CtxBlock> = files.iter().map(|(p, c)| CtxBlock::File { path: p.clone(), content: c.clone() }).collect();
        blocks.extend(extra_blocks);
        let mut refs: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
        refs.extend(extra_refs);
        self.composer.update(cx, |c, cx| { c.clear(cx); c.intercept = false; });
        self.mention = None;
        self.extra.clear();
        // 有网页引用：先把用户消息显示出来，后台抓取完成后再把资料附上并发出（抓取失败不阻止发送）
        let content = if urls.is_empty() { build_user_message_with(&text, &blocks) } else { text.clone() };
        let user = Message { id: self.next_id, calls: vec![], role: Role::User, content, refs, status: Status::Done, applied: HashSet::new(), snapshots: vec![] };
        let user_id = user.id;
        self.next_id += 1;
        let first = self.threads[self.active].messages.is_empty();
        if first { self.thread().title = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(26).collect(); }
        self.thread().messages.push(user);
        self.rounds = 0;
        self.stick = true;
        if urls.is_empty() { self.dispatch_round(cx); } else { self.fetch_then_send(user_id, text, blocks, urls, cx); }
    }

    /// 把最后一条用户消息发给当前后端
    fn dispatch_round(&mut self, cx: &mut Context<Self>) {
        if self.settings.provider == "claude-code" {
            let prompt = self.threads[self.active].messages.iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default();
            self.acp_send(prompt, cx);
        } else {
            self.start_round(cx);
        }
    }

    /// 后台抓取 `@网页`，完成后把内容作为 `<web>` 资料附上再发送。失败的网址把原因写给模型，消息照常发出。
    fn fetch_then_send(&mut self, user_id: u64, text: String, blocks: Vec<CtxBlock>, urls: Vec<String>, cx: &mut Context<Self>) {
        let ph_id = self.next_id;
        self.next_id += 1;
        let shown = if urls.len() == 1 { "正在获取网页…".to_string() } else { format!("正在获取 {} 个网页…", urls.len()) };
        self.thread().messages.push(Message { id: ph_id, calls: vec![], role: Role::Assistant, content: shown, refs: vec![], status: Status::Streaming, applied: HashSet::new(), snapshots: vec![] });
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel = Some(flag.clone());
        let (tx, rx) = async_channel::bounded::<Vec<(String, Result<String, String>)>>(1);
        cx.background_executor().spawn(async move {
            let out: Vec<_> = urls.into_iter().map(|u| { let r = crate::web::fetch(&u, crate::web::MAX_BYTES, crate::web::TIMEOUT); (u, r) }).collect();
            let _ = tx.send_blocking(out);
        }).detach();
        cx.spawn(async move |this, cx| {
            let Ok(results) = rx.recv().await else { return };
            let _ = this.update(cx, |p, cx| p.fetch_done(user_id, ph_id, text, blocks, results, flag, cx));
        }).detach();
        cx.notify();
    }

    fn fetch_done(&mut self, user_id: u64, ph_id: u64, text: String, mut blocks: Vec<CtxBlock>, results: Vec<(String, Result<String, String>)>, flag: Arc<AtomicBool>, cx: &mut Context<Self>) {
        if flag.load(Ordering::Relaxed) { return; } // 用户在抓取期间点了停止
        for (url, r) in results {
            let content = match r { Ok(t) if t.trim().is_empty() => "（页面没有可读的文字内容）".to_string(), Ok(t) => t, Err(e) => format!("（获取失败：{e}）") };
            blocks.push(CtxBlock::Web { url, content });
        }
        let full = build_user_message_with(&text, &blocks);
        for t in self.threads.iter_mut() {
            t.messages.retain(|m| m.id != ph_id);
            if let Some(m) = t.messages.iter_mut().find(|m| m.id == user_id) { m.content = full.clone(); }
        }
        self.cancel = None;
        self.dispatch_round(cx);
    }

    /// 发起一轮模型请求（一次对话里遇到工具调用会连续发起多轮）
    fn start_round(&mut self, cx: &mut Context<Self>) {
        let ai_id = self.next_id;
        self.next_id += 1;
        self.thread().messages.push(Message { id: ai_id, calls: vec![], role: Role::Assistant, content: String::new(), refs: vec![], status: Status::Streaming, applied: HashSet::new(), snapshots: vec![] });
        let msgs = &self.threads[self.active].messages;
        let last_user = msgs.iter().rposition(|m| m.role == Role::User);
        let hist: Vec<HistMsg> = msgs.iter().enumerate()
            .filter(|(_, m)| m.id != ai_id && !matches!(m.status, Status::Error(_)))
            .map(|(i, m)| HistMsg {
                role: if m.role == Role::User { "user" } else { "assistant" },
                text: if m.role == Role::User && Some(i) != last_user { strip_context(&m.content).to_string() } else { m.content.clone() },
                calls: m.calls.iter().filter(|c| !c.name.starts_with("acp:")).filter_map(|c| Some(HistCall { id: c.id.clone(), name: c.name.clone(), input: c.input.clone(), result: c.result.clone()? })).collect(),
            })
            .collect();
        let messages = api_messages(&hist);
        let tools_on = self.settings.provider != "openai";
        let system = if tools_on { format!("{}\n{}", context::SYSTEM_PROMPT, context::TOOL_HINT) } else { context::SYSTEM_PROMPT.to_string() };
        // 项目规则文件（#2）：每轮重新读，用户改了规则下一轮就生效
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        let rules = root.as_ref().and_then(|r| crate::rules::load(r));
        self.rules_file = rules.as_ref().map(|r| r.file.clone());
        self.rules_root = root;
        let system = crate::rules::with_rules(&system, rules.as_ref());
        if std::env::var("QUILL_DEBUG_SYSTEM").is_ok() { eprintln!("[system]\n{system}\n[/system]"); }
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
        if self.stick { self.scroll_end(); }
        cx.notify();
        if run_tools { self.run_tools(id, cx); }
    }

    fn set_result(&mut self, msg_id: u64, i: usize, result: String) {
        if let Some(c) = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == msg_id).and_then(|m| m.calls.get_mut(i)) { c.result = Some(result); }
    }

    /// 逐个执行本轮的工具调用，全部有结果后回填并继续下一轮（#3）。
    /// 读类工具同步执行；edit_file 交给工作区走审阅；run_command 要先经用户确认，并在后台线程运行——
    /// 这两种情况会在这里 return，等用户点选 / 命令跑完后由 `decide` / `command_done` 回到这里继续。
    fn run_tools(&mut self, msg_id: u64, cx: &mut Context<Self>) {
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        loop {
            let next = self.threads.iter().flat_map(|t| t.messages.iter()).find(|m| m.id == msg_id)
                .and_then(|m| m.calls.iter().enumerate().find(|(_, c)| c.result.is_none()).map(|(i, c)| (i, c.name.clone(), c.input.clone())));
            let Some((i, name, input)) = next else { break };
            let result = match name.as_str() {
                "edit_file" => match (input["path"].as_str(), input["content"].as_str()) {
                    // 路径不合法（比如含 ..）要如实告诉模型，让它改正，而不是假装成功
                    (Some(p), Some(_)) if root.as_ref().map(|r| tools::apply_target(r, p).is_err()).unwrap_or(false) => {
                        format!("错误：{}", tools::apply_target(root.as_ref().unwrap(), p).unwrap_err())
                    }
                    (Some(p), Some(c)) => { self.emit_apply(msg_id, Some(p.to_string()), c.to_string(), cx); "修改已放进编辑器，等待用户审阅（接受或拒绝）。".to_string() }
                    _ => "错误：edit_file 需要 path 和 content".to_string(),
                },
                "diagnostics" => match &root {
                    Some(r) => {
                        let files = self.workspace.read_with(cx, |w, _| w.all_diags()).unwrap_or_default();
                        tools::format_diagnostics(r, &files, input["path"].as_str().filter(|p| !p.trim().is_empty()))
                    }
                    None => "错误：没有打开项目".to_string(),
                },
                "run_command" => {
                    if input["command"].as_str().map(|c| c.trim().is_empty()).unwrap_or(true) { "错误：run_command 需要 command".to_string() }
                    else if root.is_none() { "错误：没有打开项目".to_string() }
                    else if tools::needs_permission(&name) && !self.always_allow {
                        self.awaiting = Some((msg_id, i));
                        if self.stick { self.scroll_end(); }
                        cx.notify();
                        return;
                    } else { self.spawn_command(msg_id, i, cx); return; }
                }
                _ => match &root { Some(r) => tools::run(r, &name, &input).unwrap_or_else(|e| format!("错误：{e}")), None => "错误：没有打开项目".to_string() },
            };
            self.set_result(msg_id, i, result);
        }
        self.rounds += 1;
        if self.rounds >= MAX_ROUNDS {
            if let Some(m) = self.threads.iter_mut().flat_map(|t| t.messages.iter_mut()).find(|m| m.id == msg_id) { m.content.push_str("\n\n（已达到单次对话的工具调用上限，先停在这里。）"); }
            self.save();
            cx.notify();
            return;
        }
        self.start_round(cx);
    }

    /// 用户对确认卡片的选择
    fn decide(&mut self, d: Decision, cx: &mut Context<Self>) {
        if self.acp_perm_pending() { self.acp_decide(d, cx); return; }
        let Some((msg_id, i)) = self.awaiting.take() else { return };
        match d {
            Decision::Deny => {
                self.set_result(msg_id, i, "用户拒绝运行这条命令。不要重复请求同一条命令：换一种办法，或向用户说明为什么需要它。".into());
                self.run_tools(msg_id, cx);
            }
            Decision::Once => self.spawn_command(msg_id, i, cx),
            Decision::Always => { self.always_allow = true; self.spawn_command(msg_id, i, cx); }
        }
        cx.notify();
    }

    /// 在后台线程运行命令，界面不卡；停止按钮通过取消标志杀掉进程
    fn spawn_command(&mut self, msg_id: u64, i: usize, cx: &mut Context<Self>) {
        let root = self.workspace.update(cx, |w, _| w.root_path()).ok().flatten();
        let command = self.threads.iter().flat_map(|t| t.messages.iter()).find(|m| m.id == msg_id)
            .and_then(|m| m.calls.get(i)).and_then(|c| c.input["command"].as_str().map(str::to_string)).unwrap_or_default();
        let Some(root) = root else { self.set_result(msg_id, i, "错误：没有打开项目".into()); self.run_tools(msg_id, cx); return };
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel = Some(flag.clone());
        self.running = Some((msg_id, i));
        let (tx, rx) = async_channel::bounded::<String>(1);
        let f = flag.clone();
        cx.background_executor().spawn(async move {
            let r = tools::exec(&root, &command, Duration::from_secs(60), &f).unwrap_or_else(|e| format!("错误：{e}"));
            let _ = tx.send_blocking(r);
        }).detach();
        cx.spawn(async move |this, cx| {
            if let Ok(r) = rx.recv().await { let _ = this.update(cx, |p, cx| p.command_done(msg_id, i, r, flag, cx)); }
        }).detach();
        if self.stick { self.scroll_end(); }
        cx.notify();
    }

    fn command_done(&mut self, msg_id: u64, i: usize, result: String, flag: Arc<AtomicBool>, cx: &mut Context<Self>) {
        self.running = None;
        let cancelled = flag.load(Ordering::Relaxed);
        if !cancelled { self.cancel = None; } // 被取消时 stop() 已经把 cancel 取走了
        self.set_result(msg_id, i, result);
        if cancelled { self.save(); cx.notify(); return; }
        self.run_tools(msg_id, cx);
    }

    /// 切换 / 新建 / 删除对话前，先结束还在等确认或运行中的命令，避免确认卡片留在看不见的对话里
    fn abort_pending(&mut self, cx: &mut Context<Self>) {
        self.mode_menu = false;
        if self.awaiting.is_some() || self.running.is_some() || self.acp.is_some() { self.stop(cx); }
        self.acp_reset();
        // 每个对话一个 ACP 会话，进程刚被关掉：稍后给最终选中的那个对话预热（#8），别让下一条消息从头等
        self.acp_prewarm_soon(cx);
    }

    /// 应用最近一条回复里还没应用的代码块（⌘⇧↵）
    pub fn apply_latest(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.threads[self.active].messages.iter_mut().rev().find(|m| m.role == Role::Assistant && m.status == Status::Done) else { return };
        let mid = m.id;
        let segs = blocks::parse_segments(&m.content);
        let mut todo = vec![];
        for (i, s) in segs.iter().enumerate() {
            if let Segment::Code { block, closed: true } = s { if !m.applied.contains(&i) && (block.path.is_some() || todo.is_empty()) { todo.push((i, block.clone())); } }
        }
        for (i, _) in &todo { m.applied.insert(*i); }
        for (_, b) in todo { self.emit_apply(mid, b.path.clone(), b.code.clone(), cx); }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.acp_stop();
        if let Some((mid, i)) = self.awaiting.take() { self.set_result(mid, i, "用户中止了这次对话，命令没有运行。".into()); }
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
            for m in self.threads[self.active].messages.iter_mut() { if m.status == Status::Streaming { m.status = Status::Done; } }
        }
        self.save();
        cx.notify();
    }

    /// 把消息数据的变化告诉列表（只在变了才动它）；贴底状态下重置后回到底部
    fn sync_msgs(&mut self, _window: &Window) {
        use crate::msglist::{plan, Key, Sync};
        let th = &self.threads[self.active];
        let key = Key { thread: self.active, len: th.messages.len(), first: th.messages.first().map_or(0, |m| m.id), last: th.messages.last().map_or(0, |m| m.id) };
        match plan(self.msgs_key, key) {
            Sync::Keep => {}
            Sync::Append(n) => { let at = self.msgs.item_count(); self.msgs.splice(at..at, n); }
            Sync::Reset => self.msgs.reset(key.len),
        }
        self.msgs_key = Some(key);
        // 贴底时每次渲染都回到底部：流式输出让最后一条变高、权限卡片展开等都不经过「消息数变化」，不能只靠事件触发
        if self.stick { self.scroll_end(); }
    }

    /// 滚到最后一条消息的末尾（下一次布局时按最新量到的高度生效）
    fn scroll_end(&self) {
        self.msgs.scroll_to(ListOffset { item_ix: self.msgs.item_count(), offset_in_item: px(0.) });
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
                                this.emit_apply(msg_id, apply_block.path.clone(), apply_block.code.clone(), cx);
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

    /// 输入框右下角的模型 chip：当前模型的简短名称；演示模式显示「演示模式」，点击去设置
    fn model_chip(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let cc = self.settings.provider == "claude-code";
        let mock = self.settings.provider == "mock" || cc;
        let name = if cc { "Claude Code".to_string() } else if mock { "演示模式".to_string() } else { crate::models::label(&self.settings.model) };
        let (hover, dim, faint, text) = (t.hover, t.dim, t.faint, t.text);
        div().id("model-chip").flex().items_center().gap(px(4.)).h(px(26.)).px(px(8.)).mr(px(4.)).rounded(px(8.)).cursor_pointer().text_size(px(12.)).text_color(dim)
            .hover(move |s| s.bg(hover).text_color(text))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_model_menu(cx)))
            .child(div().max_w(px(150.)).truncate().child(name))
            .when(!mock, |d| d.child(icon("chevron_down", 12., faint)))
    }

    /// 打开 / 关闭模式浮层（chip 点击和命令面板共用）；没有可选的模式就不开
    pub fn toggle_mode_menu(&mut self, cx: &mut Context<Self>) {
        self.model_menu = false;
        self.mode_menu = !self.mode_menu && self.acp.as_ref().map(|a| a.modes.is_some()).unwrap_or(false);
        cx.notify();
    }

    /// 输入框右下角的模式 chip（Claude Code 后端、会话建好且 Agent 声明了模式时才有）：当前模式名，点击展开选择（#9）
    fn mode_chip(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let m = self.acp.as_ref().and_then(|a| a.modes.as_ref())?;
        let name = m.available.iter().find(|x| x.id == m.current).map(|x| crate::acp::mode_label(&x.id, &x.name)).unwrap_or_else(|| m.current.clone());
        // 颜色提示风险：自动接受编辑 = 强调色，计划 = 黄，跳过所有确认 = 红，其余灰
        let col = match m.current.as_str() { "acceptEdits" | "auto" => t.accent, "plan" => t.modified, "bypassPermissions" => t.del, _ => t.dim };
        let (hover, text) = (t.hover, t.text);
        Some(div().id("mode-chip").flex().items_center().gap(px(4.)).h(px(26.)).px(px(8.)).rounded(px(8.)).cursor_pointer().text_size(px(12.)).text_color(col)
            .hover(move |s| s.bg(hover).text_color(text))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_mode_menu(cx)))
            .child(div().max_w(px(110.)).truncate().child(name)))
    }

    /// 模式选择浮层：每项一行名称 + 说明，当前项有对勾；底部提示 Shift+Tab
    fn mode_menu_view(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> Div {
        let (items, current) = self.acp.as_ref().and_then(|a| a.modes.as_ref()).map(|m| (m.available.clone(), m.current.clone())).unwrap_or_default();
        div().absolute().right(px(0.)).bottom_full().mb(px(8.)).w(px(300.)).p(px(5.)).rounded(px(12.)).bg(t.elev).border_1().border_color(t.border_strong)
            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.) }])
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| { this.mode_menu = false; cx.notify(); }))
            .child(div().px(px(9.)).pt(px(4.)).pb(px(5.)).text_size(px(11.)).text_color(t.faint).child("权限模式"))
            .children(items.into_iter().enumerate().map(|(i, m)| {
                let on = m.id == current;
                let id = m.id.clone();
                let danger = m.id == "bypassPermissions";
                div().id(("mode-opt", i)).flex().items_start().gap(px(8.)).px(px(9.)).py(px(6.)).rounded(px(8.)).cursor_pointer()
                    .when(on, |d| d.bg(t.accent_soft)).hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.acp_choose_mode(&id, cx)))
                    .child(div().flex_none().w(px(14.)).pt(px(2.)).child(if on { icon("check", 13., t.accent).into_any_element() } else { div().into_any_element() }))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(1.))
                        .child(div().text_size(px(13.)).text_color(if danger { t.del } else if on { t.text } else { t.dim }).child(crate::acp::mode_label(&m.id, &m.name)))
                        .child(div().text_size(px(11.5)).text_color(t.faint).child(crate::acp::mode_hint(&m.id, &m.description))))
            }))
            .child(div().mt(px(4.)).pt(px(5.)).pb(px(3.)).px(px(9.)).border_t_1().border_color(t.border).text_size(px(11.)).text_color(t.faint).child("⇧Tab 在输入框里切换（不含「跳过所有确认」）"))
    }

    /// 模型选择器浮层：当前项有对勾，键盘选中项高亮；底部可去设置页管理 Key
    fn model_menu_view(&self, t: &crate::theme::Theme, cx: &mut Context<Self>) -> Div {
        let choices = self.model_choices();
        let current = self.settings.model.trim().to_string();
        let sel = self.model_sel;
        div().absolute().right(px(0.)).bottom_full().mb(px(8.)).w(px(290.)).p(px(5.)).rounded(px(12.)).bg(t.elev).border_1().border_color(t.border_strong)
            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.) }])
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_model_menu(cx)))
            .child(div().px(px(9.)).pt(px(4.)).pb(px(5.)).text_size(px(11.)).text_color(t.faint).child("选择模型"))
            .children(choices.into_iter().enumerate().map(|(i, c)| {
                let on = c.id == current;
                div().id(("model-opt", i)).flex().items_center().gap(px(8.)).h(px(30.)).px(px(9.)).rounded(px(8.)).cursor_pointer().text_size(px(13.))
                    .text_color(if i == sel { t.text } else { t.dim }).when(i == sel, |d| d.bg(t.accent_soft)).hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_model(i, cx)))
                    .child(div().flex_none().w(px(14.)).child(if on { icon("check", 13., t.accent).into_any_element() } else { div().into_any_element() }))
                    .child(div().flex_1().min_w_0().truncate().child(c.label.clone()))
                    .child(div().flex_none().text_size(px(11.5)).text_color(t.faint).when(c.note.is_empty(), |d| d.font_family(MONO_FONT).max_w(px(110.)).truncate()).child(if c.note.is_empty() { if c.label == c.id { String::new() } else { c.id.clone() } } else { c.note.clone() }))
            }))
            .child(
                div().id("model-manage").mt(px(4.)).pt(px(2.)).h(px(30.)).px(px(9.)).flex().items_center().border_t_1().border_color(t.border).text_size(px(12.)).text_color(t.faint).cursor_pointer().hover(|s| s.text_color(t.dim))
                    .on_click(cx.listener(|this, _, _, cx| { this.close_model_menu(cx); cx.emit(AgentEvent::OpenSettings); }))
                    .child("管理模型与 Key…"),
            )
    }

    /// 一次工具调用：读类是一行紧凑的摘要；run_command 是带确认按钮和输出的终端卡片
    fn render_call(&self, msg_id: u64, i: usize, c: &ToolCall, cx: &mut Context<Self>) -> Div {
        if c.name.starts_with("acp:") { return self.render_acp_call(msg_id, i, c, cx); }
        let t = cx.theme().clone();
        let (verb, obj) = tools::parts(&c.name, &c.input);
        let failed = c.result.as_deref().map(|r| r.starts_with("错误")).unwrap_or(false);
        let denied = c.result.as_deref().map(|r| r.starts_with("用户拒绝") || r.starts_with("用户中止")).unwrap_or(false);
        if c.name != "run_command" {
            let ic = match c.name.as_str() { "search" => "search", "list_dir" => "folder", "edit_file" => "agent", "diagnostics" => "alert", _ => "file" };
            // 诊断工具：右侧直接给出「无问题 / N 条」
            let note: Option<(String, Hsla)> = if c.name == "diagnostics" {
                c.result.as_ref().filter(|_| !failed).map(|r| if r.starts_with("没有诊断") { ("无问题".to_string(), t.add) } else { (format!("{} 条", r.lines().filter(|l| !l.starts_with('…')).count()), t.modified) })
            } else { None };
            return div().flex().items_center().gap(px(8.)).h(px(30.)).px(px(10.)).rounded(px(9.)).border_1().border_color(t.border).text_size(px(12.5)).text_color(t.dim)
                .child(icon(ic, 13., t.faint))
                .child(div().flex_none().child(verb))
                .child(div().flex_1().min_w_0().truncate().font_family(MONO_FONT).text_size(px(12.)).text_color(t.text).child(obj))
                .when_some(note, |d, (n, col)| d.child(div().flex_none().text_size(px(11.5)).text_color(col).child(n)))
                .child(if failed { icon("x", 13., t.del).into_any_element() } else if c.result.is_some() { icon("check", 13., t.add).into_any_element() } else { div().text_color(t.faint).child("…").into_any_element() });
        }

        let awaiting = self.awaiting == Some((msg_id, i));
        let running = self.running == Some((msg_id, i));
        let key = (msg_id, 10_000 + i);
        let expanded = self.expanded.contains(&key);
        let status = c.result.as_deref().and_then(tools::result_status);
        let body: Option<String> = c.result.as_ref().filter(|_| !denied).map(|r| match tools::result_status(r) { Some((_, _, b)) => b.to_string(), None => r.clone() });
        let lines: Vec<String> = body.as_deref().map(|b| b.lines().map(str::to_string).collect()).unwrap_or_default();
        const COLLAPSED: usize = 10;
        let hidden = lines.len() > COLLAPSED && !expanded;
        let shown: Vec<String> = if hidden { lines[lines.len() - COLLAPSED..].to_vec() } else { lines.clone() };
        let uid = (msg_id * 100 + i as u64) as usize;
        let chip_el = |text: String, ok: bool| div().flex_none().px(px(7.)).h(px(18.)).flex().items_center().rounded(px(5.)).text_size(px(11.)).font_family(MONO_FONT)
            .bg(if ok { t.add_bg } else { t.del_bg }).text_color(if ok { t.add } else { t.del }).child(text);

        div().flex().flex_col().rounded(px(11.)).border_1().border_color(if awaiting { t.accent_line } else { t.border_strong }).bg(t.bg).overflow_hidden()
            .child(
                div().h(px(32.)).px(px(10.)).flex().items_center().gap(px(8.)).bg(t.elev).border_b_1().border_color(t.border).text_size(px(12.))
                    .child(icon("terminal", 14., if awaiting { t.accent } else { t.dim }))
                    .child(div().text_color(if awaiting { t.text } else { t.dim }).font_weight(FontWeight::MEDIUM).child(if awaiting { "Agent 想运行这条命令" } else { "终端" }))
                    .child(div().flex_1())
                    .child(if awaiting { div().text_color(t.accent).child("等待确认").into_any_element() }
                        else if running { div().text_color(t.dim).child("运行中…").into_any_element() }
                        else if denied { div().flex_none().px(px(7.)).h(px(18.)).flex().items_center().rounded(px(5.)).text_size(px(11.)).bg(t.press).text_color(t.dim).child(if c.result.as_deref().unwrap_or("").starts_with("用户拒绝") { "已拒绝" } else { "已中止" }).into_any_element() }
                        else if let Some((label, ok, _)) = status.clone() { chip_el(label, ok).into_any_element() }
                        else if failed { chip_el("出错".into(), false).into_any_element() }
                        else { div().text_color(t.faint).child("…").into_any_element() }),
            )
            .child(
                div().flex().gap(px(7.)).px(px(11.)).py(px(9.)).font_family(MONO_FONT).text_size(px(12.)).line_height(px(18.))
                    .child(div().flex_none().text_color(t.accent).child("$"))
                    .child(div().flex_1().min_w_0().text_color(t.text).child(obj)),
            )
            .when(!shown.is_empty(), |d| d.child(
                div().flex().flex_col().px(px(11.)).py(px(8.)).border_t_1().border_color(t.border).font_family(MONO_FONT).text_size(px(11.5)).line_height(px(17.)).text_color(t.dim)
                    .children(shown.iter().map(|l| div().child(if l.is_empty() { " ".to_string() } else { l.clone() }))),
            ))
            .when(lines.len() > COLLAPSED, |d| d.child(
                div().id(("cmd-more", uid)).h(px(26.)).flex().items_center().justify_center().border_t_1().border_color(t.border).text_size(px(12.)).text_color(t.faint).cursor_pointer().hover(|s| s.text_color(t.text))
                    .on_click(cx.listener(move |this, _, _, cx| { if !this.expanded.remove(&key) { this.expanded.insert(key); } cx.notify(); }))
                    .child(if expanded { "收起".to_string() } else { format!("展开全部 {} 行", lines.len()) }),
            ))
            .when(awaiting, |d| d.child(
                div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(8.)).border_t_1().border_color(t.border).bg(t.elev)
                    .child(
                        div().id(("cmd-always", uid)).text_size(px(12.)).text_color(t.faint).cursor_pointer().hover(|s| s.text_color(t.dim)).child("本会话始终允许")
                            .on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Always, cx))),
                    )
                    .child(div().flex_1())
                    .child(btn(("cmd-deny", uid), "拒绝  Esc", false, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Deny, cx))))
                    .child(btn(("cmd-allow", uid), "允许运行  ⌘↵", true, cx).h(px(26.)).px(px(12.)).on_click(cx.listener(|this, _, _, cx| this.decide(Decision::Once, cx)))),
            ))
    }

    /// `cont`：上一条也是助手消息（同一次回复的后续一轮，比如工具调用之后），不再重复头像，间距也收紧
    fn render_message(&self, m: &Message, cont: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        match m.role {
            Role::User => div().flex().flex_col().items_end().gap(px(6.)).mb(px(18.))
                .when(!m.refs.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(5.)).justify_end().children(m.refs.iter().map(|r| chip(r, cx)))))
                .child(div().max_w(px(340.)).px(px(13.)).py(px(9.)).rounded_tl(px(14.)).rounded_tr(px(14.)).rounded_bl(px(14.)).rounded_br(px(4.)).bg(t.elev).border_1().border_color(t.border).child(strip_context(&m.content).to_string()))
                .when_some(self.restore_row(m, cx), |d, row| d.child(row))
                .into_any_element(),
            Role::Assistant => {
                let streaming = m.status == Status::Streaming;
                let segs = blocks::parse_segments(&m.content);
                let pending: Vec<(usize, CodeBlock)> = segs.iter().enumerate().filter_map(|(i, s)| match s { Segment::Code { block, closed: true } if block.path.is_some() && !m.applied.contains(&i) => Some((i, block.clone())), _ => None }).collect();
                let mid = m.id;
                div().flex().gap(px(11.)).mb(px(if cont { 12. } else { 20. })).when(cont, |d| d.mt(px(-8.)))
                    .child(if cont { div().flex_none().size(px(22.)) } else { div().flex_none().size(px(22.)).mt(px(1.)).rounded(px(7.)).bg(t.accent).flex().items_center().justify_center().child(icon("agent", 13., t.accent_ink)).when(streaming, |d| d.opacity(0.85)) })
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().gap(px(8.)).line_height(px(21.))
                            .when(segs.is_empty() && m.calls.is_empty() && streaming, |d| d.child(div().text_color(t.faint).child("思考中…")))
                            .children(segs.iter().enumerate().map(|(i, s)| match s {
                                Segment::Text(tx) => self.render_text(tx, cx).into_any_element(),
                                Segment::Code { block, closed } => self.render_code(mid, i, block, *closed, m.applied.contains(&i), cx).into_any_element(),
                            }))
                            .children(m.calls.iter().enumerate().map(|(ci, c)| self.render_call(mid, ci, c, cx)))
                            .when(streaming && !segs.is_empty(), |d| d.child(div().w(px(7.)).h(px(14.)).rounded(px(2.)).bg(t.accent)))
                            .when_some(if let Status::Error(e) = &m.status { Some(e.clone()) } else { None }, |d, e| d.child(div().px(px(12.)).py(px(8.)).rounded(px(9.)).bg(t.del_bg).text_color(t.del).child(format!("出错了：{e}"))))
                            .when(!streaming && pending.len() > 1, |d| d.child(
                                div().flex().items_center().gap(px(8.)).px(px(12.)).py(px(8.)).rounded(px(11.)).bg(t.accent_soft).border_1().border_color(t.accent_line)
                                    .child(div().flex_1().child(format!("{} 个文件有修改建议", pending.len())))
                                    .child(btn(("apply-all", mid as usize), "全部应用", true, cx).h(px(24.)).on_click(cx.listener(move |this, _, _, cx| {
                                        for (i, b) in pending.clone() {
                                            for mm in this.threads.iter_mut().flat_map(|t| t.messages.iter_mut()) { if mm.id == mid { mm.applied.insert(i); } }
                                            this.emit_apply(mid, b.path.clone(), b.code.clone(), cx);
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

fn chip(path: &str, cx: &App) -> Div {
    let t = cx.theme();
    // 网址显示域名（最后一段路径常常没有意义），其余显示文件名
    let name = match path.strip_prefix("https://").or_else(|| path.strip_prefix("http://")) {
        Some(rest) => rest.split('/').next().unwrap_or(rest).to_string(),
        None => path.rsplit('/').next().unwrap_or(path).to_string(),
    };
    div().flex().items_center().gap(px(5.)).h(px(22.)).px(px(8.)).rounded(px(7.)).bg(t.hover).border_1().border_color(t.border).text_size(px(12.)).font_family(MONO_FONT).text_color(t.dim).child(file_icon(&name, cx)).child(name)
}

const SUGGESTS: [&str; 4] = ["解释一下当前文件做了什么", "找出这个文件里潜在的 bug", "为当前文件补充单元测试", "把这段代码重构得更简洁"];

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_rules(cx);
        let t = cx.theme().clone();
        let streaming = self.cancel.is_some() || self.awaiting.is_some();
        let (title, msg_count) = { let th = &self.threads[self.active]; (th.title.clone(), th.messages.len()) };
        self.sync_msgs(window);
        let atts = { let text = self.composer.read(cx).text(); self.attachments(&text) };
        let can_send = !self.composer.read(cx).is_empty() && !streaming;
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(t.panel)
            .on_action(cx.listener(|this, _: &crate::actions::MoveUp, _, cx| { if this.model_menu { this.move_model_sel(-1, cx); } else if let Some(m) = &mut this.mention { m.sel = (m.sel + m.items.len() - 1) % m.items.len(); cx.notify(); } }))
            .on_action(cx.listener(|this, _: &crate::actions::MoveDown, _, cx| { if this.model_menu { this.move_model_sel(1, cx); } else if let Some(m) = &mut this.mention { m.sel = (m.sel + 1) % m.items.len(); cx.notify(); } }))
            .on_action(cx.listener(|this, _: &crate::actions::AcceptReview, _, cx| {
                // ⌘↵：有命令在等确认就允许运行；否则交给后面的处理者（接受审阅）
                if this.awaiting.is_some() || this.acp_perm_pending() { this.decide(Decision::Once, cx); } else { cx.propagate(); }
            }))
            .on_action(cx.listener(|this, _: &crate::actions::Enter, _, cx| if this.restoring.is_some() { this.confirm_restore(cx) } else if this.model_menu { this.choose_model(this.model_sel, cx) } else { this.pick_mention(cx) }))
            .on_action(cx.listener(|this, _: &crate::actions::Outdent, _, cx| this.acp_cycle_mode(cx)))
            .on_action(cx.listener(|this, _: &crate::actions::Tab, _, cx| if this.model_menu { this.choose_model(this.model_sel, cx) } else { this.pick_mention(cx) }))
            .child(
                div().h(px(44.)).flex_none().px(px(12.)).flex().items_center().gap(px(6.)).border_b_1().border_color(t.border)
                    .child(
                        div().id("thread-btn").flex().items_center().gap(px(5.)).px(px(8.)).h(px(28.)).rounded(px(8.)).cursor_pointer().font_weight(FontWeight::SEMIBOLD).max_w(px(250.))
                            .hover(|s| s.bg(t.hover)).on_click(cx.listener(|this, _, _, cx| { this.menu = !this.menu; cx.notify(); }))
                            .child(div().truncate().child(title)).child(icon("chevron_down", 13., t.faint)),
                    )
                    .child(div().flex_1())
                    .child(icon_button("new-thread", "plus", false, cx).on_click(cx.listener(|this, _, _, cx| this.new_thread(cx)))),
            )
            .child(
                div().id("agent-msgs").flex_1().min_h_0().px(px(16.)).pt(px(18.)).pb(px(8.))
                    .on_scroll_wheel(cx.listener(|this, _, _, _| { this.stick = false; }))
                    .when(msg_count == 0, |d| d.child(
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
                    .when(msg_count > 0, |d| d.child(
                        list(self.msgs.clone(), cx.processor(|this, ix: usize, _window, cx| {
                            let th = &this.threads[this.active];
                            let Some(m) = th.messages.get(ix) else { return div().into_any_element() };
                            let cont = ix > 0 && th.messages[ix - 1].role == Role::Assistant && m.role == Role::Assistant;
                            // 外面包一层：列表量高度时算的是这个包装元素，消息自己的上下外边距（含「连续消息」的负边距）才会被算进去
                            div().w_full().child(this.render_message(m, cont, cx)).into_any_element()
                        })).size_full()
                    )),
            )
            .child(
                div().flex_none().mx(px(12.)).mb(px(12.)).mt(px(6.)).relative().rounded(px(14.)).bg(t.elev).border_1().border_color(t.border_strong)
                    .when_some(self.mention.as_ref().map(|m| (m.items.clone(), m.sel)), |d, (items, sel)| d.child(
                        div().absolute().left(px(8.)).right(px(8.)).bottom(px(100.)).p(px(5.)).rounded(px(11.)).bg(t.elev).border_1().border_color(t.border_strong)
                            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(10.)), blur_radius: px(30.), spread_radius: px(0.) }])
                            .children(items.into_iter().enumerate().map(|(i, c)| {
                                let label = c.label();
                                let tag_col = match c.tag() { "符号" => t.accent, "对话" => t.modified, "网页" => t.add, _ => t.faint };
                                let detail = c.detail();
                                div().flex().items_center().gap(px(8.)).px(px(9.)).py(px(5.)).rounded(px(7.)).text_size(px(12.)).text_color(if i == sel { t.text } else { t.dim })
                                    .when(i == sel, |d| d.bg(t.accent_soft))
                                    .child(div().flex_none().w(px(28.)).text_size(px(10.5)).text_color(tag_col).child(c.tag()))
                                    .child(if detail.is_some() { div().flex_none().max_w(px(170.)).truncate().font_family(MONO_FONT).child(label) } else { div().flex_1().min_w_0().truncate().font_family(MONO_FONT).child(label) })
                                    .when_some(detail, |d, s| d.child(div().flex_1().min_w_0().truncate().text_size(px(11.5)).text_color(t.faint).child(s)))
                            })),
                    ))
                    .when(!atts.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(5.)).px(px(10.)).pt(px(9.)).children(atts.iter().map(|a| chip(a, cx)))))
                    .child(div().px(px(12.)).py(px(8.)).child(self.composer.clone()))
                    .child(
                        div().flex().items_center().gap(px(6.)).px(px(8.)).pb(px(8.)).pl(px(12.)).text_size(px(12.)).text_color(t.faint)
                            .child("↵ 发送 · ⇧↵ 换行")
                            .when(self.always_allow, |d| d.child(
                                div().id("auto-allow").px(px(6.)).py(px(1.)).rounded(px(5.)).bg(t.accent_soft).text_color(t.accent).cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| { this.always_allow = false; cx.notify(); })).child("自动允许命令 ✕"),
                            ))
                            .when_some(self.rules_file.clone(), |d, f| d.child(div().id("rules-tag").px(px(6.)).py(px(1.)).rounded(px(5.)).bg(t.press).text_color(t.dim).child(format!("规则 · {f}"))))
                            .child(div().flex_1())
                            .children(self.mode_chip(&t, cx))
                            .child(self.model_chip(&t, cx))
                            .child(if streaming {
                                div().id("stop").size(px(28.)).rounded(px(9.)).flex().items_center().justify_center().bg(t.press).cursor_pointer().child(icon("stop", 14., t.text))
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))).into_any_element()
                            } else {
                                div().id("send").size(px(28.)).rounded(px(9.)).flex().items_center().justify_center().bg(t.accent).cursor_pointer().when(!can_send, |d| d.opacity(0.3)).child(icon("send", 15., t.accent_ink))
                                    .on_click(cx.listener(|this, _, _, cx| this.send(cx))).into_any_element()
                            }),
                    )
                    // 最后绘制，才能盖在输入框文字上面；贴着输入框上沿，向上展开
                    .when(self.model_menu, |d| d.child(self.model_menu_view(&t, cx)))
                    .when(self.mode_menu && self.acp.as_ref().map(|a| a.modes.is_some()).unwrap_or(false), |d| d.child(self.mode_menu_view(&t, cx))),
            )
            .when(self.menu, |d| d.child(
                div().absolute().inset_0().id("thread-scrim").on_click(cx.listener(|this, _, _, cx| { this.menu = false; cx.notify(); })).child(
                    div().absolute().top(px(42.)).left(px(10.)).w(px(300.)).max_h(px(340.)).p(px(6.)).rounded(px(12.)).bg(t.elev).border_1().border_color(t.border_strong).overflow_hidden()
                        .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.45), offset: point(px(0.), px(14.)), blur_radius: px(40.), spread_radius: px(0.) }])
                        .children(self.threads.iter().enumerate().map(|(i, th)| {
                            div().id(("th", i)).flex().items_center().px(px(10.)).py(px(7.)).rounded(px(8.)).cursor_pointer().gap(px(8.))
                                .text_color(if i == self.active { t.text } else { t.dim }).when(i == self.active, |d| d.bg(t.accent_soft)).hover(|s| s.bg(t.hover))
                                .on_click(cx.listener(move |this, _, _, cx| { this.abort_pending(cx); this.active = i; this.menu = false; this.stick = true; cx.notify(); }))
                                .child(div().flex_1().truncate().child(th.title.clone()))
                                .child(div().id(("thdel", i)).child(icon("trash", 13., t.faint)).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.abort_pending(cx);
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

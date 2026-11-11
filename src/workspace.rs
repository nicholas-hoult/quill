//! 主工作区：标题栏、侧栏（文件 / 搜索 / Git）、标签页、编辑器、Agent 面板、状态栏、各类浮层。
use crate::actions::*;
use crate::agent::{AgentEvent, AgentPanel};
use crate::ai::Settings;
use crate::editor::{Editor, EditorEvent, EditorMode};
use crate::git::{self, Change};
use crate::palette::{CommandItem, Palette, PaletteEvent, PaletteMode};
use crate::settings_view::{SettingsEvent, SettingsView};
use crate::inline::{InlineAssist as InlineView, InlineEvent};
use crate::syntax;
use crate::terminal::TerminalView;
use crate::theme::{ActiveTheme, MONO_FONT, Theme, UI_FONT};
use crate::tree::{self, Hit, TreeRow};
use crate::ui::{btn, file_icon, icon, icon_button, kbd};
use gpui::{prelude::*, *};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum SideView { Files, Search, Git }

#[derive(Clone, Copy, PartialEq)]
enum Resize { Side, Agent, Term }

struct Tab {
    path: String,
    editor: Entity<Editor>,
    _sub: Subscription,
}

struct Toast { id: u64, text: String, error: bool }

enum Overlay {
    None,
    Palette(Entity<Palette>),
    Settings(Entity<SettingsView>),
    Prompt(Entity<PromptView>),
}

/// 文件树右键菜单
struct ContextMenu { pos: Point<Pixels>, path: String, is_dir: bool }

pub struct PromptView {
    title: String,
    input: Entity<Editor>,
    kind: PromptKind,
    _sub: Subscription,
}

#[derive(Clone)]
pub enum PromptKind { NewFile(String), NewFolder(String), Rename(String), ConfirmDelete(String) }

pub enum PromptEvent { Close, Submit(PromptKind, String) }
impl EventEmitter<PromptEvent> for PromptView {}

impl PromptView {
    fn new(title: &str, initial: &str, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let confirm = matches!(kind, PromptKind::ConfirmDelete(_));
        let input = cx.new(|cx| Editor::new(EditorMode::input(""), cx));
        input.update(cx, |e, cx| { e.set_text(initial, cx); });
        if !confirm { window.focus(&input.read(cx).focus); }
        let sub = cx.subscribe(&input, |this, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Submit => { let t = this.input.read(cx).text(); cx.emit(PromptEvent::Submit(this.kind.clone(), t)); }
            EditorEvent::Cancel => cx.emit(PromptEvent::Close),
            _ => {}
        });
        Self { title: title.to_string(), input, kind, _sub: sub }
    }
}

impl Render for PromptView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let confirm = matches!(self.kind, PromptKind::ConfirmDelete(_));
        div().id("prompt-scrim").absolute().inset_0().flex().justify_center().items_start().pt(px(160.)).bg(t.scrim).occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PromptEvent::Close)))
            .child(
                div().w(px(420.)).p(px(18.)).flex().flex_col().gap(px(12.)).rounded(px(14.)).bg(t.elev).border_1().border_color(t.border_strong)
                    .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.5), offset: point(px(0.), px(18.)), blur_radius: px(50.), spread_radius: px(0.) }])
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                    .when(!confirm, |d| d.child(div().h(px(36.)).px(px(12.)).flex().items_center().rounded(px(9.)).bg(t.bg).border_1().border_color(t.accent_line).child(div().flex_1().child(self.input.clone()))))
                    .child(div().flex().justify_end().gap(px(8.))
                        .child(btn("prompt-cancel", "取消", false, cx).on_click(cx.listener(|_, _, _, cx| cx.emit(PromptEvent::Close))))
                        .child(btn("prompt-ok", if confirm { "删除" } else { "确定" }, true, cx).on_click(cx.listener(|this, _, _, cx| { let t = this.input.read(cx).text(); cx.emit(PromptEvent::Submit(this.kind.clone(), t)); })))),
            )
    }
}

pub struct Workspace {
    focus: FocusHandle,
    root: Option<PathBuf>,
    files: Vec<String>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    tree_scroll: UniformListScrollHandle,
    selected: Option<String>,
    side_view: SideView,
    sidebar: bool,
    agent_open: bool,
    side_w: f32,
    agent_w: f32,
    side_t: f32,
    agent_t: f32,
    resizing: Option<(Resize, f32, f32)>,
    overlay: Overlay,
    menu: Option<ContextMenu>,
    agent: Entity<AgentPanel>,
    terminal: Option<Entity<TerminalView>>,
    term_open: bool,
    term_h: f32,
    term_t: f32,
    search_input: Entity<Editor>,
    search_hits: Vec<Hit>,
    search_scroll: UniformListScrollHandle,
    find_input: Entity<Editor>,
    find_open: bool,
    inline: Option<Entity<InlineView>>,
    changes: Vec<Change>,
    branch: Option<String>,
    pub settings: Settings,
    toasts: Vec<Toast>,
    toast_seq: u64,
    recents: Vec<String>,
    _subs: Vec<Subscription>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

fn recents_path() -> Option<PathBuf> { Some(PathBuf::from(std::env::var("HOME").ok()?).join("Library/Application Support/Quill/recent.json")) }

fn load_recents() -> Vec<String> {
    recents_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn system_is_dark(window: &Window) -> bool { matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark) }

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let weak = cx.weak_entity();
        let agent = cx.new(|cx| AgentPanel::new(weak, settings.clone(), cx));
        let search_input = cx.new(|cx| Editor::new(EditorMode::input("在项目中搜索"), cx));
        let find_input = cx.new(|cx| Editor::new(EditorMode::input("查找"), cx));
        let mut subs = vec![];
        subs.push(cx.subscribe_in(&agent, window, |this, _, ev: &AgentEvent, window, cx| match ev {
            AgentEvent::Apply { path, code } => this.apply_code(path.clone(), code.clone(), window, cx),
            AgentEvent::OpenSettings => this.open_settings(window, cx),
        }));
        subs.push(cx.subscribe(&search_input, |this, _, ev: &EditorEvent, cx| if let EditorEvent::Changed = ev { this.run_search(cx) }));
        subs.push(cx.subscribe_in(&find_input, window, |this, input, ev: &EditorEvent, window, cx| match ev {
            EditorEvent::Changed => { let q = input.read(cx).text(); if let Some(e) = this.active_editor() { e.update(cx, |e, cx| e.set_search(Some(q), cx)); } }
            EditorEvent::Submit => { if let Some(e) = this.active_editor() { e.update(cx, |e, cx| e.find_step(true, cx)); } }
            EditorEvent::Cancel => this.close_find(window, cx),
            _ => {}
        }));
        let mut this = Self {
            focus: cx.focus_handle(), root: None, files: vec![], tabs: vec![], active: None, expanded: HashSet::new(), rows: vec![], tree_scroll: UniformListScrollHandle::new(),
            selected: None, side_view: SideView::Files, sidebar: true, agent_open: true, side_w: 264.0, agent_w: 440.0, side_t: 1.0, agent_t: 1.0, resizing: None,
            overlay: Overlay::None, menu: None, agent, terminal: None, term_open: false, term_h: 280.0, term_t: 0.0, search_input, search_hits: vec![], search_scroll: UniformListScrollHandle::new(), find_input, find_open: false,
            inline: None, changes: vec![], branch: None, settings, toasts: vec![], toast_seq: 0, recents: load_recents(), _subs: subs,
        };
        this.apply_theme(window, cx);
        this
    }

    pub fn apply_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let dark = match self.settings.theme.as_str() { "dark" => true, "light" => false, _ => system_is_dark(window) };
        cx.set_global(if dark { Theme::dark() } else { Theme::light() });
        cx.notify();
    }

    // ---------- 工具 ----------

    fn active_editor(&self) -> Option<Entity<Editor>> { self.active.and_then(|i| self.tabs.get(i)).map(|t| t.editor.clone()) }

    pub fn file_text_rel(&self, rel: &str, cx: &App) -> Option<String> {
        let root = self.root.as_ref()?;
        let abs = root.join(rel).to_string_lossy().to_string();
        self.file_text(&abs, cx)
    }

    pub fn file_text(&self, abs: &str, cx: &App) -> Option<String> {
        if let Some(t) = self.tabs.iter().find(|t| t.path == abs) { return Some(t.editor.read(cx).text()); }
        std::fs::read_to_string(abs).ok()
    }

    fn rel(&self, abs: &str) -> String {
        self.root.as_ref().and_then(|r| Path::new(abs).strip_prefix(r).ok()).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| abs.to_string())
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toasts.push(Toast { id, text: text.into(), error });
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(3600)).await;
            let _ = this.update(cx, |w, cx| { w.toasts.retain(|t| t.id != id); cx.notify(); });
        }).detach();
        cx.notify();
    }

    // ---------- 项目 ----------

    pub fn open_root(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.active = None;
        self.terminal = None;
        self.term_open = false;
        self.expanded.clear();
        self.root = Some(path.clone());
        let s = path.to_string_lossy().to_string();
        self.recents.retain(|r| *r != s);
        self.recents.insert(0, s);
        self.recents.truncate(10);
        if let Some(p) = recents_path() {
            if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
            let _ = std::fs::write(p, serde_json::to_string(&self.recents).unwrap_or_default());
        }
        window.set_window_title(&format!("{} — Quill", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
        self.refresh(cx);
        window.focus(&self.focus);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        self.rows = tree::build_rows(&root, &self.expanded);
        let r2 = root.clone();
        cx.spawn(async move |this, cx| {
            let r3 = r2.clone();
            let (files, changes, branch) = cx.background_executor().spawn(async move {
                let s = r3.to_string_lossy().to_string();
                (tree::list_files(&r3, 30_000), git::status(&s), git::branch(&s))
            }).await;
            let _ = this.update(cx, |w, cx| {
                w.agent.update(cx, |a, _| a.files = files.clone());
                w.files = files; w.changes = changes; w.branch = branch;
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("打开".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update_in(cx, |w, window, cx| w.open_root(p, window, cx));
                }
            }
        }).detach();
    }

    pub fn open_file(&mut self, abs: &str, line: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.path == abs) {
            self.active = Some(i);
        } else {
            let text = match std::fs::read(abs) {
                Ok(b) => match String::from_utf8(b) { Ok(s) => s, Err(_) => { self.toast("不是 UTF-8 文本文件，无法打开", true, cx); return; } },
                Err(e) => { self.toast(format!("无法打开：{e}"), true, cx); return; }
            };
            let editor = cx.new(|cx| { let mut e = Editor::new(EditorMode::code(), cx); e.open(Some(abs.to_string()), &text, cx); e });
            let sub = cx.subscribe_in(&editor, window, |this, ed, ev: &EditorEvent, window, cx| match ev {
                EditorEvent::InlineAssist => this.open_inline(window, cx),
                EditorEvent::Changed | EditorEvent::ReviewEnded => cx.notify(),
                EditorEvent::Cancel => { if this.find_open { this.close_find(window, cx); } else { let _ = ed; } }
                _ => {}
            });
            self.tabs.push(Tab { path: abs.to_string(), editor: editor.clone(), _sub: sub });
            self.active = Some(self.tabs.len() - 1);
            if let Some(root) = self.root.clone() {
                let rel = self.rel(abs);
                let ed = editor.clone();
                cx.spawn(async move |_, cx| {
                    let r = root.to_string_lossy().to_string();
                    let head = cx.background_executor().spawn(async move { git::head_content(&r, &rel) }).await;
                    let _ = ed.update(cx, |e, cx| e.set_head(head, cx));
                }).detach();
            }
        }
        self.selected = Some(abs.to_string());
        let ed = self.active_editor().unwrap();
        if let Some(l) = line { ed.update(cx, |e, cx| e.scroll_to_line(l.saturating_sub(1), cx)); }
        window.focus(&ed.read(cx).focus);
        let rel = self.rel(abs);
        self.agent.update(cx, |a, _| { a.active_file = Some(rel); });
        cx.notify();
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor() else { return };
        let (path, text) = ed.read_with(cx, |e, _| (e.path.clone(), e.text()));
        let Some(path) = path else { return };
        match std::fs::write(&path, &text) {
            Ok(()) => {
                ed.update(cx, |e, cx| { e.mark_saved(); cx.notify(); });
                self.refresh(cx);
                if let Some(root) = self.root.clone() {
                    let rel = self.rel(&path);
                    let ed2 = ed.clone();
                    cx.spawn(async move |_, cx| {
                        let r = root.to_string_lossy().to_string();
                        let head = cx.background_executor().spawn(async move { git::head_content(&r, &rel) }).await;
                        let _ = ed2.update(cx, |e, cx| e.set_head(head, cx));
                    }).detach();
                }
            }
            Err(e) => self.toast(format!("保存失败：{e}"), true, cx),
        }
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() { return; }
        self.tabs.remove(i);
        self.active = if self.tabs.is_empty() { None } else { Some(match self.active { Some(a) if a > i => a - 1, Some(a) if a == i => i.min(self.tabs.len() - 1), Some(a) => a, None => 0 }) };
        if let Some(ed) = self.active_editor() {
            window.focus(&ed.read(cx).focus);
            let rel = self.rel(&self.tabs[self.active.unwrap()].path);
            self.agent.update(cx, |a, _| a.active_file = Some(rel));
        } else {
            self.agent.update(cx, |a, _| a.active_file = None);
            window.focus(&self.focus);
        }
        cx.notify();
    }

    fn apply_code(&mut self, path: Option<String>, code: String, window: &mut Window, cx: &mut Context<Self>) {
        let abs = match (&path, &self.root) {
            (Some(p), Some(root)) => {
                let rel = p.trim_start_matches("./").trim_start_matches('/');
                let abs = root.join(rel);
                if !abs.exists() {
                    if let Some(d) = abs.parent() { let _ = std::fs::create_dir_all(d); }
                    if let Err(e) = std::fs::write(&abs, "") { self.toast(format!("无法创建文件：{e}"), true, cx); return; }
                    self.refresh(cx);
                }
                abs.to_string_lossy().to_string()
            }
            _ => match self.active.and_then(|i| self.tabs.get(i)) { Some(t) => t.path.clone(), None => { self.toast("没有可应用的目标文件：先打开一个文件，或让模型给出 path", true, cx); return; } },
        };
        self.open_file(&abs, None, window, cx);
        if let Some(ed) = self.active_editor() {
            ed.update(cx, |e, cx| {
                // 无 path 且有选区：只替换选区
                if path.is_none() && !e.sel.is_empty() {
                    e.begin_review(cx);
                    let (s, l) = (e.sel.start, e.sel.len());
                    e.replace_range_streaming(s, l, &code, cx);
                } else { e.apply_with_review(&code, cx); }
            });
        }
    }

    // ---------- 搜索 / 查找 ----------

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search_input.read(cx).text();
        let Some(root) = self.root.clone() else { return };
        if q.trim().is_empty() { self.search_hits.clear(); cx.notify(); return; }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(160)).await;
            let still = this.update(cx, |w, cx| w.search_input.read(cx).text() == q).unwrap_or(false);
            if !still { return; }
            let q2 = q.clone();
            let hits = cx.background_executor().spawn(async move { tree::search_text(&root, &q2, 600) }).await;
            let _ = this.update(cx, |w, cx| { if w.search_input.read(cx).text() == q { w.search_hits = hits; cx.notify(); } });
        }).detach();
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active.is_none() { return; }
        self.find_open = true;
        let sel = self.active_editor().map(|e| e.read(cx).selected_text()).unwrap_or_default();
        if !sel.is_empty() && !sel.contains('\n') { self.find_input.update(cx, |f, cx| f.set_text(&sel, cx)); }
        window.focus(&self.find_input.read(cx).focus);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        if let Some(e) = self.active_editor() { e.update(cx, |e, cx| e.set_search(None, cx)); window.focus(&e.read(cx).focus); }
        cx.notify();
    }

    // ---------- 浮层 ----------

    fn commands() -> Vec<CommandItem> {
        vec![
            CommandItem { title: "打开文件夹…", keys: "⌘O" }, CommandItem { title: "跳转到文件", keys: "⌘P" }, CommandItem { title: "保存", keys: "⌘S" },
            CommandItem { title: "切换侧栏", keys: "⌘B" }, CommandItem { title: "切换 AI 对话", keys: "⇧⌘A" }, CommandItem { title: "聚焦对话输入框", keys: "⌘L" },
            CommandItem { title: "在项目中搜索", keys: "⇧⌘F" }, CommandItem { title: "查看 Git 改动", keys: "" }, CommandItem { title: "就地改写（Inline Assist）", keys: "⌘K" },
            CommandItem { title: "在文件中查找", keys: "⌘F" }, CommandItem { title: "新建对话", keys: "" }, CommandItem { title: "打开设置", keys: "⌘," },
            CommandItem { title: "切换深色 / 浅色", keys: "" }, CommandItem { title: "关闭当前标签页", keys: "⌘W" }, CommandItem { title: "应用最近的代码块", keys: "⇧⌘↵" }, CommandItem { title: "切换终端", keys: "⌘J" },
        ]
    }

    fn open_palette(&mut self, mode: PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files.clone();
        let open: Vec<String> = self.tabs.iter().map(|t| self.rel(&t.path)).collect();
        let p = cx.new(|cx| Palette::new(mode, files, open, Self::commands(), window, cx));
        let sub = cx.subscribe_in(&p, window, |this, _, ev: &PaletteEvent, window, cx| {
            this.overlay = Overlay::None;
            match ev {
                PaletteEvent::Close => {}
                PaletteEvent::OpenFile(rel) => { if let Some(r) = this.root.clone() { this.open_file(&r.join(rel).to_string_lossy(), None, window, cx); } }
                PaletteEvent::Command(i) => this.run_command(*i, window, cx),
            }
            if let Some(e) = this.active_editor() { if matches!(ev, PaletteEvent::Close) { window.focus(&e.read(cx).focus); } } else { window.focus(&this.focus); }
            cx.notify();
        });
        self._subs.push(sub);
        self.overlay = Overlay::Palette(p);
        cx.notify();
    }

    fn run_command(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        match i {
            0 => self.pick_folder(window, cx),
            1 => self.open_palette(PaletteMode::Files, window, cx),
            2 => self.save_active(cx),
            3 => { self.sidebar = !self.sidebar; cx.notify(); }
            4 => { self.agent_open = !self.agent_open; cx.notify(); }
            5 => self.focus_agent(window, cx),
            6 => { self.sidebar = true; self.side_view = SideView::Search; window.focus(&self.search_input.read(cx).focus); cx.notify(); }
            7 => { self.sidebar = true; self.side_view = SideView::Git; cx.notify(); }
            8 => self.open_inline(window, cx),
            9 => self.open_find(window, cx),
            10 => self.agent.update(cx, |a, cx| a.new_thread(cx)),
            11 => self.open_settings(window, cx),
            12 => self.toggle_theme(window, cx),
            13 => { if let Some(a) = self.active { self.close_tab(a, window, cx); } }
            14 => self.agent.update(cx, |a, cx| a.apply_latest(cx)),
            15 => self.toggle_terminal(window, cx),
            _ => {}
        }
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() { return; }
        self.term_open = !self.term_open;
        if self.term_open {
            if self.terminal.is_none() {
                let cwd = self.root.as_ref().unwrap().to_string_lossy().to_string();
                let t = cx.new(|cx| TerminalView::new(&cwd, cx));
                if let Some(e) = t.read(cx).error.clone() { self.term_open = false; self.toast(format!("无法启动终端：{e}"), true, cx); return; }
                self.terminal = Some(t);
            }
            if let Some(t) = &self.terminal { window.focus(&t.read(cx).focus); }
        } else if let Some(e) = self.active_editor() { window.focus(&e.read(cx).focus); }
        cx.notify();
    }

    fn focus_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_open = true;
        self.agent.read(cx).focus_composer(window, cx);
        cx.notify();
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = cx.theme().dark;
        self.settings.theme = if dark { "light".into() } else { "dark".into() };
        let _ = self.settings.save();
        self.apply_theme(window, cx);
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.settings.clone();
        let v = cx.new(|cx| SettingsView::new(s, window, cx));
        let sub = cx.subscribe_in(&v, window, |this, _, ev: &SettingsEvent, window, cx| {
            this.overlay = Overlay::None;
            if let SettingsEvent::Save(s) = ev {
                this.settings = s.clone();
                let _ = s.save();
                let s2 = s.clone();
                this.agent.update(cx, |a, _| a.settings = s2);
                this.apply_theme(window, cx);
                this.toast("设置已保存", false, cx);
            }
            if let Some(e) = this.active_editor() { window.focus(&e.read(cx).focus); }
            cx.notify();
        });
        self._subs.push(sub);
        self.overlay = Overlay::Settings(v);
        cx.notify();
    }

    fn open_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor() else { return };
        if self.inline.is_some() { return; }
        let settings = self.settings.clone();
        let ed2 = ed.clone();
        let v = cx.new(|cx| InlineView::new(ed2, settings, window, cx));
        let sub = cx.subscribe_in(&v, window, |this, _, ev: &InlineEvent, window, cx| {
            let InlineEvent::Close = ev;
            this.inline = None;
            if let Some(e) = this.active_editor() { window.focus(&e.read(cx).focus); }
            cx.notify();
        });
        self._subs.push(sub);
        self.inline = Some(v);
        cx.notify();
    }

    fn prompt(&mut self, title: &str, initial: &str, kind: PromptKind, window: &mut Window, cx: &mut Context<Self>) {
        let v = cx.new(|cx| PromptView::new(title, initial, kind, window, cx));
        let sub = cx.subscribe_in(&v, window, |this, _, ev: &PromptEvent, window, cx| {
            this.overlay = Overlay::None;
            if let PromptEvent::Submit(kind, text) = ev { this.finish_prompt(kind.clone(), text.trim().to_string(), window, cx); }
            window.focus(&this.focus);
            cx.notify();
        });
        self._subs.push(sub);
        self.overlay = Overlay::Prompt(v);
        cx.notify();
    }

    fn finish_prompt(&mut self, kind: PromptKind, text: String, window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            PromptKind::NewFile(dir) | PromptKind::NewFolder(dir) if text.is_empty() => { let _ = dir; }
            PromptKind::NewFile(dir) => {
                let p = Path::new(&dir).join(&text);
                if p.exists() { self.toast("已存在同名文件", true, cx); return; }
                if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
                match std::fs::write(&p, "") { Ok(()) => { self.expanded.insert(dir); self.refresh(cx); self.open_file(&p.to_string_lossy(), None, window, cx); } Err(e) => self.toast(e.to_string(), true, cx) }
            }
            PromptKind::NewFolder(dir) => {
                match std::fs::create_dir_all(Path::new(&dir).join(&text)) { Ok(()) => { self.expanded.insert(dir); self.refresh(cx); } Err(e) => self.toast(e.to_string(), true, cx) }
            }
            PromptKind::Rename(old) => {
                if text.is_empty() { return; }
                let new = Path::new(&old).parent().map(|p| p.join(&text)).unwrap_or_default();
                if new.exists() { self.toast("目标已存在", true, cx); return; }
                match std::fs::rename(&old, &new) {
                    Ok(()) => { self.tabs.retain(|t| !(t.path == old || t.path.starts_with(&format!("{old}/")))); self.active = if self.tabs.is_empty() { None } else { Some(0) }; self.refresh(cx); }
                    Err(e) => self.toast(e.to_string(), true, cx),
                }
            }
            PromptKind::ConfirmDelete(path) => {
                let p = Path::new(&path);
                let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
                match r {
                    Ok(()) => { self.tabs.retain(|t| !(t.path == path || t.path.starts_with(&format!("{path}/")))); self.active = if self.tabs.is_empty() { None } else { Some(0) }; self.refresh(cx); }
                    Err(e) => self.toast(e.to_string(), true, cx),
                }
            }
        }
    }

    // ---------- 渲染：各部分 ----------

    fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let name = self.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Quill".into());
        let file = self.active.and_then(|i| self.tabs.get(i)).map(|t| self.rel(&t.path));
        div().h(px(44.)).flex_none().flex().items_center().gap(px(10.)).pl(px(84.)).pr(px(12.)).bg(t.panel).border_b_1().border_color(t.border).window_control_area(WindowControlArea::Drag)
            .child(icon_button("tb-side", "sidebar", self.sidebar, cx).on_click(cx.listener(|this, _, _, cx| { this.sidebar = !this.sidebar; cx.notify(); })))
            .child(div().flex().gap(px(6.)).text_color(t.dim).max_w(px(300.)).truncate().child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(name)).when_some(file, |d, f| d.child(format!("· {f}"))))
            .child(div().flex_1())
            .child(
                div().id("pill").h(px(28.)).w(px(360.)).px(px(12.)).flex().items_center().gap(px(8.)).rounded(px(9.)).bg(t.hover).border_1().border_color(t.border).text_color(t.faint).cursor_pointer()
                    .hover(|s| s.bg(t.press).border_color(t.border_strong).text_color(t.dim))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(PaletteMode::Files, window, cx)))
                    .child(icon("search", 14., t.faint)).child("搜索文件、运行命令").child(div().flex_1()).child(kbd("⌘P", cx)),
            )
            .child(div().flex_1())
            .child(icon_button("tb-term", "terminal", self.term_open, cx).on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx))))
            .child(icon_button("tb-agent", "agent", self.agent_open, cx).on_click(cx.listener(|this, _, _, cx| { this.agent_open = !this.agent_open; cx.notify(); })))
            .child(icon_button("tb-set", "settings", false, cx).on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))))
    }

    fn sidebar_view(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let tabs = [(SideView::Files, "文件"), (SideView::Search, "搜索"), (SideView::Git, "改动")];
        let nchanges = self.changes.len();
        div().size_full().flex().flex_col()
            .child(div().flex().gap(px(2.)).px(px(10.)).pt(px(8.)).pb(px(6.)).children(tabs.iter().map(|(v, label)| {
                let on = self.side_view == *v;
                let v = *v;
                div().id(*label).px(px(10.)).h(px(26.)).flex().items_center().gap(px(5.)).rounded(px(7.)).cursor_pointer().text_color(if on { t.text } else { t.dim }).font_weight(FontWeight::MEDIUM)
                    .when(on, |d| d.bg(t.press)).hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |this, _, window, cx| { this.side_view = v; if v == SideView::Search { window.focus(&this.search_input.read(cx).focus); } cx.notify(); }))
                    .child(*label).when(v == SideView::Git && nchanges > 0, |d| d.child(div().text_size(px(10.)).text_color(t.accent).child(nchanges.to_string())))
            })))
            .child(match self.side_view {
                SideView::Files => self.file_tree(cx).into_any_element(),
                SideView::Search => self.search_panel(cx).into_any_element(),
                SideView::Git => self.git_panel(cx).into_any_element(),
            })
    }

    fn file_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let n = self.rows.len();
        if n == 0 { return div().p(px(24.)).text_color(t.faint).flex().justify_center().child("空文件夹").into_any_element(); }
        uniform_list("tree", n, cx.processor(|this, range: std::ops::Range<usize>, _w, cx| {
            let t = cx.theme().clone();
            range.map(|ix| {
                let row = this.rows[ix].clone();
                let sel = this.selected.as_deref() == Some(row.path.as_str());
                let rel = this.rel(&row.path);
                let st = this.changes.iter().find(|c| c.path == rel || (row.is_dir && c.path.starts_with(&format!("{rel}/")))).map(|c| c.status);
                let (p1, p2, p3) = (row.path.clone(), row.path.clone(), row.path.clone());
                let is_dir = row.is_dir;
                div().id(("row", ix)).h(px(26.)).mx(px(6.)).pl(px(6. + row.depth as f32 * 14.)).pr(px(8.)).flex().items_center().gap(px(6.)).rounded(px(6.)).cursor_pointer()
                    .text_color(if sel { t.text } else { t.dim }).when(sel, |d| d.bg(t.accent_soft)).hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if is_dir { if !this.expanded.remove(&p1) { this.expanded.insert(p1.clone()); } this.selected = Some(p1.clone()); this.refresh(cx); }
                        else { this.open_file(&p1, None, window, cx); }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| { this.menu = Some(ContextMenu { pos: ev.position, path: p2.clone(), is_dir }); this.selected = Some(p3.clone()); cx.notify(); }))
                    .child(div().w(px(12.)).flex_none().when(row.is_dir, |d| d.child(icon("chevron", 10., t.faint).when(row.expanded, |s| s.with_transformation(Transformation::rotate(percentage(0.25)))))))
                    .child(if row.is_dir { icon("folder", 14., t.dim).into_any_element() } else { file_icon(&row.name, cx).into_any_element() })
                    .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                    .when_some(st, |d, s| d.child(div().text_size(px(10.)).font_family(MONO_FONT).font_weight(FontWeight::BOLD).text_color(match s { 'D' => t.del, 'M' => t.modified, _ => t.add }).child(if s == '?' { "U".to_string() } else { s.to_string() })))
            }).collect::<Vec<_>>()
        })).track_scroll(self.tree_scroll.clone()).flex_1().pb(px(10.)).into_any_element()
    }

    fn search_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let n = self.search_hits.len();
        div().flex_1().min_h_0().flex().flex_col()
            .child(div().px(px(10.)).pb(px(8.)).child(div().h(px(30.)).px(px(10.)).flex().items_center().rounded(px(8.)).bg(t.elev).border_1().border_color(t.border).child(div().flex_1().child(self.search_input.clone()))))
            .child(if n == 0 {
                let q = self.search_input.read(cx).text();
                div().p(px(20.)).text_color(t.faint).flex().justify_center().child(if q.is_empty() { "输入关键字开始搜索" } else { "没有结果" }).into_any_element()
            } else {
                uniform_list("search-hits", n, cx.processor(|this, range: std::ops::Range<usize>, _w, cx| {
                    let t = cx.theme().clone();
                    range.map(|ix| {
                        let h = this.search_hits[ix].clone();
                        let (path, line) = (h.path.clone(), h.line);
                        div().id(("hit", ix)).h(px(40.)).mx(px(6.)).px(px(10.)).flex().flex_col().justify_center().rounded(px(6.)).cursor_pointer().hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |this, _, window, cx| { if let Some(r) = this.root.clone() { this.open_file(&r.join(&path).to_string_lossy(), Some(line), window, cx); } }))
                            .child(div().flex().gap(px(6.)).text_size(px(12.)).child(div().font_weight(FontWeight::MEDIUM).text_color(t.text).child(h.path.rsplit('/').next().unwrap_or(&h.path).to_string())).child(div().text_color(t.faint).truncate().child(format!(":{}  {}", h.line, h.path))))
                            .child(div().font_family(MONO_FONT).text_size(px(11.5)).text_color(t.dim).truncate().child(h.text.trim().to_string()))
                    }).collect::<Vec<_>>()
                })).track_scroll(self.search_scroll.clone()).flex_1().into_any_element()
            })
    }

    fn git_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div().flex_1().min_h_0().flex().flex_col().px(px(6.))
            .child(div().px(px(8.)).py(px(6.)).flex().items_center().gap(px(6.)).child(icon("branch", 14., t.dim)).child(div().font_weight(FontWeight::MEDIUM).child(self.branch.clone().unwrap_or_else(|| "非 Git 仓库".into()))).child(div().text_color(t.faint).child(format!("· {} 处改动", self.changes.len()))))
            .children(self.changes.iter().enumerate().map(|(i, c)| {
                let path = c.path.clone();
                let name = c.path.rsplit('/').next().unwrap_or(&c.path).to_string();
                let col = match c.status { 'D' => t.del, 'M' => t.modified, _ => t.add };
                div().id(("chg", i)).h(px(26.)).px(px(8.)).flex().items_center().gap(px(8.)).rounded(px(6.)).cursor_pointer().text_color(t.dim).hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(move |this, _, window, cx| { if let Some(r) = this.root.clone() { this.open_file(&r.join(&path).to_string_lossy(), None, window, cx); } }))
                    .child(file_icon(&name, cx)).child(div().flex_1().min_w_0().truncate().child(c.path.clone())).child(div().text_size(px(10.)).font_family(MONO_FONT).font_weight(FontWeight::BOLD).text_color(col).child(if c.status == '?' { "U".to_string() } else { c.status.to_string() }))
            }))
            .when(self.changes.is_empty(), |d| d.child(div().p(px(20.)).text_color(t.faint).flex().justify_center().child("工作区干净")))
    }

    fn tabs_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div().h(px(38.)).flex_none().flex().items_end().px(px(6.)).gap(px(2.)).bg(t.panel).border_b_1().border_color(t.border).overflow_hidden().children(self.tabs.iter().enumerate().map(|(i, tab)| {
            let on = self.active == Some(i);
            let dirty = tab.editor.read(cx).is_dirty();
            let reviewing = tab.editor.read(cx).in_review();
            let name = tab.path.rsplit('/').next().unwrap_or(&tab.path).to_string();
            let p = tab.path.clone();
            div().id(("tab", i)).h(px(32.)).max_w(px(220.)).pl(px(12.)).pr(px(6.)).flex().items_center().gap(px(8.)).rounded_tl(px(9.)).rounded_tr(px(9.)).cursor_pointer().relative()
                .text_color(if on { t.text } else { t.dim }).when(on, |d| d.bg(t.bg)).hover(|s| s.bg(if on { t.bg } else { t.hover }).text_color(t.text))
                .on_click(cx.listener(move |this, _, window, cx| this.open_file(&p, None, window, cx)))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab(i, window, cx)))
                .when(on, |d| d.child(div().absolute().top_0().left(px(10.)).right(px(10.)).h(px(2.)).rounded(px(2.)).bg(t.accent)))
                .child(file_icon(&name, cx))
                .child(div().truncate().child(name))
                .child(div().id(("tx", i)).size(px(18.)).rounded(px(5.)).flex().items_center().justify_center().text_color(t.faint).hover(|s| s.bg(t.press).text_color(t.text))
                    .on_click(cx.listener(move |this, _, window, cx| { cx.stop_propagation(); this.close_tab(i, window, cx); }))
                    .child(if dirty || reviewing { div().size(px(7.)).rounded_full().bg(t.accent).into_any_element() } else { icon("x", 12., t.faint).into_any_element() }))
        }))
    }

    fn find_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let (n, cur) = self.active_editor().map(|e| e.update(cx, |e, _| { let n = e.matches().len(); (n, e.search_current) })).unwrap_or((0, 0));
        div().absolute().top(px(10.)).right(px(22.)).h(px(34.)).pl(px(12.)).pr(px(6.)).flex().items_center().gap(px(8.)).rounded(px(10.)).bg(t.elev).border_1().border_color(t.border_strong)
            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(8.)), blur_radius: px(24.), spread_radius: px(0.) }])
            .child(div().w(px(190.)).child(self.find_input.clone()))
            .child(div().text_size(px(12.)).text_color(t.faint).min_w(px(44.)).child(if n == 0 { "无结果".to_string() } else { format!("{}/{}", cur + 1, n) }))
            .child(icon_button("find-x", "x", false, cx).size(px(24.)).on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))))
    }

    fn review_bar(&self, ed: &Entity<Editor>, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = cx.theme().clone();
        let n = ed.update(cx, |e, _| if e.in_review() { Some(e.review_hunk_count()) } else { None })?;
        let (e1, e2) = (ed.clone(), ed.clone());
        Some(
            div().absolute().bottom(px(18.)).left_0().right_0().flex().justify_center().child(
                div().flex().items_center().gap(px(8.)).pl(px(14.)).pr(px(6.)).h(px(36.)).rounded(px(12.)).bg(t.elev).border_1().border_color(t.accent_line)
                    .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.45), offset: point(px(0.), px(10.)), blur_radius: px(28.), spread_radius: px(0.) }])
                    .child(div().text_color(t.dim).child("审阅 AI 改动 ·")).child(div().text_color(t.accent).font_weight(FontWeight::BOLD).child(format!("{n} 处")))
                    .child(btn("rv-reject", "全部拒绝", false, cx).h(px(26.)).on_click(move |_, _, cx| { e1.update(cx, |e, cx| e.reject_all(cx)); }))
                    .child(btn("rv-accept", "全部接受 ⌘↵", true, cx).h(px(26.)).on_click(move |_, _, cx| { e2.update(cx, |e, cx| e.accept_all(cx)); })),
            ),
        )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let ed = self.active_editor();
        let (pos, lang) = ed.as_ref().map(|e| { let e = e.read(cx); (Some(e.cursor_line_col()), syntax::lang_label(e.lang)) }).unwrap_or((None, ""));
        let provider = if self.settings.provider == "mock" { "演示模式".to_string() } else { self.settings.model.clone() };
        div().h(px(26.)).flex_none().flex().items_center().gap(px(14.)).px(px(12.)).bg(t.panel).border_t_1().border_color(t.border).text_size(px(12.)).text_color(t.dim)
            .when_some(self.branch.clone(), |d, b| d.child(div().flex().items_center().gap(px(5.)).child(icon("branch", 13., t.dim)).child(b)))
            .when(!self.changes.is_empty(), |d| d.child(div().id("st-chg").cursor_pointer().hover(|s| s.text_color(t.text)).on_click(cx.listener(|this, _, _, cx| { this.sidebar = true; this.side_view = SideView::Git; cx.notify(); })).child(format!("{} 处改动", self.changes.len()))))
            .child(div().flex_1())
            .when_some(pos, |d, (l, c)| d.child(format!("行 {l}，列 {c}")).child(lang))
            .child(div().id("st-ai").flex().items_center().gap(px(6.)).cursor_pointer().hover(|s| s.text_color(t.text)).on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))
                .child(div().size(px(6.)).rounded_full().bg(if self.settings.provider == "mock" { t.modified } else { t.add })).child(provider))
    }

    fn welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div().flex_1().flex().items_center().px(px(90.)).bg(t.bg).child(
            div().w(px(560.)).flex().flex_col().gap(px(10.))
                .child(div().flex().items_end().child(div().text_size(px(48.)).font_weight(FontWeight::BOLD).line_height(px(52.)).child("Quill")).child(div().text_size(px(48.)).font_weight(FontWeight::BOLD).line_height(px(52.)).text_color(t.accent).child(".")))
                .child(div().text_size(px(15.)).text_color(t.dim).mb(px(18.)).child("用对话改代码。每一处改动，你都看得见、收得回。"))
                .child(div().flex().gap(px(8.)).mb(px(18.))
                    .child(btn("w-open", "打开文件夹", true, cx).h(px(34.)).px(px(16.)).on_click(cx.listener(|this, _, window, cx| this.pick_folder(window, cx))))
                    .child(btn("w-set", "设置", false, cx).h(px(34.)).px(px(16.)).on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))))
                .when(!self.recents.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(t.faint).child("最近打开")).children(self.recents.iter().take(6).enumerate().map(|(i, r)| {
                    let p = PathBuf::from(r);
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let home = std::env::var("HOME").unwrap_or_default();
                    div().id(("recent", i)).flex().gap(px(12.)).px(px(10.)).py(px(6.)).rounded(px(8.)).cursor_pointer().hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.open_root(p.clone(), window, cx)))
                        .child(div().child(name)).child(div().text_color(t.faint).truncate().child(r.replacen(&home, "~", 1)))
                })))
                .child(div().mt(px(20.)).grid().grid_cols(2).gap(px(6.)).text_color(t.dim)
                    .child(kbd("⌘P", cx)).child("跳转到文件").child(kbd("⇧⌘P", cx)).child("命令面板").child(kbd("⌘L", cx)).child("呼出对话").child(kbd("⌘K", cx)).child("就地改写选中代码")),
        )
    }

    fn context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.menu.as_ref()?;
        let t = cx.theme().clone();
        let (path, is_dir) = (m.path.clone(), m.is_dir);
        let dir = if is_dir { path.clone() } else { Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default() };
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let items: Vec<(&str, usize)> = vec![("新建文件", 0), ("新建文件夹", 1), ("重命名", 2), ("删除", 3), ("在访达中显示", 4)];
        Some(
            div().absolute().inset_0().id("ctx-scrim").occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| { this.menu = None; cx.notify(); }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| { this.menu = None; cx.notify(); }))
                .child(
                    div().absolute().left(m.pos.x).top(m.pos.y).w(px(180.)).p(px(5.)).rounded(px(11.)).bg(t.elev).border_1().border_color(t.border_strong)
                        .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.45), offset: point(px(0.), px(10.)), blur_radius: px(28.), spread_radius: px(0.) }])
                        .children(items.into_iter().map(|(label, k)| {
                            let (path, dir, name) = (path.clone(), dir.clone(), name.clone());
                            div().id(("ctx", k)).h(px(28.)).px(px(10.)).flex().items_center().rounded(px(7.)).cursor_pointer().text_color(if k == 3 { t.del } else { t.text }).hover(|s| s.bg(t.hover)).child(label)
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.menu = None;
                                    match k {
                                        0 => this.prompt("新建文件", "", PromptKind::NewFile(dir.clone()), window, cx),
                                        1 => this.prompt("新建文件夹", "", PromptKind::NewFolder(dir.clone()), window, cx),
                                        2 => this.prompt("重命名", &name, PromptKind::Rename(path.clone()), window, cx),
                                        3 => this.prompt(&format!("删除「{name}」？此操作不可撤销"), "", PromptKind::ConfirmDelete(path.clone()), window, cx),
                                        _ => { let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn(); }
                                    }
                                    cx.notify();
                                }))
                        })),
                )
                .into_any_element(),
        )
    }

    fn handle_h(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div().id("h-term").h(px(1.)).flex_none().relative().bg(t.border).cursor_row_resize()
            .child(div().absolute().left_0().right_0().top(px(-3.)).h(px(7.)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| { this.resizing = Some((Resize::Term, f32::from(ev.position.y), this.term_h)); cx.notify(); }))
    }

    fn handle(&self, id: &'static str, which: Resize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let active = self.resizing.map(|r| r.0 == which).unwrap_or(false);
        div().id(id).w(px(1.)).flex_none().relative().bg(t.border).cursor_col_resize()
            .child(div().absolute().top_0().bottom_0().left(px(-3.)).w(px(7.)).when(active, |d| d.child(div().absolute().top_0().bottom_0().left(px(3.)).w(px(2.)).bg(t.accent)))
                .hover(|s| s.bg(t.accent_soft)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                let (pos, w) = match which { Resize::Side => (f32::from(ev.position.x), this.side_w), Resize::Agent => (f32::from(ev.position.x), this.agent_w), Resize::Term => (f32::from(ev.position.y), this.term_h) };
                this.resizing = Some((which, pos, w));
                cx.notify();
            }))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        // 面板开合动画：每帧向目标靠近
        let (st, at) = (if self.sidebar && self.root.is_some() { 1.0 } else { 0.0 }, if self.agent_open { 1.0 } else { 0.0 });
        let mut animating = false;
        let tt = if self.term_open { 1.0 } else { 0.0 };
        for (cur, target) in [(&mut self.side_t, st), (&mut self.agent_t, at), (&mut self.term_t, tt)] {
            let d = target - *cur;
            if d.abs() > 0.003 { *cur += d * 0.28; animating = true; } else { *cur = target; }
        }
        if animating { window.request_animation_frame(); }

        let editor = self.active_editor();
        let review = editor.as_ref().and_then(|e| self.review_bar(e, cx));
        let center = if self.root.is_none() {
            self.welcome(cx).into_any_element()
        } else {
            div().flex_1().min_w_0().flex().flex_col().bg(t.bg)
                .child(self.tabs_bar(cx))
                .child(match &editor {
                    Some(e) => div().flex_1().min_h_0().relative().child(e.clone()).when(self.find_open, |d| d.child(self.find_bar(cx))).children(review).when_some(self.inline.clone(), |d, i| d.child(i)).into_any_element(),
                    None => div().flex_1().flex().items_center().justify_center().flex_col().gap(px(6.)).child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).child("选一个文件开始")).child(div().text_color(t.dim).child("⌘P 跳转文件 · ⌘L 和 AI 对话")).into_any_element(),
                })
                .when(self.term_t > 0.01, |d| d
                    .child(self.handle_h(cx))
                    .child(div().flex_none().h(px(self.term_h * self.term_t)).overflow_hidden().bg(t.bg).flex().flex_col()
                        .child(div().h(px(30.)).flex_none().px(px(14.)).flex().items_center().gap(px(8.)).text_color(t.dim).text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(icon("terminal", 14., t.dim)).child("终端").child(div().flex_1())
                            .child(icon_button("term-x", "x", false, cx).size(px(22.)).on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(window, cx)))))
                        .child(div().flex_1().min_h_0().children(self.terminal.clone()))))
                .into_any_element()
        };

        let overlay = match &self.overlay {
            Overlay::Palette(p) => Some(p.clone().into_any_element()),
            Overlay::Settings(s) => Some(s.clone().into_any_element()),
            Overlay::Prompt(p) => Some(p.clone().into_any_element()),
            Overlay::None => None,
        };

        div().id("workspace").size_full().flex().flex_col().relative().bg(t.bg).text_color(t.text).font_family(UI_FONT).text_size(px(13.)).key_context("Workspace").track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save_active(cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| { if let Some(a) = this.active { this.close_tab(a, window, cx); } }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| { if this.tabs.len() > 1 { let n = (this.active.unwrap_or(0) + 1) % this.tabs.len(); let p = this.tabs[n].path.clone(); this.open_file(&p, None, window, cx); } }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| { if this.tabs.len() > 1 { let n = (this.active.unwrap_or(0) + this.tabs.len() - 1) % this.tabs.len(); let p = this.tabs[n].path.clone(); this.open_file(&p, None, window, cx); } }))
            .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.open_palette(PaletteMode::Files, window, cx)))
            .on_action(cx.listener(|this, _: &CommandPalette, window, cx| this.open_palette(PaletteMode::Commands, window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| { this.sidebar = !this.sidebar; cx.notify(); }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| this.toggle_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleAgent, _, cx| { this.agent_open = !this.agent_open; cx.notify(); }))
            .on_action(cx.listener(|this, _: &FocusAgent, window, cx| this.focus_agent(window, cx)))
            .on_action(cx.listener(|this, _: &ProjectSearch, window, cx| { this.sidebar = true; this.side_view = SideView::Search; window.focus(&this.search_input.read(cx).focus); cx.notify(); }))
            .on_action(cx.listener(|this, _: &ShowFiles, _, cx| { this.sidebar = true; this.side_view = SideView::Files; cx.notify(); }))
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| this.pick_folder(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)))
            .on_action(cx.listener(|this, _: &NewThread, _, cx| this.agent.update(cx, |a, cx| a.new_thread(cx))))
            .on_action(cx.listener(|this, _: &FindInFile, window, cx| this.open_find(window, cx)))
            .on_action(cx.listener(|this, _: &ApplyLatest, _, cx| this.agent.update(cx, |a, cx| a.apply_latest(cx))))
            .on_action(cx.listener(|this, _: &AcceptReview, _, cx| { if let Some(e) = this.active_editor() { e.update(cx, |e, cx| e.accept_all(cx)); } }))
            .on_action(cx.listener(|this, _: &RejectReview, _, cx| { if let Some(e) = this.active_editor() { e.update(cx, |e, cx| e.reject_all(cx)); } }))
            .on_action(cx.listener(|this, _: &InlineAssist, window, cx| this.open_inline(window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if let Some((which, x0, w0)) = this.resizing {
                    let dx = f32::from(ev.position.x) - x0;
                    let dy = f32::from(ev.position.y) - x0;
                    let total = f32::from(window.viewport_size().width);
                    match which {
                        Resize::Term => this.term_h = (w0 - dy).clamp(120.0, 620.0),
                        Resize::Side => this.side_w = (w0 + dx).clamp(180.0, 520.0),
                        Resize::Agent => this.agent_w = (w0 - dx).clamp(320.0, (total * 0.6).max(360.0)),
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| { if this.resizing.take().is_some() { cx.notify(); } }))
            .child(self.titlebar(cx))
            .child(
                div().flex_1().min_h_0().flex()
                    .child(div().flex_none().w(px(self.side_w * self.side_t)).overflow_hidden().bg(t.panel).opacity(self.side_t.min(1.0))
                        .child(div().w(px(self.side_w)).h_full().child(self.sidebar_view(cx))))
                    .when(self.side_t > 0.99, |d| d.child(self.handle("h-side", Resize::Side, cx)))
                    .child(center)
                    .when(self.agent_t > 0.99, |d| d.child(self.handle("h-agent", Resize::Agent, cx)))
                    .child(div().flex_none().w(px(self.agent_w * self.agent_t)).overflow_hidden().bg(t.panel).opacity(self.agent_t.min(1.0))
                        .child(div().w(px(self.agent_w)).h_full().child(self.agent.clone()))),
            )
            .child(self.status_bar(cx))
            .children(self.context_menu(cx))
            .children(overlay)
            .child(div().absolute().bottom(px(40.)).left_0().right_0().flex().flex_col().items_center().gap(px(8.)).children(self.toasts.iter().map(|ts| {
                div().px(px(16.)).py(px(9.)).rounded(px(11.)).bg(t.elev).border_1().border_color(if ts.error { t.del } else { t.border_strong }).text_color(if ts.error { t.del } else { t.text })
                    .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.4), offset: point(px(0.), px(8.)), blur_radius: px(24.), spread_radius: px(0.) }]).child(ts.text.clone())
            })))
    }
}

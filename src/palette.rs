//! 命令面板 / 文件跳转（⌘P、⌘⇧P）。
use crate::actions::*;
use crate::editor::{Editor, EditorEvent, EditorMode};
use crate::fuzzy::fuzzy_match;
use crate::theme::{ActiveTheme, MONO_FONT};
use crate::ui::{file_icon, kbd};
use gpui::{prelude::*, *};

pub enum PaletteEvent {
    Close,
    OpenFile(String),
    Command(usize),
    GotoLine(usize),
}

#[derive(Clone, Copy, PartialEq)]
pub enum PaletteMode { Files, Commands, Symbols, Line }

pub struct CommandItem {
    pub title: &'static str,
    pub keys: &'static str,
}

#[derive(Clone)]
struct Item {
    label: String,
    sub: String,
    keys: String,
    hits: Vec<usize>,
    is_file: bool,
    id: usize,
}

pub struct Palette {
    input: Entity<Editor>,
    mode: PaletteMode,
    files: Vec<String>,
    open_files: Vec<String>,
    commands: Vec<CommandItem>,
    /// (名称, 类型标签, 行号)
    symbols: Vec<(String, &'static str, usize)>,
    items: Vec<Item>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _sub: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle { self.input.read(cx).focus.clone() }
}

impl Palette {
    pub fn new(mode: PaletteMode, files: Vec<String>, open_files: Vec<String>, commands: Vec<CommandItem>, symbols: Vec<(String, &'static str, usize)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ph = match mode { PaletteMode::Files => "按文件名跳转…", PaletteMode::Commands => "输入命令…", PaletteMode::Symbols => "跳转到符号…", PaletteMode::Line => "输入行号，回车跳转" };
        let input = cx.new(|cx| Editor::new(EditorMode::input(ph), cx));
        let sub = cx.subscribe(&input, |this, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Changed => { this.refilter(cx); }
            EditorEvent::Submit => this.confirm(cx),
            EditorEvent::Cancel => cx.emit(PaletteEvent::Close),
            _ => {}
        });
        window.focus(&input.read(cx).focus);
        let mut p = Self { input, mode, files, open_files, commands, symbols, items: vec![], selected: 0, scroll: UniformListScrollHandle::new(), _sub: sub };
        p.refilter(cx);
        p
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text();
        let mut scored: Vec<(f32, Item)> = vec![];
        match self.mode {
            PaletteMode::Files => {
                for (i, f) in self.files.iter().enumerate() {
                    let m = if q.is_empty() { Some((if self.open_files.contains(f) { 1.0 } else { 0.0 }, vec![])) } else { fuzzy_match(&q, f) };
                    if let Some((s, hits)) = m {
                        let (dir, name) = f.rsplit_once('/').map(|(d, n)| (d.to_string(), n.to_string())).unwrap_or((String::new(), f.clone()));
                        scored.push((s, Item { label: name, sub: dir, keys: String::new(), hits, is_file: true, id: i }));
                    }
                }
            }
            PaletteMode::Symbols => {
                for (i, (name, kind, line)) in self.symbols.iter().enumerate() {
                    if let Some((s, hits)) = fuzzy_match(&q, name) {
                        scored.push((s - i as f32 * 0.001, Item { label: name.clone(), sub: format!("{kind} · 第 {} 行", line + 1), keys: String::new(), hits, is_file: false, id: i }));
                    }
                }
            }
            PaletteMode::Line => {}
            PaletteMode::Commands => {
                for (i, c) in self.commands.iter().enumerate() {
                    if let Some((s, hits)) = fuzzy_match(&q, c.title) {
                        scored.push((s, Item { label: c.title.to_string(), sub: String::new(), keys: c.keys.to_string(), hits, is_file: false, id: i }));
                    }
                }
            }
        }
        if !q.is_empty() || self.mode == PaletteMode::Files { scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)); }
        self.items = scored.into_iter().take(80).map(|(_, i)| i).collect();
        self.selected = 0;
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.mode == PaletteMode::Line {
            if let Ok(n) = self.input.read(cx).text().trim().parse::<usize>() { cx.emit(PaletteEvent::GotoLine(n.saturating_sub(1))); }
            return;
        }
        let Some(it) = self.items.get(self.selected).cloned() else { return };
        match self.mode {
            PaletteMode::Files => cx.emit(PaletteEvent::OpenFile(self.files[it.id].clone())),
            PaletteMode::Commands => cx.emit(PaletteEvent::Command(it.id)),
            PaletteMode::Symbols => cx.emit(PaletteEvent::GotoLine(self.symbols[it.id].2)),
            PaletteMode::Line => {}
        }
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() { return; }
        let n = self.items.len() as isize;
        self.selected = ((self.selected as isize + d).rem_euclid(n)) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let count = self.items.len();
        let line_mode = self.mode == PaletteMode::Line;
        div()
            .id("palette-scrim")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(110.))
            .bg(t.scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.step(1, cx)))
            .child(
                div()
                    .id("palette")
                    .w(px(640.))
                    .max_h(px(460.))
                    .flex()
                    .flex_col()
                    .rounded(px(16.))
                    .bg(t.elev)
                    .border_1()
                    .border_color(t.border_strong)
                    .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.5), offset: point(px(0.), px(18.)), blur_radius: px(50.), spread_radius: px(0.) }])
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().h(px(52.)).px(px(20.)).flex().items_center().border_b_1().border_color(t.border).child(div().flex_1().child(self.input.clone())))
                    .child(if line_mode {
                        div().px(px(20.)).py(px(10.)).text_size(px(12.)).text_color(t.faint).child("输入行号后按回车").into_any_element()
                    } else if count == 0 {
                        div().p(px(24.)).text_color(t.faint).flex().justify_center().child("没有匹配项").into_any_element()
                    } else {
                        uniform_list("palette-items", count, cx.processor(|this, range: std::ops::Range<usize>, _w, cx| {
                            let t = cx.theme().clone();
                            range.map(|ix| {
                                let it = this.items[ix].clone();
                                let sel = ix == this.selected;
                                let name_hits: Vec<usize> = it.hits.iter().copied().filter(|h| !it.is_file || *h >= it.sub.chars().count() + usize::from(!it.sub.is_empty())).map(|h| if it.is_file { h - (it.sub.chars().count() + usize::from(!it.sub.is_empty())) } else { h }).collect();
                                let label_byte_hits: Vec<(std::ops::Range<usize>, HighlightStyle)> = {
                                    let idx: Vec<(usize, char)> = it.label.char_indices().collect();
                                    name_hits.iter().filter_map(|&h| idx.get(h).map(|&(b, c)| (b..b + c.len_utf8(), HighlightStyle { color: Some(t.accent), font_weight: Some(FontWeight::BOLD), ..Default::default() }))).collect()
                                };
                                div()
                                    .id(("pi", ix))
                                    .h(px(36.))
                                    .mx(px(6.))
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .rounded(px(9.))
                                    .cursor_pointer()
                                    .text_color(if sel { t.text } else { t.dim })
                                    .when(sel, |d| d.bg(t.accent_soft))
                                    .on_mouse_move(cx.listener(move |this, _, _, cx| { if this.selected != ix { this.selected = ix; cx.notify(); } }))
                                    .on_click(cx.listener(move |this, _, _, cx| { this.selected = ix; this.confirm(cx); }))
                                    .when(it.is_file, |d| d.child(file_icon(&it.label, cx)))
                                    .child(div().flex_none().child(StyledText::new(it.label.clone()).with_highlights(label_byte_hits)))
                                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(t.faint).font_family(MONO_FONT).child(it.sub.clone()))
                                    .when(!it.keys.is_empty(), |d| d.child(kbd(&it.keys, cx)))
                            }).collect::<Vec<_>>()
                        }))
                        .track_scroll(self.scroll.clone())
                        .my(px(6.))
                        .h(px(count.min(10) as f32 * 36.0 + 4.0))
                        .into_any_element()
                    }),
            )
    }
}

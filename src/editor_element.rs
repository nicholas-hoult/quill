//! 编辑器绘制：只排版可见行（gpui 内部按文本+样式缓存排版结果），自己画选区、光标、行号、审阅高亮。
use crate::actions::*;
use crate::editor::{Editor, EditorEvent, RowKind};
use crate::theme::{ActiveTheme, MONO_FONT, UI_FONT};
use gpui::{prelude::*, *};
use std::ops::Range;

#[derive(Default)]
pub struct LastLayout {
    pub bounds: Bounds<Pixels>,
    pub text_left: Pixels,
    pub rows: Vec<VisRow>,
    pub hunk_buttons: Vec<HunkButton>,
    pub total_h: f32,
    pub gutter_w: f32,
}

pub struct VisRow {
    pub y: Pixels,
    pub h: Pixels,
    pub kind: RowKind,
    pub line_start: usize,
    pub len: usize,
    pub tabs: Vec<usize>,
    pub shaped: Option<WrappedLine>,
}

pub struct HunkButton {
    pub rect: Bounds<Pixels>,
    pub hunk: usize,
    pub accept: bool,
}

fn to_display(tabs: &[usize], i: usize) -> usize { i + 3 * tabs.iter().filter(|&&t| t < i).count() }

fn from_display(tabs: &[usize], d: usize) -> usize {
    let mut extra = 0;
    for &t in tabs {
        let dt = t + extra;
        if d >= dt + 4 { extra += 3; } else if d > dt { return t + 1; } else { break; }
    }
    d.saturating_sub(extra)
}

impl LastLayout {
    /// 某字节偏移在窗口中的坐标（用于输入法候选框定位）
    pub fn position_of(&self, offset: usize, lh: f32) -> Option<(Pixels, Pixels)> {
        for r in &self.rows {
            if let RowKind::Line(_) = r.kind {
                if offset >= r.line_start && offset <= r.line_start + r.len {
                    let p = match &r.shaped {
                        Some(s) => s.position_for_index(to_display(&r.tabs, offset - r.line_start), px(lh)).unwrap_or(point(px(0.), px(0.))),
                        None => point(px(0.), px(0.)),
                    };
                    return Some((self.text_left + p.x, r.y + p.y));
                }
            }
        }
        None
    }

    /// 窗口坐标 → 字节偏移
    pub fn offset_at(&self, p: Point<Pixels>, lh: f32, doc_len: usize) -> Option<usize> {
        let first = self.rows.first()?;
        if p.y < first.y { return Some(self.first_line_offset()); }
        let mut last_line_end = doc_len;
        for (i, r) in self.rows.iter().enumerate() {
            if p.y >= r.y && p.y < r.y + r.h {
                return Some(match &r.kind {
                    RowKind::Line(_) => match &r.shaped {
                        Some(s) => {
                            let idx = match s.closest_index_for_position(point(p.x - self.text_left, p.y - r.y), px(lh)) { Ok(i) | Err(i) => i };
                            r.line_start + from_display(&r.tabs, idx).min(r.len)
                        }
                        None => r.line_start,
                    },
                    RowKind::Removed { .. } => self.rows[i..].iter().find_map(|n| if let RowKind::Line(_) = n.kind { Some(n.line_start) } else { None }).unwrap_or(doc_len),
                });
            }
            if let RowKind::Line(_) = r.kind { last_line_end = r.line_start + r.len; }
        }
        Some(last_line_end.max(if p.y > first.y { last_line_end } else { 0 }))
    }

    fn first_line_offset(&self) -> usize { self.rows.iter().find_map(|r| if let RowKind::Line(_) = r.kind { Some(r.line_start) } else { None }).unwrap_or(0) }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key = if self.review.is_some() { "Editor Review" } else { "Editor" };
        let root = div()
            .id("editor")
            .key_context(key)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::move_to_line_start))
            .on_action(cx.listener(Self::move_to_line_end))
            .on_action(cx.listener(Self::select_to_line_start))
            .on_action(cx.listener(Self::select_to_line_end))
            .on_action(cx.listener(Self::move_to_doc_start))
            .on_action(cx.listener(Self::move_to_doc_end))
            .on_action(cx.listener(Self::select_to_doc_start))
            .on_action(cx.listener(Self::select_to_doc_end))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::newline_action))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|e, _: &Copy, w, cx| e.copy(&Copy, w, cx)))
            .on_action(cx.listener(|e, _: &Cut, w, cx| e.cut(w, cx)))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::duplicate_line))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::toggle_wrap))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_all_occurrences))
            .on_action(cx.listener(Self::fold_at_cursor))
            .on_action(cx.listener(Self::unfold_at_cursor))
            .on_action(cx.listener(|_, _: &ShowCharacterPalette, w, _| w.show_character_palette()))
            .on_action(cx.listener(|_, _: &InlineAssist, _, cx| cx.emit(EditorEvent::InlineAssist)));
        let lh = self.mode.line_height;
        match self.mode.max_lines {
            Some(max) => {
                let content = if self.mode.wrap { self.layout.total_h.max(lh) } else { (self.buffer.line_count() as f32 * lh).max(lh) };
                root.w_full().h(px(content.min(max as f32 * lh) + self.mode.padding * 2.0)).child(EditorElement { editor: cx.entity() })
            }
            None => root.size_full().child(EditorElement { editor: cx.entity() }),
        }
    }
}

pub trait PxF { fn f(self) -> f32; }
impl PxF for Pixels { fn f(self) -> f32 { f32::from(self) } }

/// 开发用：QUILL_PERF=1 时每 90 帧在 stderr 打印编辑器 prepaint / paint 的平均耗时（毫秒）
pub static AFTER_OPEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 当前线程已用的 CPU 时间（微秒）。机器被别的进程占满时墙钟时间不可信，CPU 时间基本不受影响
fn thread_cpu_us() -> u64 {
    #[repr(C)] struct Ts { sec: i64, nsec: i64 }
    unsafe extern "C" { fn clock_gettime(clk: u32, ts: *mut Ts) -> i32; }
    let mut ts = Ts { sec: 0, nsec: 0 };
    // macOS 的 CLOCK_THREAD_CPUTIME_ID = 16
    unsafe { clock_gettime(16, &mut ts); }
    (ts.sec as u64) * 1_000_000 + (ts.nsec as u64) / 1000
}

pub struct PerfT(std::time::Instant, u64);
impl PerfT { fn now() -> Self { PerfT(std::time::Instant::now(), thread_cpu_us()) } }

fn perf_record(kind: usize, t0: PerfT) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("QUILL_PERF").is_ok()) { return; }
    static US: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    static N: AtomicU64 = AtomicU64::new(0);
    let us = t0.0.elapsed().as_micros() as u64;
    let cpu = thread_cpu_us().saturating_sub(t0.1);
    if AFTER_OPEN.load(Ordering::Relaxed) > 0 { AFTER_OPEN.fetch_sub(1, Ordering::Relaxed); eprintln!("[perf] 打开后的帧 {} 墙钟 {:.1}ms CPU {:.1}ms", if kind == 0 { "prepaint" } else { "paint" }, us as f64 / 1000.0, cpu as f64 / 1000.0); }
    if us > 4000 { eprintln!("[perf] 慢帧 {} {:.1}ms", if kind == 0 { "prepaint" } else { "paint" }, us as f64 / 1000.0); }
    US[kind].fetch_add(us, Ordering::Relaxed);
    if kind == 1 && N.fetch_add(1, Ordering::Relaxed) % 90 == 89 {
        eprintln!("[perf] 90 帧平均：prepaint {:.2}ms  paint {:.2}ms", US[0].swap(0, Ordering::Relaxed) as f64 / 90_000.0, US[1].swap(0, Ordering::Relaxed) as f64 / 90_000.0);
    }
}

pub struct EditorElement {
    pub editor: Entity<Editor>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element { self }
}

pub struct Prepaint {
    hitbox: Hitbox,
    gutter_w: f32,
    numbers: Vec<(Pixels, ShapedLine, bool)>,
    cursor: Option<Bounds<Pixels>>,
    marks: Vec<(Bounds<Pixels>, Hsla)>,
    row_bgs: Vec<(Bounds<Pixels>, Hsla)>,
    thumb: Option<Bounds<Pixels>>,
    buttons: Vec<(Bounds<Pixels>, ShapedLine, bool)>,
    placeholder: Option<ShapedLine>,
    focused: bool,
    eol_marks: Vec<Bounds<Pixels>>,
    dots: Vec<(Bounds<Pixels>, Hsla)>,
    extra_cursors: Vec<Bounds<Pixels>>,
    caret_alpha: f32,
    chips: Vec<(Bounds<Pixels>, ShapedLine)>,
    chevrons: Vec<(Pixels, ShapedLine)>,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        let t0 = PerfT::now();
        let theme = cx.theme().clone();
        let focused = self.editor.read(cx).focus.is_focused(window);
        if self.editor.read(cx).was_focused != focused { self.editor.update(cx, |e, _| { e.was_focused = focused; if focused { e.blink_at = std::time::Instant::now(); } }); }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let editor = self.editor.clone();
        let out = editor.update(cx, |ed, cx| {
            if std::env::var("QUILL_BENCH").is_ok() && ed.buffer.line_count() > 200 {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    let lines: Vec<String> = (100..160).map(|l| ed.buffer.line_text(l)).collect();
                    let f = font(MONO_FONT);
                    let mk = |t: &str| TextRun { len: t.len(), font: f.clone(), color: theme.text, background_color: None, underline: None, strikethrough: None };
                    for round in 0..2 {
                        let t = std::time::Instant::now();
                        for l in &lines { let _ = window.text_system().shape_text(SharedString::from(format!("{l} ")), px(13.), &[mk(&format!("{l} "))], None, None); }
                        let a = t.elapsed().as_micros() as f64 / 60.0;
                        let t = std::time::Instant::now();
                        for l in &lines { let s = format!("{l}  "); let _ = window.text_system().shape_line(SharedString::from(s.clone()), px(13.), &[mk(&s)], None); }
                        let b = t.elapsed().as_micros() as f64 / 60.0;
                        eprintln!("[bench] 第{}轮 每行：shape_text {:.0}µs  shape_line {:.0}µs", round + 1, a, b);
                    }
                });
            }
            let t_ck = std::time::Instant::now();
            let mut cks: Vec<(&'static str, u128)> = vec![];
            let mode = ed.mode.clone();
            let lh = px(mode.line_height);
            let fsize = px(mode.font_size);
            let base_font = font(if mode.ui_font { UI_FONT } else { MONO_FONT });
            let num_font = font(MONO_FONT);
            let pad = px(mode.padding);
            ed.refresh_review();
            ed.sync_analysis(cx);
            if ed.search.is_some() { ed.matches(); }
            let head_hunks: Vec<crate::diff::Hunk> = if mode.gutter && ed.review.is_none() { ed.head_hunks().to_vec() } else { vec![] };

            // 行号栏宽度
            let digits = ed.buffer.line_count().to_string().len().max(2);
            let digit_w = {
                let l = window.text_system().shape_line("0".into(), px(mode.font_size - 1.0), &[TextRun { len: 1, font: num_font.clone(), color: theme.faint, background_color: None, underline: None, strikethrough: None }], None);
                l.width.f()
            };
            let gutter_w = if mode.gutter { digits as f32 * digit_w + 44.0 } else { 0.0 };
            let text_left = bounds.left() + px(gutter_w) + if mode.gutter { px(6.) } else { pad };
            let wrap_w = if mode.wrap { Some((bounds.right() - text_left - pad - px(6.)).max(px(40.))) } else { None };
            let view_h = bounds.size.height - pad * 2.0;
            let top = bounds.top() + pad;

            cks.push(("准备", t_ck.elapsed().as_micros()));
            // ---- 行模型 ----
            let review_rows: Option<Vec<RowKind>> = ed.review.as_ref().map(|r| r.rows.clone());
            let fold_map = ed.visible_lines();
            let row_count = review_rows.as_ref().map(|r| r.len()).or_else(|| fold_map.as_ref().map(|m| m.len())).unwrap_or_else(|| ed.buffer.line_count());
            let kind_at = |i: usize| -> RowKind { match (&review_rows, &fold_map) { (Some(r), _) => r[i].clone(), (None, Some(m)) => RowKind::Line(m[i]), _ => RowKind::Line(i) } };

            let total_h: f32;
            let mut heights: Vec<f32> = vec![];
            let mut shaped_all: Vec<Option<(WrappedLine, Vec<usize>)>> = vec![];
            if mode.wrap {
                // 自动换行只用于小文本（聊天输入框）：整体排版以得到每行高度
                for i in 0..row_count {
                    let (sh, tabs) = shape_row(ed, &theme, &base_font, fsize, wrap_w, &kind_at(i), &mode.placeholder, window);
                    let h = sh.as_ref().map(|s: &WrappedLine| s.size(lh).height.f()).unwrap_or(mode.line_height).max(mode.line_height);
                    heights.push(h);
                    shaped_all.push(sh.map(|s| (s, tabs)));
                }
                total_h = heights.iter().sum();
            } else {
                total_h = row_count as f32 * mode.line_height;
            }
            let max_scroll = (total_h - view_h.f()).max(0.0);

            // ---- 光标可见 ----
            let cursor = ed.cursor();
            let cur_line = ed.buffer.line_of(cursor);
            let cur_row = ed.review.as_ref().and_then(|r| r.line_row.get(cur_line).copied()).or_else(|| fold_map.as_ref().map(|m| match m.binary_search(&cur_line) { Ok(i) | Err(i) => i.min(m.len().saturating_sub(1)) })).unwrap_or(cur_line);
            let row_y = |i: usize| -> f32 { if mode.wrap { heights[..i.min(heights.len())].iter().sum() } else { i as f32 * mode.line_height } };
            if ed.reveal && !(mode.wrap && heights.is_empty()) {
                let y0 = row_y(cur_row);
                let hh = if mode.wrap { heights.get(cur_row).copied().unwrap_or(mode.line_height) } else { mode.line_height };
                let margin = if mode.max_lines.is_some() { 0.0 } else { mode.line_height * 2.0 };
                if y0 - margin < ed.scroll_y { ed.scroll_y = (y0 - margin).max(0.0); }
                if y0 + hh + margin > ed.scroll_y + view_h.f() { ed.scroll_y = y0 + hh + margin - view_h.f(); }
            }
            ed.scroll_y = ed.scroll_y.clamp(0.0, max_scroll);

            // ---- 可见行 ----
            let first = if mode.wrap {
                let mut acc = 0.0; let mut f = 0;
                for (i, h) in heights.iter().enumerate() { if acc + h > ed.scroll_y { f = i; break; } acc += h; f = i; }
                f
            } else { (ed.scroll_y / mode.line_height) as usize };
            let mut rows: Vec<VisRow> = vec![];
            let mut y_off = row_y(first);
            let mut i = first;
            let sel = ed.sel.clone();
            while i < row_count && y_off - ed.scroll_y < view_h.f() + mode.line_height {
                let kind = kind_at(i);
                let (shaped, tabs) = if mode.wrap {
                    match shaped_all[i].take() { Some((s, t)) => (Some(s), t), None => (None, vec![]) }
                } else {
                    let (s, t) = shape_row(ed, &theme, &base_font, fsize, wrap_w, &kind, &mode.placeholder, window);
                    (s, t)
                };
                let h = if mode.wrap { heights[i] } else { mode.line_height };
                let (ls, len) = match kind { RowKind::Line(l) => (ed.buffer.line_start(l), ed.buffer.line_end(l) - ed.buffer.line_start(l)), _ => (0, 0) };
                rows.push(VisRow { y: top + px(y_off - ed.scroll_y), h: px(h), kind, line_start: ls, len, tabs, shaped });
                y_off += h;
                i += 1;
            }

            cks.push(("排版可见行", t_ck.elapsed().as_micros()));
            // ---- 水平滚动：保持光标可见 ----
            if !mode.wrap {
                if let Some(r) = rows.iter().find(|r| matches!(r.kind, RowKind::Line(l) if l == cur_line)) {
                    let cx_px = r.shaped.as_ref().and_then(|s| s.position_for_index(to_display(&r.tabs, cursor.saturating_sub(r.line_start)), lh)).map(|p| p.x.f()).unwrap_or(0.0);
                    let vis_w = (bounds.right() - text_left).f() - 24.0;
                    if ed.reveal {
                        if cx_px - ed.scroll_x > vis_w { ed.scroll_x = cx_px - vis_w + 40.0; }
                        if cx_px - ed.scroll_x < 0.0 { ed.scroll_x = (cx_px - 40.0).max(0.0); }
                    }
                }
                if ed.scroll_x < 0.0 { ed.scroll_x = 0.0; }
            }
            ed.reveal = false;
            let scroll_x = px(ed.scroll_x);
            let text_origin_x = text_left - scroll_x;

            // ---- 行背景（当前行、审阅）----
            let mut row_bgs: Vec<(Bounds<Pixels>, Hsla)> = vec![];
            let mut marks: Vec<(Bounds<Pixels>, Hsla)> = vec![];
            let mut numbers: Vec<(Pixels, ShapedLine, bool)> = vec![];
            let mut eol_marks: Vec<Bounds<Pixels>> = vec![];
            let mut dots: Vec<(Bounds<Pixels>, Hsla)> = vec![];
            let mut chips: Vec<(Bounds<Pixels>, ShapedLine)> = vec![];
            let mut chevrons: Vec<(Pixels, ShapedLine)> = vec![];
            let gutter_hover = ed.gutter_hover;
            let mut line_sev: std::collections::HashMap<usize, u8> = Default::default();
            for d in &ed.diagnostics { let l = ed.buffer.line_of(d.range.start.min(ed.buffer.len())); let e = line_sev.entry(l).or_insert(d.severity); if d.severity < *e { *e = d.severity; } }
            let hunks_new: Vec<std::ops::Range<usize>> = ed.review.as_ref().map(|r| r.hunks.iter().map(|h| h.new.clone()).collect()).unwrap_or_default();
            let in_review = ed.review.is_some();
            for r in &rows {
                let full = Bounds::new(point(bounds.left(), r.y), size(bounds.size.width, r.h));
                match &r.kind {
                    RowKind::Line(l) => {
                        let is_cur = *l == cur_line;
                        if mode.highlight_active_line && is_cur && sel.is_empty() && !in_review { row_bgs.push((full, theme.line_active)); }
                        if in_review && hunks_new.iter().any(|h| h.contains(l)) {
                            row_bgs.push((full, theme.add_bg));
                            marks.push((Bounds::new(point(bounds.left() + px(gutter_w - 6.0), r.y), size(px(3.), r.h)), theme.add));
                        }
                        if !in_review {
                            for h in &head_hunks {
                                if h.new.contains(l) {
                                    let c = if h.old.is_empty() { theme.add } else { theme.modified };
                                    marks.push((Bounds::new(point(bounds.left() + px(gutter_w - 6.0), r.y), size(px(3.), r.h)), c));
                                } else if h.new.is_empty() && h.new.start == *l {
                                    marks.push((Bounds::new(point(bounds.left() + px(gutter_w - 8.0), r.y - px(1.)), size(px(7.), px(2.))), theme.del));
                                }
                            }
                        }
                        if let Some(sev) = line_sev.get(l) {
                            if mode.gutter { dots.push((Bounds::new(point(bounds.left() + px(9.), r.y + px((mode.line_height - 6.0) / 2.0)), size(px(6.), px(6.))), diag_color(&theme, *sev))); }
                        }
                        // 折叠：标题行末尾的「⋯」标签，行号栏的箭头
                        if mode.gutter && ed.review.is_none() {
                            let end = ed.fold_end_of(*l);
                            let foldable = end.is_some() || (gutter_hover && ed.foldable_end(*l).is_some());
                            if foldable {
                                let g = if end.is_some() { "›" } else { "⌄" };
                                let sh = window.text_system().shape_line(g.into(), px(13.), &[TextRun { len: g.len(), font: font(UI_FONT), color: if end.is_some() { theme.accent } else { theme.dim }, background_color: None, underline: None, strikethrough: None }], None);
                                chevrons.push((r.y, sh));
                            }
                            if let Some(e) = end {
                                let closer: String = ed.buffer.line_text(e).trim().chars().take(8).collect();
                                let label = if closer.is_empty() { "⋯".to_string() } else { format!("⋯ {closer}") };
                                let sh = window.text_system().shape_line(label.clone().into(), px(11.5), &[TextRun { len: label.len(), font: font(UI_FONT), color: theme.dim, background_color: None, underline: None, strikethrough: None }], None);
                                let tw = r.shaped.as_ref().map(|s| s.size(lh).width).unwrap_or(px(0.));
                                let w = sh.width + px(14.);
                                chips.push((Bounds::new(point(text_origin_x + tw + px(8.), r.y + px(2.)), size(w, px(mode.line_height - 4.))), sh));
                            }
                        }
                        if mode.gutter {
                            let s = (l + 1).to_string();
                            let color = if is_cur { theme.dim } else { theme.faint };
                            let shaped = window.text_system().shape_line(s.clone().into(), px(mode.font_size - 1.0), &[TextRun { len: s.len(), font: num_font.clone(), color, background_color: None, underline: None, strikethrough: None }], None);
                            numbers.push((r.y, shaped, is_cur));
                        }
                        // 选区跨过行尾时，在行尾补一小块表示换行被选中
                        if !sel.is_empty() && sel.start <= r.line_start + r.len && sel.end > r.line_start + r.len && *l + 1 < ed.buffer.line_count() {
                            let x = r.shaped.as_ref().map(|s| s.size(lh).width).unwrap_or(px(0.));
                            eol_marks.push(Bounds::new(point(text_origin_x + x, r.y), size(px(7.), r.h.min(lh))));
                        }
                    }
                    RowKind::Removed { .. } => {
                        row_bgs.push((full, theme.del_bg));
                        marks.push((Bounds::new(point(bounds.left() + px(gutter_w - 6.0), r.y), size(px(3.), r.h)), theme.del));
                    }
                }
            }

            cks.push(("行背景与行号", t_ck.elapsed().as_micros()));
            // ---- 审阅按钮 ----
            let mut buttons: Vec<(Bounds<Pixels>, ShapedLine, bool)> = vec![];
            let mut hunk_buttons: Vec<HunkButton> = vec![];
            if in_review {
                let mut seen = std::collections::HashSet::new();
                for r in &rows {
                    let hunk = match &r.kind {
                        RowKind::Removed { hunk, .. } => Some(*hunk),
                        RowKind::Line(l) => hunks_new.iter().position(|h| h.contains(l) || (h.is_empty() && h.start == *l)),
                    };
                    if let Some(hk) = hunk {
                        if seen.insert(hk) {
                            let mut x = bounds.right() - px(14.);
                            for (label, accept) in [("拒绝", false), ("接受", true)] {
                                let sh = window.text_system().shape_line(label.into(), px(11.5), &[TextRun { len: label.len(), font: font(UI_FONT), color: if accept { theme.accent_ink } else { theme.text }, background_color: None, underline: None, strikethrough: None }], None);
                                let w = sh.width + px(18.);
                                x -= w;
                                let rect = Bounds::new(point(x, r.y + px(1.)), size(w, px(mode.line_height - 3.)));
                                buttons.push((rect, sh, accept));
                                hunk_buttons.push(HunkButton { rect, hunk: hk, accept });
                                x -= px(5.);
                            }
                        }
                    }
                }
            }

            // ---- 光标：不做位置滑动（VS Code / IDEA / Zed 默认都没有，打字时会拖尾），坐标吸附到物理像素 ----
            let scale = window.scale_factor();
            let (cw, ch, cdy) = crate::caret::geometry(mode.line_height, mode.font_size, scale);
            let caret_box = |x: Pixels, y: Pixels| Bounds::new(point(px(crate::caret::snap(x.f(), scale)), px(crate::caret::snap(y.f() + cdy, scale))), size(px(cw), px(ch)));
            let mut cursor_box = None;
            if let Some((x, y)) = ed.layout_position(&rows, text_left, scroll_x, cursor, lh) {
                ed.cursor_anim = Some(point(x, y));
                cursor_box = Some(caret_box(x, y));
            }
            // 闪烁（VS Code smooth）：只有淡出阶段连续请求下一帧，亮住 / 暗住时画面不变
            let ph = crate::caret::phase(ed.blink_at.elapsed());
            let caret_a = if focused { crate::caret::alpha_for(ph, ed.smooth_caret) } else { 0.0 };
            if focused && crate::caret::wants_frames(ph, ed.smooth_caret) { window.request_animation_frame(); }
            let extra_cursors: Vec<Bounds<Pixels>> = if focused && caret_a > 0.01 {
                ed.extra.iter().filter_map(|r| ed.layout_position(&rows, text_left, scroll_x, r.end, lh).map(|(x, y)| caret_box(x, y))).collect()
            } else { vec![] };
            let show_cursor = focused && ed.sel.is_empty() || (focused && ed.marked.is_some());
            let cursor_out = if show_cursor && caret_a > 0.01 { cursor_box } else { None };

            // ---- 滚动条 ----
            let thumb = if total_h > view_h.f() + 1.0 && !mode.single_line && mode.gutter {
                let ratio = view_h.f() / total_h;
                let th = (view_h.f() * ratio).max(28.0);
                let ty = (ed.scroll_y / max_scroll.max(1.0)) * (view_h.f() - th);
                Some(Bounds::new(point(bounds.right() - px(8.), top + px(ty)), size(px(5.), px(th))))
            } else { None };

            // ---- 占位提示 ----
            let placeholder = if ed.buffer.is_empty() && !mode.placeholder.is_empty() {
                Some(window.text_system().shape_line(mode.placeholder.clone(), fsize, &[TextRun { len: mode.placeholder.len(), font: base_font.clone(), color: theme.faint, background_color: None, underline: None, strikethrough: None }], None))
            } else { None };

            cks.push(("光标与其余", t_ck.elapsed().as_micros()));
            if t_ck.elapsed().as_micros() > 9000 && std::env::var("QUILL_PERF").is_ok() { eprintln!("[perf] 分段(累计µs) {:?} runs={}µs shape={}µs", cks, PERF_RUNS.swap(0, std::sync::atomic::Ordering::Relaxed), PERF_SHAPE.swap(0, std::sync::atomic::Ordering::Relaxed)); } else { PERF_RUNS.store(0, std::sync::atomic::Ordering::Relaxed); PERF_SHAPE.store(0, std::sync::atomic::Ordering::Relaxed); }
            if (ed.layout.total_h - total_h).abs() > 0.5 && mode.max_lines.is_some() { cx.notify(); }
            ed.layout = LastLayout { bounds, text_left: text_origin_x, rows, hunk_buttons, total_h, gutter_w };
            Prepaint { hitbox, gutter_w, numbers, cursor: cursor_out, marks, row_bgs, thumb, buttons, placeholder, focused, eol_marks, dots, chips, chevrons, extra_cursors, caret_alpha: caret_a }
        });
        perf_record(0, t0);
        out
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pre: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let t_paint = PerfT::now();
        let theme = cx.theme().clone();
        let editor = self.editor.clone();
        let (lh, mode_gutter, single, padding) = { let e = editor.read(cx); (e.mode.line_height, e.mode.gutter, e.mode.single_line, e.mode.padding) };
        let lhp = px(lh);
        window.set_cursor_style(CursorStyle::IBeam, &pre.hitbox);
        let focus = editor.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, editor.clone()), cx);

        let (rows, text_left, font_size) = editor.update(cx, |e, _| (std::mem::take(&mut e.layout.rows), e.layout.text_left, e.mode.font_size));
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if mode_gutter { window.paint_quad(fill(bounds, theme.bg)); }
            for (b, c) in &pre.row_bgs { window.paint_quad(fill(*b, *c)); }
            let (mut t_bg, mut t_tx) = (0u128, 0u128);
            for r in &rows {
                let origin = point(text_left, r.y);
                match (&r.kind, &r.shaped) {
                    (_, Some(s)) => {
                        let a = std::time::Instant::now();
                        let _ = s.paint_background(origin, lhp, TextAlign::Left, None, window, cx);
                        t_bg += a.elapsed().as_micros();
                        let b = std::time::Instant::now();
                        let _ = s.paint(origin, lhp, TextAlign::Left, None, window, cx);
                        t_tx += b.elapsed().as_micros();
                    }
                    _ => {}
                }
            }
            if AFTER_OPEN.load(std::sync::atomic::Ordering::Relaxed) > 0 { eprintln!("[perf] paint 内部：行背景 {}µs  文字 {}µs  共 {} 行", t_bg, t_tx, rows.len()); }
            for b in &pre.eol_marks { window.paint_quad(fill(*b, theme.selection)); }
            for (b, sh) in &pre.chips {
                window.paint_quad(quad(*b, px(5.), theme.press, px(1.), theme.border, BorderStyle::default()));
                let _ = sh.paint(point(b.left() + px(7.), b.top() + px((b.size.height.f() - 14.0) / 2.0)), px(14.), window, cx);
            }
            if let Some(p) = &pre.placeholder {
                let _ = p.paint(point(text_left, bounds.top() + px(padding) + px((lh - font_size * 1.25) / 2.0)), lhp, window, cx);
            }
            // 光标：和 VS Code / IDEA 一样用接近文字的中性色（不是强调色）、笔直的 2px 竖线、一整行高，没有圆角和光晕
            let paint_caret = |window: &mut Window, b: Bounds<Pixels>| {
                let mut c = theme.text;
                c.a *= if theme.dark { 0.88 } else { 0.94 } * pre.caret_alpha;
                window.paint_quad(fill(b, c));
            };
            if let Some(c) = pre.cursor { paint_caret(window, c); }
            for c in &pre.extra_cursors { paint_caret(window, *c); }
            // 行号栏盖在滚动后的文本之上
            if mode_gutter {
                window.paint_quad(fill(Bounds::new(bounds.origin, size(px(pre.gutter_w), bounds.size.height)), theme.bg));
                for (y, s, _) in &pre.numbers {
                    let x = bounds.left() + px(pre.gutter_w) - px(24.) - s.width;
                    let _ = s.paint(point(x, *y + px(0.5)), lhp, window, cx);
                }
                for (b, c) in &pre.marks { window.paint_quad(fill(*b, *c)); }
                for (b, c) in &pre.dots { window.paint_quad(quad(*b, px(3.), *c, px(0.), *c, BorderStyle::default())); }
                for (y, sh) in &pre.chevrons { let _ = sh.paint(point(bounds.left() + px(pre.gutter_w) - px(19.), *y + px(1.)), lhp, window, cx); }
            } else {
                for (b, c) in &pre.marks { window.paint_quad(fill(*b, *c)); }
            }
            for (rect, sh, accept) in &pre.buttons {
                window.paint_quad(quad(*rect, px(6.), if *accept { theme.accent } else { theme.elev }, px(1.), if *accept { theme.accent } else { theme.border_strong }, BorderStyle::default()));
                let _ = sh.paint(point(rect.left() + px(9.), rect.top() + px((rect.size.height.f() - 14.0) / 2.0)), px(14.), window, cx);
            }
            if let Some(t) = pre.thumb { window.paint_quad(quad(t, px(3.), theme.border_strong, px(0.), theme.border_strong, BorderStyle::default())); }
        });
        editor.update(cx, |e, _| e.layout.rows = rows);
        let _ = (pre.focused, single);
        perf_record(1, t_paint);

        // ---- 鼠标 ----
        let hitbox_id = pre.hitbox.id;
        {
            let editor = editor.clone();
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left || !bounds.contains(&ev.position) { return; }
                let _ = hitbox_id;
                editor.update(cx, |ed, cx| {
                    window.focus(&ed.focus);
                    ed.mouse_down(ev.position, ev.click_count, ev.modifiers.shift, cx);
                });
            });
        }
        {
            let editor = editor.clone();
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture && phase != DispatchPhase::Bubble { return; }
                editor.update(cx, |ed, cx| {
                    let over = ed.mode.gutter && ed.layout.bounds.contains(&ev.position) && ev.position.x < ed.layout.bounds.left() + px(ed.layout.gutter_w);
                    if over != ed.gutter_hover { ed.gutter_hover = over; cx.notify(); }
                    if !ed.dragging || ev.pressed_button != Some(MouseButton::Left) { return; }
                    let lh = ed.mode.line_height;
                    // 拖到边缘外时自动滚动
                    let b = ed.layout.bounds;
                    if ev.position.y < b.top() { ed.scroll_y = (ed.scroll_y - 14.0).max(0.0); }
                    if ev.position.y > b.bottom() { ed.scroll_y += 14.0; }
                    let len = ed.buffer.len();
                    if let Some(o) = ed.layout.offset_at(ev.position, lh, len) { ed.select_to(o, cx); }
                    cx.notify();
                });
            });
        }
        {
            let editor = editor.clone();
            window.on_mouse_event(move |ev: &MouseUpEvent, _phase, _window, cx| {
                if ev.button == MouseButton::Left { editor.update(cx, |ed, _| ed.dragging = false); }
            });
        }
        {
            let editor = editor.clone();
            window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || !bounds.contains(&ev.position) { return; }
                editor.update(cx, |ed, cx| {
                    let d = ev.delta.pixel_delta(px(ed.mode.line_height));
                    if ed.mode.single_line { return; }
                    let max = (ed.layout.total_h - (ed.layout.bounds.size.height.f() - ed.mode.padding * 2.0)).max(0.0);
                    if d.y.f().abs() >= d.x.f().abs() || ed.mode.wrap { ed.scroll_y = (ed.scroll_y - d.y.f()).clamp(0.0, max); }
                    else { ed.scroll_x = (ed.scroll_x - d.x.f()).max(0.0); }
                    cx.notify();
                });
            });
        }
    }
}

impl Editor {
    fn layout_position(&self, rows: &[VisRow], text_left: Pixels, scroll_x: Pixels, offset: usize, lh: Pixels) -> Option<(Pixels, Pixels)> {
        let _ = scroll_x;
        for r in rows {
            if let RowKind::Line(_) = r.kind {
                if offset >= r.line_start && offset <= r.line_start + r.len {
                    let p = match &r.shaped {
                        Some(s) => s.position_for_index(to_display(&r.tabs, offset - r.line_start), lh).unwrap_or(point(px(0.), px(0.))),
                        None => point(px(0.), px(0.)),
                    };
                    return Some((text_left + p.x, r.y + p.y));
                }
            }
        }
        None
    }
}

/// 把一行排版成可绘制的文本（含语法高亮、选区背景、查找高亮、输入法下划线）
#[allow(clippy::too_many_arguments)]
fn shape_row(ed: &Editor, theme: &crate::theme::Theme, base_font: &Font, fsize: Pixels, wrap_w: Option<Pixels>, kind: &RowKind, _placeholder: &SharedString, window: &mut Window) -> (Option<WrappedLine>, Vec<usize>) {
    match kind {
        RowKind::Removed { text, .. } => {
            if text.is_empty() { return (None, vec![]); }
            let (disp, tabs) = expand_tabs(text);
            let run = TextRun { len: disp.len(), font: base_font.clone(), color: theme.dim, background_color: None, underline: None, strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: Some(theme.del.opacity(0.5)) }) };
            (window.text_system().shape_text(disp.into(), fsize, &[run], wrap_w, None).ok().and_then(|mut v| v.pop()), tabs)
        }
        RowKind::Line(l) => {
            let ls = ed.buffer.line_start(*l);
            let le = ed.buffer.line_end(*l);
            if le == ls { return (None, vec![]); }
            let text = ed.buffer.line_text(*l);
            let (disp, tabs) = expand_tabs(&text);
            let t_r = std::time::Instant::now();
            let runs = line_runs(ed, theme, base_font, ls, le, &tabs, &text);
            PERF_RUNS.fetch_add(t_r.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
            let t_s = std::time::Instant::now();
            let r = window.text_system().shape_text(disp.into(), fsize, &runs, wrap_w, None).ok().and_then(|mut v| v.pop());
            PERF_SHAPE.fetch_add(t_s.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
            (r, tabs)
        }
    }
}

pub fn diag_color(t: &crate::theme::Theme, sev: u8) -> Hsla {
    match sev { 1 => t.del, 2 => t.modified, 3 => t.syntax(crate::syntax::Kind::Function), _ => t.dim }
}

pub static PERF_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PERF_SHAPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn expand_tabs(s: &str) -> (String, Vec<usize>) {
    if !s.contains('\t') { return (s.to_string(), vec![]); }
    let tabs: Vec<usize> = s.match_indices('\t').map(|(i, _)| i).collect();
    (s.replace('\t', "    "), tabs)
}

fn line_runs(ed: &Editor, theme: &crate::theme::Theme, font: &Font, ls: usize, le: usize, tabs: &[usize], text: &str) -> Vec<TextRun> {
    let mut cuts: Vec<usize> = vec![ls, le];
    let spans = &ed.spans_ref()[..];
    let first = spans.partition_point(|(r, _)| r.end <= ls);
    for (r, _) in spans[first..].iter().take_while(|(r, _)| r.start < le) { cuts.push(r.start.clamp(ls, le)); cuts.push(r.end.clamp(ls, le)); }
    let sel = &ed.sel;
    if sel.start < le && sel.end > ls { cuts.push(sel.start.clamp(ls, le)); cuts.push(sel.end.clamp(ls, le)); }
    for x in &ed.extra { if x.start < le && x.end > ls { cuts.push(x.start.clamp(ls, le)); cuts.push(x.end.clamp(ls, le)); } }
    let matches = &ed.search_matches;
    if !matches.is_empty() {
        let f = matches.partition_point(|r| r.end <= ls);
        for r in matches[f..].iter().take_while(|r| r.start < le) { cuts.push(r.start.clamp(ls, le)); cuts.push(r.end.clamp(ls, le)); }
    }
    if let Some(m) = &ed.marked { if m.start < le && m.end > ls { cuts.push(m.start.clamp(ls, le)); cuts.push(m.end.clamp(ls, le)); } }
    for d in &ed.diagnostics { if d.range.start < le && d.range.end > ls { cuts.push(d.range.start.clamp(ls, le)); cuts.push(d.range.end.clamp(ls, le)); } }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::with_capacity(cuts.len());
    let mut si = first;
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        while si < spans.len() && spans[si].0.end <= a { si += 1; }
        let color = if si < spans.len() && spans[si].0.start <= a { theme.syntax(spans[si].1) } else { theme.text };
        let in_sel = (!sel.is_empty() && a >= sel.start && b <= sel.end) || ed.extra.iter().any(|x| !x.is_empty() && a >= x.start && b <= x.end);
        let cur_match = ed.search_matches.get(ed.search_current);
        let bg = if in_sel { Some(theme.selection) }
            else if ed.search_matches.iter().any(|r| r.start <= a && b <= r.end) {
                Some(if cur_match.map(|r: &Range<usize>| r.start <= a && b <= r.end).unwrap_or(false) { theme.accent_line } else { theme.accent_soft })
            } else { None };
        let underline = ed.marked.as_ref().filter(|m| a >= m.start && b <= m.end).map(|_| UnderlineStyle { color: Some(theme.dim), thickness: px(1.), wavy: false })
            .or_else(|| ed.diagnostics.iter().filter(|d| d.range.start <= a && b <= d.range.end).min_by_key(|d| d.severity).map(|d| UnderlineStyle { color: Some(diag_color(theme, d.severity)), thickness: px(1.3), wavy: true }));
        // 排版文本里 tab 被展开成 4 个空格，长度要按展开后计算
        let len_disp = to_display(tabs, b - ls) - to_display(tabs, a - ls);
        let _ = text;
        runs.push(TextRun { len: len_disp, font: font.clone(), color, background_color: bg, underline, strikethrough: None });
    }
    runs
}

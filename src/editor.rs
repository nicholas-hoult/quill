//! 编辑器实体：文本、选区、撤销、语法高亮、审阅（diff）、输入法。绘制在 editor_element.rs。
//! 同一套代码既当代码编辑器，也当单行输入框 / 聊天输入框（通过 EditorMode 切换），
//! 这样输入法、剪贴板、撤销、选区在整个应用里行为完全一致。
use crate::actions::*;
use crate::buffer::Buffer;
use crate::diff::{self, Hunk};
use crate::syntax::{self, Kind, Lang};
use gpui::{prelude::*, *};
use std::ops::Range;
use std::time::{Duration, Instant};

pub enum EditorEvent {
    Changed,
    Submit,
    Cancel,
    /// 请求打开行内改写
    InlineAssist,
    ReviewEnded,
}

#[derive(Clone)]
pub struct EditorMode {
    pub gutter: bool,
    pub single_line: bool,
    pub wrap: bool,
    pub ui_font: bool,
    pub font_size: f32,
    pub line_height: f32,
    pub placeholder: SharedString,
    /// 内容自适应高度，最多显示的行数
    pub max_lines: Option<usize>,
    pub padding: f32,
    pub highlight_active_line: bool,
    pub enter_submits: bool,
}

impl EditorMode {
    pub fn code() -> Self {
        Self { gutter: true, single_line: false, wrap: false, ui_font: false, font_size: 13.0, line_height: 21.0, placeholder: "".into(), max_lines: None, padding: 8.0, highlight_active_line: true, enter_submits: false }
    }
    pub fn input(placeholder: &str) -> Self {
        Self { gutter: false, single_line: true, wrap: false, ui_font: true, font_size: 14.0, line_height: 22.0, placeholder: placeholder.to_string().into(), max_lines: Some(1), padding: 0.0, highlight_active_line: false, enter_submits: true }
    }
    pub fn composer(placeholder: &str) -> Self {
        Self { gutter: false, single_line: false, wrap: true, ui_font: true, font_size: 13.5, line_height: 21.0, placeholder: placeholder.to_string().into(), max_lines: Some(8), padding: 0.0, highlight_active_line: false, enter_submits: true }
    }
}

#[derive(Clone, Debug)]
pub enum RowKind {
    Line(usize),
    Removed { hunk: usize, text: String },
}

pub struct Review {
    pub original: String,
    pub hunks: Vec<Hunk>,
    pub rows: Vec<RowKind>,
    pub line_row: Vec<usize>,
    pub version: u64,
}

pub struct Editor {
    pub focus: FocusHandle,
    pub mode: EditorMode,
    pub path: Option<String>,
    pub buffer: Buffer,
    saved_text: String,
    pub sel: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    goal_col: Option<usize>,
    pub lang: Option<Lang>,
    pub(crate) spans: Vec<(Range<usize>, Kind)>,
    spans_version: u64,
    pub scroll_y: f32,
    pub scroll_x: f32,
    pub reveal: bool,
    pub review: Option<Review>,
    pub head: Option<String>,
    head_hunks: (u64, Vec<Hunk>),
    pub blink_on: bool,
    blink_epoch: u64,
    pub dragging: bool,
    pub layout: crate::editor_element::LastLayout,
    pub search: Option<String>,
    pub(crate) search_matches: Vec<Range<usize>>,
    search_version: u64,
    pub search_current: usize,
    clock: Instant,
    pub cursor_anim: Option<Point<Pixels>>,
    pub read_only: bool,
    /// 外部（如 @ 提及弹窗）接管 上/下/回车/Tab 时置位，编辑器把这些键交还给父级
    pub intercept: bool,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

fn indent_unit(lang: Option<Lang>) -> &'static str {
    match lang {
        Some(Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Json | Lang::Css | Lang::Bash) => "  ",
        _ => "    ",
    }
}

fn comment_prefix(lang: Option<Lang>) -> Option<&'static str> {
    match lang {
        Some(Lang::Rust | Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Go) => Some("// "),
        Some(Lang::Python | Lang::Bash) => Some("# "),
        _ => None,
    }
}

impl Editor {
    pub fn new(mode: EditorMode, cx: &mut Context<Self>) -> Self {
        let this = Self {
            focus: cx.focus_handle(),
            mode,
            path: None,
            buffer: Buffer::new(""),
            saved_text: String::new(),
            sel: 0..0,
            reversed: false,
            marked: None,
            goal_col: None,
            lang: None,
            spans: vec![],
            spans_version: u64::MAX,
            scroll_y: 0.0,
            scroll_x: 0.0,
            reveal: false,
            review: None,
            head: None,
            head_hunks: (u64::MAX, vec![]),
            blink_on: true,
            blink_epoch: 0,
            dragging: false,
            layout: Default::default(),
            search: None,
            search_matches: vec![],
            search_version: u64::MAX,
            search_current: 0,
            clock: Instant::now(),
            cursor_anim: None,
            read_only: false,
            intercept: false,
        };
        // 光标闪烁：输入后停 0.5s 再开始闪
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(530)).await;
                if this.update(cx, |e, cx| { e.blink_on = !e.blink_on; cx.notify(); }).is_err() { break; }
            }
        })
        .detach();
        this
    }

    fn now_ms(&self) -> u64 { self.clock.elapsed().as_millis() as u64 }

    // ---------- 文档 ----------

    pub fn open(&mut self, path: Option<String>, text: &str, cx: &mut Context<Self>) {
        self.lang = path.as_deref().and_then(syntax::lang_for_path);
        self.path = path;
        self.buffer = Buffer::new(text);
        self.saved_text = text.to_string();
        self.sel = 0..0;
        self.scroll_y = 0.0;
        self.scroll_x = 0.0;
        self.review = None;
        self.spans_version = u64::MAX;
        cx.notify();
    }

    pub fn text(&self) -> String { self.buffer.text() }

    pub fn is_empty(&self) -> bool { self.buffer.is_empty() }

    pub fn is_dirty(&self) -> bool {
        self.buffer.len() != self.saved_text.len() || self.buffer.text() != self.saved_text
    }

    pub fn mark_saved(&mut self) { self.saved_text = self.buffer.text(); }

    /// 磁盘上的内容和上次保存 / 打开时不一样
    pub fn disk_differs(&self, disk: &str) -> bool { disk != self.saved_text }

    /// 文件在外部被修改且这里没有未保存的改动：直接换成磁盘内容，保留光标和滚动位置
    pub fn reload_from_disk(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if self.is_dirty() || self.review.is_some() { return false; }
        self.buffer = Buffer::new(text);
        self.saved_text = text.to_string();
        let c = self.sel.start.min(text.len());
        let c = (0..=c).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        self.sel = c..c;
        self.spans_version = u64::MAX;
        self.head_hunks.0 = u64::MAX;
        cx.emit(EditorEvent::Changed);
        cx.notify();
        true
    }

    pub fn toggle_wrap(&mut self, _: &ToggleWrap, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode.single_line || self.mode.max_lines.is_some() { return; }
        if !self.mode.wrap && self.buffer.line_count() > 6000 { return; } // 软换行要整体排版，超大文件会卡
        self.mode.wrap = !self.mode.wrap;
        self.scroll_x = 0.0;
        self.touch(cx);
    }

    /// 把当前选中的匹配替换成 `with`，然后跳到下一个
    pub fn replace_current(&mut self, with: &str, cx: &mut Context<Self>) {
        let ms = self.matches().to_vec();
        if ms.iter().any(|m| *m == self.sel) {
            let r = self.sel.clone();
            let p = r.start + with.len();
            self.edit(r, with, p..p, cx);
        }
        self.search_version = u64::MAX;
        self.find_step(true, cx);
    }

    /// 替换全部匹配，作为一次可撤销的修改，返回替换个数
    pub fn replace_all_matches(&mut self, with: &str, cx: &mut Context<Self>) -> usize {
        let ms = self.matches().to_vec();
        if ms.is_empty() { return 0; }
        let text = self.buffer.text();
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for m in &ms { out.push_str(&text[last..m.start]); out.push_str(with); last = m.end; }
        out.push_str(&text[last..]);
        let n = self.buffer.len();
        self.edit(0..n, &out, 0..0, cx);
        self.search_version = u64::MAX;
        ms.len()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.buffer = Buffer::new("");
        self.sel = 0..0;
        self.marked = None;
        self.scroll_y = 0.0;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let n = self.buffer.len();
        let now = self.now_ms();
        self.buffer.replace(0..n, text, 0..0, text.len()..text.len(), now + 10_000);
        self.sel = text.len()..text.len();
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    pub fn set_head(&mut self, head: Option<String>, cx: &mut Context<Self>) {
        self.head = head;
        self.head_hunks.0 = u64::MAX;
        cx.notify();
    }

    pub fn cursor(&self) -> usize { if self.reversed { self.sel.start } else { self.sel.end } }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        let c = self.cursor();
        let line = self.buffer.line_of(c);
        let col = self.buffer.slice(self.buffer.line_start(line)..c).chars().count();
        (line + 1, col + 1)
    }

    pub fn selected_text(&self) -> String { self.buffer.slice(self.sel.clone()) }

    pub(crate) fn spans_ref(&self) -> &Vec<(Range<usize>, Kind)> { &self.spans }

    /// 语法高亮区间（按文本版本缓存）
    pub fn spans(&mut self) -> &[(Range<usize>, Kind)] {
        if self.spans_version != self.buffer.version {
            self.spans = match self.lang {
                Some(l) if self.buffer.len() < 600_000 => syntax::highlight(l, &self.buffer.text()),
                _ => vec![],
            };
            self.spans_version = self.buffer.version;
        }
        &self.spans
    }

    /// 相对 HEAD 的改动块（行号基于当前文本）
    pub fn head_hunks(&mut self) -> &[Hunk] {
        if self.head_hunks.0 != self.buffer.version {
            let hs = match &self.head {
                Some(h) if self.buffer.line_count() < 30_000 => diff::hunks(h, &self.buffer.text()),
                _ => vec![],
            };
            self.head_hunks = (self.buffer.version, hs);
        }
        &self.head_hunks.1
    }

    // ---------- 选区 / 光标 ----------

    fn touch(&mut self, cx: &mut Context<Self>) {
        self.blink_on = true;
        self.blink_epoch += 1;
        self.reveal = true;
        cx.notify();
    }

    pub fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let o = offset.min(self.buffer.len());
        self.sel = o..o;
        self.reversed = false;
        self.touch(cx);
    }

    pub fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let o = offset.min(self.buffer.len());
        let anchor = if self.reversed { self.sel.end } else { self.sel.start };
        if o < anchor { self.sel = o..anchor; self.reversed = true; } else { self.sel = anchor..o; self.reversed = false; }
        self.touch(cx);
    }

    fn go(&mut self, offset: usize, select: bool, keep_goal: bool, cx: &mut Context<Self>) {
        if !keep_goal { self.goal_col = None; }
        if select { self.select_to(offset, cx) } else { self.move_to(offset, cx) }
    }

    fn vertical(&mut self, delta: isize, select: bool, cx: &mut Context<Self>) {
        let c = self.cursor();
        let line = self.buffer.line_of(c);
        let col = *self.goal_col.get_or_insert_with(|| self.buffer.slice(self.buffer.line_start(line)..c).chars().count());
        let target = line as isize + delta;
        if target < 0 { return self.go(0, select, true, cx); }
        if target as usize >= self.buffer.line_count() { return self.go(self.buffer.len(), select, true, cx); }
        let t = target as usize;
        let text = self.buffer.line_text(t);
        let byte = text.char_indices().nth(col).map(|(i, _)| i).unwrap_or(text.len());
        self.go(self.buffer.line_start(t) + byte, select, true, cx);
    }

    pub fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() { let o = self.buffer.prev_boundary(self.cursor()); self.go(o, false, false, cx) } else { self.go(self.sel.start, false, false, cx) }
    }
    pub fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() { let o = self.buffer.next_boundary(self.cursor()); self.go(o, false, false, cx) } else { self.go(self.sel.end, false, false, cx) }
    }
    pub fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.prev_boundary(self.cursor()); self.go(o, true, false, cx) }
    pub fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.next_boundary(self.cursor()); self.go(o, true, false, cx) }
    pub fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) { if self.mode.single_line || self.intercept { cx.propagate() } else { self.vertical(-1, false, cx) } }
    pub fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) { if self.mode.single_line || self.intercept { cx.propagate() } else { self.vertical(1, false, cx) } }
    pub fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) { self.vertical(-1, true, cx) }
    pub fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) { self.vertical(1, true, cx) }
    pub fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.word_left(self.cursor()); self.go(o, false, false, cx) }
    pub fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.word_right(self.cursor()); self.go(o, false, false, cx) }
    pub fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.word_left(self.cursor()); self.go(o, true, false, cx) }
    pub fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.word_right(self.cursor()); self.go(o, true, false, cx) }

    /// 智能行首：先到第一个非空白字符，再按一次到真正行首
    fn smart_home(&self) -> usize {
        let c = self.cursor();
        let line = self.buffer.line_of(c);
        let start = self.buffer.line_start(line);
        let text = self.buffer.line_text(line);
        let first = start + (text.len() - text.trim_start().len());
        if c == first || self.mode.single_line { start } else { first }
    }
    pub fn move_to_line_start(&mut self, _: &MoveToLineStart, _: &mut Window, cx: &mut Context<Self>) { let o = self.smart_home(); self.go(o, false, false, cx) }
    pub fn select_to_line_start(&mut self, _: &SelectToLineStart, _: &mut Window, cx: &mut Context<Self>) { let o = self.smart_home(); self.go(o, true, false, cx) }
    pub fn move_to_line_end(&mut self, _: &MoveToLineEnd, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.line_end(self.buffer.line_of(self.cursor())); self.go(o, false, false, cx) }
    pub fn select_to_line_end(&mut self, _: &SelectToLineEnd, _: &mut Window, cx: &mut Context<Self>) { let o = self.buffer.line_end(self.buffer.line_of(self.cursor())); self.go(o, true, false, cx) }
    pub fn move_to_doc_start(&mut self, _: &MoveToDocStart, _: &mut Window, cx: &mut Context<Self>) { self.go(0, false, false, cx) }
    pub fn move_to_doc_end(&mut self, _: &MoveToDocEnd, _: &mut Window, cx: &mut Context<Self>) { self.go(self.buffer.len(), false, false, cx) }
    pub fn select_to_doc_start(&mut self, _: &SelectToDocStart, _: &mut Window, cx: &mut Context<Self>) { self.go(0, true, false, cx) }
    pub fn select_to_doc_end(&mut self, _: &SelectToDocEnd, _: &mut Window, cx: &mut Context<Self>) { self.go(self.buffer.len(), true, false, cx) }
    pub fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) { let n = self.page_lines(); self.vertical(-(n as isize), false, cx) }
    pub fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) { let n = self.page_lines(); self.vertical(n as isize, false, cx) }
    fn page_lines(&self) -> usize { ((f32::from(self.layout.bounds.size.height) / self.mode.line_height) as usize).saturating_sub(2).max(1) }
    pub fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) { self.sel = 0..self.buffer.len(); self.reversed = false; self.touch(cx) }

    // ---------- 编辑 ----------

    /// 所有文本修改的唯一入口：更新缓冲区、选区、通知
    fn edit(&mut self, range: Range<usize>, text: &str, new_sel: Range<usize>, cx: &mut Context<Self>) {
        if self.read_only { return; }
        let now = self.now_ms();
        self.buffer.replace(range, text, self.sel.clone(), new_sel.clone(), now);
        self.sel = new_sel;
        self.reversed = false;
        self.marked = None;
        self.after_change(cx);
    }

    fn after_change(&mut self, cx: &mut Context<Self>) {
        self.touch(cx);
        cx.emit(EditorEvent::Changed);
    }

    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let r = self.sel.clone();
        let p = r.start + text.len();
        self.edit(r, text, p..p, cx);
    }

    pub fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let c = self.cursor();
            if c == 0 { return; }
            // 在一对括号 / 引号之间退格时一起删除
            let prev = self.buffer.prev_boundary(c);
            let (a, b) = (self.buffer.slice(prev..c), self.buffer.slice(c..self.buffer.next_boundary(c)));
            let pair = matches!((a.as_str(), b.as_str()), ("(", ")") | ("[", "]") | ("{", "}") | ("\"", "\"") | ("'", "'") | ("`", "`"));
            let end = if pair { c + b.len() } else { c };
            // 行首空白按缩进单位回退
            let line = self.buffer.line_of(c);
            let ls = self.buffer.line_start(line);
            let before = self.buffer.slice(ls..c);
            let start = if !before.is_empty() && before.chars().all(|ch| ch == ' ') && !self.mode.single_line {
                let u = indent_unit(self.lang).len();
                let drop = if before.len() % u == 0 { u } else { before.len() % u };
                c - drop
            } else { prev };
            self.edit(start..end, "", start..start, cx);
        } else {
            let s = self.sel.start;
            let r = self.sel.clone();
            self.edit(r, "", s..s, cx);
        }
    }

    pub fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            let c = self.cursor();
            let n = self.buffer.next_boundary(c);
            if n == c { return; }
            self.edit(c..n, "", c..c, cx);
        } else {
            let s = self.sel.start;
            let r = self.sel.clone();
            self.edit(r, "", s..s, cx);
        }
    }

    pub fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let s = if self.sel.is_empty() { self.buffer.word_left(c) } else { self.sel.start };
        let e = if self.sel.is_empty() { c } else { self.sel.end };
        self.edit(s..e, "", s..s, cx);
    }

    pub fn delete_to_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let s = self.buffer.line_start(self.buffer.line_of(c));
        let s = if s == c && c > 0 { c - 1 } else { s };
        self.edit(s..c, "", s..s, cx);
    }

    pub fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let e = self.buffer.line_end(self.buffer.line_of(c));
        let e = if e == c { self.buffer.next_boundary(c) } else { e };
        self.edit(c..e, "", c..c, cx);
    }

    pub fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.intercept { cx.propagate(); return; }
        if self.mode.enter_submits {
            cx.emit(EditorEvent::Submit);
            return;
        }
        self.newline(cx);
    }

    pub fn newline_action(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) { self.newline(cx); }

    fn newline(&mut self, cx: &mut Context<Self>) {
        if self.mode.single_line { return; }
        let c = self.sel.start;
        let line = self.buffer.line_of(c);
        let ls = self.buffer.line_start(line);
        let before = self.buffer.slice(ls..c);
        let indent: String = before.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        let unit = indent_unit(self.lang);
        let trimmed = before.trim_end();
        let opens = trimmed.ends_with('{') || trimmed.ends_with('(') || trimmed.ends_with('[') || (self.lang == Some(Lang::Python) && trimmed.ends_with(':'));
        let after_char = self.buffer.slice(self.sel.end..self.buffer.next_boundary(self.sel.end));
        let closes = opens && matches!((trimmed.chars().last(), after_char.as_str()), (Some('{'), "}") | (Some('('), ")") | (Some('['), "]"));
        let r = self.sel.clone();
        if closes {
            let inner = format!("\n{indent}{unit}");
            let text = format!("{inner}\n{indent}");
            let p = r.start + inner.len();
            self.edit(r, &text, p..p, cx);
        } else {
            let text = format!("\n{indent}{}", if opens { unit } else { "" });
            let p = r.start + text.len();
            self.edit(r, &text, p..p, cx);
        }
    }

    pub fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        if self.intercept { cx.propagate(); return; }
        if self.mode.single_line { return; }
        let unit = indent_unit(self.lang);
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        if l0 == l1 && !self.mode.wrap {
            let col = self.buffer.slice(self.buffer.line_start(l0)..self.sel.start).chars().count();
            let n = unit.len() - col % unit.len();
            self.insert(&" ".repeat(n), cx);
        } else if l0 == l1 {
            self.insert(unit, cx);
        } else {
            self.indent_lines(l0, l1, true, cx);
        }
    }

    pub fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        self.indent_lines(l0, l1, false, cx);
    }

    fn indent_lines(&mut self, l0: usize, l1: usize, add: bool, cx: &mut Context<Self>) {
        let unit = indent_unit(self.lang);
        let (s, e) = (self.buffer.line_start(l0), self.buffer.line_end(l1));
        let block = self.buffer.slice(s..e);
        let new: Vec<String> = block.split('\n').map(|l| {
            if add { if l.is_empty() { l.to_string() } else { format!("{unit}{l}") } }
            else if let Some(r) = l.strip_prefix(unit) { r.to_string() }
            else if let Some(r) = l.strip_prefix('\t') { r.to_string() }
            else { l.trim_start_matches(' ').to_string() }
        }).collect();
        let text = new.join("\n");
        let sel = s..s + text.len();
        self.edit(s..e, &text, sel, cx);
    }

    pub fn toggle_comment(&mut self, _: &ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        let Some(prefix) = comment_prefix(self.lang) else { return };
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        let (s, e) = (self.buffer.line_start(l0), self.buffer.line_end(l1));
        let block = self.buffer.slice(s..e);
        let lines: Vec<&str> = block.split('\n').collect();
        let marker = prefix.trim_end();
        let all = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with(marker));
        let min_indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        let out: Vec<String> = lines.iter().map(|l| {
            if l.trim().is_empty() { return l.to_string(); }
            if all {
                let ind = l.len() - l.trim_start().len();
                let rest = &l[ind..];
                let rest = rest.strip_prefix(prefix).or_else(|| rest.strip_prefix(marker)).unwrap_or(rest);
                format!("{}{}", &l[..ind], rest)
            } else { format!("{}{}{}", &l[..min_indent], prefix, &l[min_indent..]) }
        }).collect();
        let text = out.join("\n");
        let sel = s..s + text.len();
        self.edit(s..e, &text, sel, cx);
    }

    pub fn duplicate_line(&mut self, _: &DuplicateLine, _: &mut Window, cx: &mut Context<Self>) {
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        let (s, e) = (self.buffer.line_start(l0), self.buffer.line_end(l1));
        let block = self.buffer.slice(s..e);
        let ins = format!("\n{block}");
        let d = ins.len();
        let sel = self.sel.start + d..self.sel.end + d;
        self.edit(e..e, &ins, sel, cx);
    }

    pub fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        let s = self.buffer.line_start(l0);
        let e = if l1 + 1 < self.buffer.line_count() { self.buffer.line_start(l1 + 1) } else { self.buffer.len() };
        let s2 = if e == self.buffer.len() && s > 0 { s - 1 } else { s };
        self.edit(s2..e, "", s2..s2, cx);
    }

    fn move_lines(&mut self, up: bool, cx: &mut Context<Self>) {
        let (l0, l1) = (self.buffer.line_of(self.sel.start), self.buffer.line_of(self.sel.end));
        if (up && l0 == 0) || (!up && l1 + 1 >= self.buffer.line_count()) { return; }
        let (s, e) = (self.buffer.line_start(l0), self.buffer.line_end(l1));
        let block = self.buffer.slice(s..e);
        if up {
            let (ps, pe) = (self.buffer.line_start(l0 - 1), self.buffer.line_end(l0 - 1));
            let prev = self.buffer.slice(ps..pe);
            let text = format!("{block}\n{prev}");
            let shift = prev.len() + 1;
            let sel = self.sel.start - shift..self.sel.end - shift;
            self.edit(ps..e, &text, sel, cx);
        } else {
            let (ns, ne) = (self.buffer.line_start(l1 + 1), self.buffer.line_end(l1 + 1));
            let next = self.buffer.slice(ns..ne);
            let text = format!("{next}\n{block}");
            let shift = next.len() + 1;
            let sel = self.sel.start + shift..self.sel.end + shift;
            self.edit(s..ne, &text, sel, cx);
        }
    }
    pub fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) { self.move_lines(true, cx) }
    pub fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) { self.move_lines(false, cx) }

    pub fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self.buffer.undo() { self.sel = sel.clone(); self.reversed = false; self.marked = None; self.after_change(cx); }
    }
    pub fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(sel) = self.buffer.redo() { self.sel = sel.clone(); self.reversed = false; self.marked = None; self.after_change(cx); }
    }

    pub fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
        } else if !self.mode.single_line {
            // 无选区时复制整行（对标 Zed / VS Code）
            let l = self.buffer.line_of(self.cursor());
            cx.write_to_clipboard(ClipboardItem::new_string(format!("{}\n", self.buffer.line_text(l))));
        }
    }
    pub fn cut(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() && !self.mode.single_line {
            let l = self.buffer.line_of(self.cursor());
            cx.write_to_clipboard(ClipboardItem::new_string(format!("{}\n", self.buffer.line_text(l))));
            self.delete_line(&DeleteLine, w, cx);
        } else if !self.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text()));
            let s = self.sel.start;
            let r = self.sel.clone();
            self.edit(r, "", s..s, cx);
        }
    }
    pub fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) {
            let t = if self.mode.single_line { t.replace(['\n', '\r'], " ") } else { t.replace("\r\n", "\n") };
            self.insert(&t, cx);
        }
    }

    pub fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() { self.search = None; self.search_matches.clear(); cx.notify(); return; }
        cx.emit(EditorEvent::Cancel);
    }

    // ---------- 查找 ----------

    pub fn set_search(&mut self, q: Option<String>, cx: &mut Context<Self>) {
        self.search = q.filter(|s| !s.is_empty());
        self.search_version = u64::MAX;
        self.search_current = 0;
        cx.notify();
    }

    pub fn matches(&mut self) -> &[Range<usize>] {
        if self.search_version != self.buffer.version {
            self.search_matches.clear();
            if let Some(q) = &self.search {
                let text = self.buffer.text().to_lowercase();
                let q = q.to_lowercase();
                let mut from = 0;
                // lowercase 可能改变字节长度，仅在长度一致时才有可靠偏移
                if text.len() == self.buffer.len() {
                    while let Some(i) = text[from..].find(&q) {
                        self.search_matches.push(from + i..from + i + q.len());
                        from += i + q.len().max(1);
                        if self.search_matches.len() > 5000 { break; }
                    }
                }
            }
            self.search_version = self.buffer.version;
        }
        &self.search_matches
    }

    pub fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let c = self.cursor();
        let ms = self.matches().to_vec();
        if ms.is_empty() { return; }
        let idx = if forward { ms.iter().position(|m| m.start >= c.max(self.sel.end)).unwrap_or(0) }
                  else { ms.iter().rposition(|m| m.end <= self.sel.start).unwrap_or(ms.len() - 1) };
        self.search_current = idx;
        self.sel = ms[idx].clone();
        self.reversed = false;
        self.touch(cx);
    }

    pub fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) { self.find_step(true, cx) }
    pub fn find_prev(&mut self, _: &FindPrev, _: &mut Window, cx: &mut Context<Self>) { self.find_step(false, cx) }

    // ---------- 审阅（AI 改动逐块接受 / 拒绝）----------

    pub fn in_review(&self) -> bool { self.review.is_some() }

    /// 进入审阅：把当前内容当作「原文」，之后的修改都以它为基线做 diff。
    pub fn begin_review(&mut self, cx: &mut Context<Self>) {
        if self.review.is_none() {
            self.review = Some(Review { original: self.buffer.text(), hunks: vec![], rows: vec![], line_row: vec![], version: u64::MAX });
        }
        cx.notify();
    }

    /// 重新计算审阅的改动块与显示行（文本变化后调用）
    pub fn refresh_review(&mut self) {
        let Some(r) = &mut self.review else { return };
        if r.version == self.buffer.version { return; }
        let text = self.buffer.text();
        r.hunks = diff::hunks(&r.original, &text);
        let old_lines = diff::split_lines(&r.original);
        let n = self.buffer.line_count();
        let mut rows = Vec::with_capacity(n + 8);
        let mut line_row = Vec::with_capacity(n);
        let mut hi = 0;
        for line in 0..n {
            while hi < r.hunks.len() && r.hunks[hi].new.start == line {
                for l in r.hunks[hi].old.clone() {
                    rows.push(RowKind::Removed { hunk: hi, text: old_lines[l].trim_end_matches(['\n', '\r']).to_string() });
                }
                hi += 1;
            }
            line_row.push(rows.len());
            rows.push(RowKind::Line(line));
        }
        // 文件末尾被删除的行
        while hi < r.hunks.len() {
            for l in r.hunks[hi].old.clone() { rows.push(RowKind::Removed { hunk: hi, text: old_lines[l].trim_end_matches(['\n', '\r']).to_string() }); }
            hi += 1;
        }
        r.rows = rows;
        r.line_row = line_row;
        r.version = self.buffer.version;
    }

    pub fn review_hunk_count(&mut self) -> usize {
        self.refresh_review();
        self.review.as_ref().map(|r| r.hunks.len()).unwrap_or(0)
    }

    /// 以「新文本」整体替换内容并进入审阅（AI 回复里的「应用」）
    pub fn apply_with_review(&mut self, new_text: &str, cx: &mut Context<Self>) {
        self.begin_review(cx);
        let n = self.buffer.len();
        let now = self.now_ms();
        // 模型常常漏掉文件末尾的换行；原文有而新文本没有时补上，避免产生一个假的「改动」
        let mut owned;
        let new_text = if self.buffer.text().ends_with('\n') && !new_text.ends_with('\n') { owned = new_text.to_string(); owned.push('\n'); owned.as_str() } else { new_text };
        self.buffer.replace(0..n, new_text, self.sel.clone(), 0..0, now + 10_000);
        self.sel = 0..0;
        self.refresh_review();
        self.after_change(cx);
        self.scroll_to_first_hunk();
        if self.review.as_ref().map(|r| r.hunks.is_empty()).unwrap_or(false) { self.review = None; cx.emit(EditorEvent::ReviewEnded); }
    }

    fn scroll_to_first_hunk(&mut self) {
        if let Some(r) = &self.review {
            if let Some(h) = r.hunks.first() {
                let row = r.line_row.get(h.new.start).copied().unwrap_or(0).saturating_sub(r.hunks[0].old.len());
                self.scroll_y = (row as f32 * self.mode.line_height - 120.0).max(0.0);
            }
        }
    }

    /// 流式写入：把 [start, start+len) 替换成 text，返回新的长度（用于下一次替换）
    pub fn replace_range_streaming(&mut self, start: usize, len: usize, text: &str, cx: &mut Context<Self>) -> usize {
        let now = self.now_ms();
        self.buffer.replace(start..start + len, text, self.sel.clone(), start + text.len()..start + text.len(), now + 10_000);
        self.sel = start + text.len()..start + text.len();
        self.refresh_review();
        self.after_change(cx);
        text.len()
    }

    pub fn accept_all(&mut self, cx: &mut Context<Self>) {
        if self.review.take().is_some() { cx.emit(EditorEvent::ReviewEnded); cx.notify(); }
    }

    pub fn reject_all(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.review.take() {
            let now = self.now_ms();
            self.buffer.replace_all(&r.original, now);
            self.sel = 0..0;
            self.after_change(cx);
            cx.emit(EditorEvent::ReviewEnded);
        }
    }

    pub fn accept_hunk(&mut self, k: usize, cx: &mut Context<Self>) {
        self.refresh_review();
        let Some(r) = &mut self.review else { return };
        let Some(h) = r.hunks.get(k).cloned() else { return };
        let (old_lines, new_text) = (diff::split_lines(&r.original), self.buffer.text());
        let new_lines = diff::split_lines(&new_text);
        let mut out = String::new();
        for l in &old_lines[..h.old.start] { out.push_str(l); }
        for l in &new_lines[h.new.clone()] { out.push_str(l); }
        for l in &old_lines[h.old.end..] { out.push_str(l); }
        r.original = out;
        r.version = u64::MAX;
        self.refresh_review();
        self.finish_hunk_action(cx);
    }

    pub fn reject_hunk(&mut self, k: usize, cx: &mut Context<Self>) {
        self.refresh_review();
        let Some(r) = &self.review else { return };
        let Some(h) = r.hunks.get(k).cloned() else { return };
        let old_lines = diff::split_lines(&r.original);
        let restore: String = old_lines[h.old.clone()].concat();
        let (s, e) = (self.buffer.line_start(h.new.start), if h.new.end >= self.buffer.line_count() { self.buffer.len() } else { self.buffer.line_start(h.new.end) });
        // 新文本里最后一行没有换行符时，被替换的区间不含换行
        let now = self.now_ms();
        self.buffer.replace(s..e, &restore, s..e, s..s + restore.len(), now + 10_000);
        self.sel = s..s;
        self.refresh_review();
        self.after_change(cx);
        self.finish_hunk_action(cx);
    }

    fn finish_hunk_action(&mut self, cx: &mut Context<Self>) {
        if self.review.as_ref().map(|r| r.hunks.is_empty()).unwrap_or(false) { self.review = None; cx.emit(EditorEvent::ReviewEnded); }
        cx.notify();
    }

    // ---------- 滚动 ----------

    pub fn scroll_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let l = line.min(self.buffer.line_count().saturating_sub(1));
        let o = self.buffer.line_start(l);
        self.sel = o..o;
        self.reversed = false;
        let h = f32::from(self.layout.bounds.size.height).max(300.0);
        self.scroll_y = (l as f32 * self.mode.line_height - h / 2.0).max(0.0);
        self.touch(cx);
    }

    // ---------- 输入法 ----------

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let (mut u8o, mut u16o) = (0, 0);
        for ch in text.chars() {
            if u16o >= offset { break; }
            u16o += ch.len_utf16();
            u8o += ch.len_utf8();
        }
        u8o
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let text = self.buffer.slice(0..offset);
        text.chars().map(|c| c.len_utf16()).sum()
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> { self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end) }
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> { self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end) }
}

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range_utf16: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = self.range_from_utf16(&range_utf16);
        actual.replace(self.range_to_utf16(&r));
        Some(self.buffer.slice(r))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.sel), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) { self.marked = None; }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only { return; }
        let explicit = range_utf16.is_some();
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.sel.clone());
        // 自动补全括号 / 引号：只处理最常见的情形（无选区或有选区时包裹）
        if !explicit && self.marked.is_none() && text.chars().count() == 1 && !self.mode.single_line && !self.mode.wrap {
            let ch = text.chars().next().unwrap();
            let close = match ch { '(' => Some(')'), '[' => Some(']'), '{' => Some('}'), '"' => Some('"'), '\'' => Some('\''), '`' => Some('`'), _ => None };
            let next = self.buffer.slice(range.end..self.buffer.next_boundary(range.end));
            if !range.is_empty() {
                if let Some(c) = close {
                    let inner = self.buffer.slice(range.clone());
                    let t = format!("{ch}{inner}{c}");
                    let sel = range.start + ch.len_utf8()..range.start + ch.len_utf8() + inner.len();
                    self.edit(range, &t, sel, cx);
                    return;
                }
            } else if matches!(ch, ')' | ']' | '}' | '"' | '\'' | '`') && next == text {
                // 键入已存在的闭合符号：跳过
                let p = range.end + text.len();
                self.move_to(p, cx);
                return;
            } else if let Some(c) = close {
                let prev = self.buffer.slice(self.buffer.prev_boundary(range.start)..range.start);
                let word_before = prev.chars().next().map(|p| p.is_alphanumeric()).unwrap_or(false);
                let quote = matches!(ch, '"' | '\'' | '`');
                let next_ok = next.is_empty() || next.chars().next().map(|n| n.is_whitespace() || ")]};,.".contains(n)).unwrap_or(true);
                if next_ok && !(quote && word_before) {
                    let t = format!("{ch}{c}");
                    let p = range.start + ch.len_utf8();
                    self.edit(range, &t, p..p, cx);
                    return;
                }
            }
            // 键入 } 且光标前只有空白：回退一级缩进
            if ch == '}' {
                let ls = self.buffer.line_start(self.buffer.line_of(range.start));
                let before = self.buffer.slice(ls..range.start);
                let u = indent_unit(self.lang);
                if !before.is_empty() && before.chars().all(|c| c == ' ') && before.len() >= u.len() {
                    let s = range.start - u.len();
                    self.edit(s..range.end, "}", s + 1..s + 1, cx);
                    return;
                }
            }
        }
        let p = range.start + text.len();
        self.edit(range, text, p..p, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, range_utf16: Option<Range<usize>>, text: &str, new_sel_utf16: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only { return; }
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked.clone()).unwrap_or(self.sel.clone());
        let now = self.now_ms();
        let sel_default = range.start + text.len()..range.start + text.len();
        self.buffer.replace(range.clone(), text, self.sel.clone(), sel_default.clone(), now + 10_000);
        self.marked = if text.is_empty() { None } else { Some(range.start..range.start + text.len()) };
        self.sel = new_sel_utf16.as_ref().map(|r| {
            // new_sel 是相对于刚写入文本的 UTF-16 偏移
            let local = &text[..];
            let to8 = |u: usize| { let (mut a, mut b) = (0, 0); for ch in local.chars() { if b >= u { break; } b += ch.len_utf16(); a += ch.len_utf8(); } a };
            range.start + to8(r.start)..range.start + to8(r.end)
        }).unwrap_or(sel_default);
        self.reversed = false;
        self.after_change(cx);
    }

    fn bounds_for_range(&mut self, range_utf16: Range<usize>, element_bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let r = self.range_from_utf16(&range_utf16);
        let (x, y) = self.layout.position_of(r.start, self.mode.line_height)?;
        let _ = element_bounds;
        Some(Bounds::new(point(x, y), size(px(2.0), px(self.mode.line_height))))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let o = self.layout.offset_at(p, self.mode.line_height, self.buffer.len())?;
        Some(self.offset_to_utf16(o))
    }
}

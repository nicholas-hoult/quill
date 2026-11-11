//! 内置终端：alacritty_terminal（Zed 同款内核）负责 PTY 和终端状态机，这里只做按键翻译和绘制。
use crate::theme::{ActiveTheme, MONO_FONT};
use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty::{self, Options as PtyOptions};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
use gpui::{prelude::*, *};
use std::sync::Arc;

#[derive(Clone)]
struct Proxy(async_channel::Sender<TermEvent>);

impl EventListener for Proxy {
    fn send_event(&self, e: TermEvent) { let _ = self.0.try_send(e); }
}

struct Size { cols: usize, lines: usize }

impl Dimensions for Size {
    fn total_lines(&self) -> usize { self.lines }
    fn screen_lines(&self) -> usize { self.lines }
    fn columns(&self) -> usize { self.cols }
}

pub struct TerminalView {
    pub focus: FocusHandle,
    term: Arc<FairMutex<Term<Proxy>>>,
    tx: EventLoopSender,
    cols: usize,
    lines: usize,
    cell_w: f32,
    cell_h: f32,
    pub exited: bool,
}

impl TerminalView {
    pub fn new(cwd: &str, cx: &mut Context<Self>) -> Result<Self, String> {
        let (ev_tx, ev_rx) = async_channel::unbounded();
        let proxy = Proxy(ev_tx);
        let (cols, lines) = (100usize, 14usize);
        let term = Arc::new(FairMutex::new(Term::new(Config::default(), &Size { cols, lines }, proxy.clone())));
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let opts = PtyOptions { shell: Some(tty::Shell::new(shell, vec!["-l".into()])), working_directory: Some(cwd.into()), drain_on_exit: false, env };
        let ws = WindowSize { num_lines: lines as u16, num_cols: cols as u16, cell_width: 8, cell_height: 16 };
        let pty = tty::new(&opts, ws, 0).map_err(|e| e.to_string())?;
        let ev_loop = EventLoop::new(term.clone(), proxy, pty, false, false).map_err(|e| e.to_string())?;
        let tx = ev_loop.channel();
        ev_loop.spawn();
        let tx2 = tx.clone();
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = ev_rx.recv().await {
                match ev {
                    TermEvent::PtyWrite(s) => { let _ = tx2.send(Msg::Input(s.into_bytes().into())); }
                    TermEvent::Exit | TermEvent::ChildExit(_) => { let _ = this.update(cx, |t, cx| { t.exited = true; cx.notify(); }); }
                    TermEvent::Wakeup => { if this.update(cx, |_, cx| cx.notify()).is_err() { break; } }
                    _ => {}
                }
            }
        }).detach();
        Ok(Self { focus: cx.focus_handle(), term, tx, cols, lines, cell_w: 8.0, cell_h: 17.0, exited: false })
    }

    fn write(&self, bytes: Vec<u8>) { let _ = self.tx.send(Msg::Input(bytes.into())); }

    fn resize_to(&mut self, w: f32, h: f32) {
        if self.cell_w < 1.0 || self.cell_h < 1.0 { return; }
        let cols = ((w / self.cell_w) as usize).max(10);
        let lines = ((h / self.cell_h) as usize).max(3);
        if cols != self.cols || lines != self.lines {
            self.cols = cols;
            self.lines = lines;
            self.term.lock().resize(Size { cols, lines });
            let _ = self.tx.send(Msg::Resize(WindowSize { num_lines: lines as u16, num_cols: cols as u16, cell_width: self.cell_w as u16, cell_height: self.cell_h as u16 }));
        }
    }

    pub fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = &k.modifiers;
        if m.platform {
            match k.key.as_str() {
                "v" => { if let Some(t) = cx.read_from_clipboard().and_then(|i| i.text()) { self.write(t.replace("\r\n", "\n").replace('\n', "\r").into_bytes()); } }
                "c" => { if let Some(s) = self.term.lock().selection_to_string() { cx.write_to_clipboard(ClipboardItem::new_string(s)); } }
                "k" => self.write(b"\x0c".to_vec()),
                _ => { cx.propagate(); return; }
            }
            return;
        }
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        let arrow = |c: char| -> Vec<u8> { if app_cursor { format!("\x1bO{c}").into_bytes() } else { format!("\x1b[{c}").into_bytes() } };
        let bytes: Option<Vec<u8>> = match k.key.as_str() {
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(if m.alt { b"\x1b\x7f".to_vec() } else { b"\x7f".to_vec() }),
            "tab" => Some(if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
            "escape" => Some(b"\x1b".to_vec()),
            "up" => Some(arrow('A')),
            "down" => Some(arrow('B')),
            "right" => Some(if m.alt { b"\x1bf".to_vec() } else { arrow('C') }),
            "left" => Some(if m.alt { b"\x1bb".to_vec() } else { arrow('D') }),
            "home" => Some(b"\x1b[H".to_vec()),
            "end" => Some(b"\x1b[F".to_vec()),
            "pageup" => Some(b"\x1b[5~".to_vec()),
            "pagedown" => Some(b"\x1b[6~".to_vec()),
            "delete" => Some(b"\x1b[3~".to_vec()),
            key if m.control && key.len() == 1 => {
                let c = key.as_bytes()[0];
                if c.is_ascii_lowercase() { Some(vec![c - b'a' + 1]) } else { None }
            }
            _ => k.key_char.as_ref().map(|s| { let mut v = s.clone().into_bytes(); if m.alt { v.insert(0, 0x1b); } v }),
        };
        if let Some(b) = bytes { self.write(b); self.term.lock().scroll_display(Scroll::Bottom); cx.notify(); }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus.clone() }
}

fn ansi(theme: &crate::theme::Theme, c: AColor, is_fg: bool) -> Hsla {
    let named = |n: NamedColor| -> Hsla {
        let dark = theme.dark;
        let hex = |d: u32, l: u32| -> Hsla { rgb(if dark { d } else { l }).into() };
        match n {
            NamedColor::Black | NamedColor::DimBlack => hex(0x2a2a2e, 0x1e1d1b),
            NamedColor::Red | NamedColor::DimRed => hex(0xf27a82, 0xcc3a45),
            NamedColor::Green | NamedColor::DimGreen => hex(0x7fd08d, 0x2f8a43),
            NamedColor::Yellow | NamedColor::DimYellow => hex(0xe6c27a, 0xa87a14),
            NamedColor::Blue | NamedColor::DimBlue => hex(0x82b6f2, 0x1f5fbf),
            NamedColor::Magenta | NamedColor::DimMagenta => hex(0xc7a9f2, 0x7c3aed),
            NamedColor::Cyan | NamedColor::DimCyan => hex(0x6fd3c9, 0x0f766e),
            NamedColor::White | NamedColor::DimWhite => hex(0xd8d6d2, 0x6d6b67),
            NamedColor::BrightBlack => hex(0x6c6b68, 0x8a8883),
            NamedColor::BrightRed => hex(0xff9aa0, 0xe0525c),
            NamedColor::BrightGreen => hex(0xa0e3ab, 0x3fa656),
            NamedColor::BrightYellow => hex(0xf2d595, 0xc29220),
            NamedColor::BrightBlue => hex(0xa3c9f7, 0x3a7ad6),
            NamedColor::BrightMagenta => hex(0xdac4f7, 0x9559f0),
            NamedColor::BrightCyan => hex(0x92e3da, 0x1b9187),
            NamedColor::BrightWhite => hex(0xffffff, 0x1e1d1b),
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => theme.text,
            NamedColor::Background => theme.bg,
            NamedColor::Cursor => theme.accent,
        }
    };
    match c {
        AColor::Named(n) => named(n),
        AColor::Spec(Rgb { r, g, b }) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into(),
        AColor::Indexed(i) => {
            let i = i as usize;
            if i < 16 {
                const N: [NamedColor; 16] = [NamedColor::Black, NamedColor::Red, NamedColor::Green, NamedColor::Yellow, NamedColor::Blue, NamedColor::Magenta, NamedColor::Cyan, NamedColor::White, NamedColor::BrightBlack, NamedColor::BrightRed, NamedColor::BrightGreen, NamedColor::BrightYellow, NamedColor::BrightBlue, NamedColor::BrightMagenta, NamedColor::BrightCyan, NamedColor::BrightWhite];
                named(N[i])
            } else if i < 232 {
                let v = i - 16;
                let lv = |n: usize| -> u32 { if n == 0 { 0 } else { 55 + 40 * n as u32 } };
                rgb((lv(v / 36) << 16) | (lv((v / 6) % 6) << 8) | lv(v % 6)).into()
            } else {
                let g = 8 + 10 * (i as u32 - 232);
                rgb((g << 16) | (g << 8) | g).into()
            }
        }
    }
    .pipe(|c| if is_fg { c } else { c })
}

trait Pipe: Sized { fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R { f(self) } }
impl<T> Pipe for T {}

impl Render for TerminalView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let entity = cx.entity();
        let entity2 = entity.clone();
        div().id("terminal").size_full().bg(t.bg).pl(px(14.)).pt(px(2.)).key_context("Terminal").track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, _| window.focus(&this.focus)))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let d = ev.delta.pixel_delta(px(this.cell_h));
                let lines = (d.y.0 / this.cell_h).round() as i32;
                if lines != 0 { this.term.lock().scroll_display(Scroll::Delta(lines)); cx.notify(); }
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        // 度量字符格尺寸，并按可用区域调整终端行列数
                        let f = font(MONO_FONT);
                        let run = TextRun { len: 1, font: f, color: hsla(0., 0., 1., 1.), background_color: None, underline: None, strikethrough: None };
                        let sh = window.text_system().shape_line("M".into(), px(12.5), &[run], None);
                        let (cw, ch) = (sh.width.0, 17.0);
                        entity.update(cx, |t, _| { t.cell_w = cw; t.cell_h = ch; t.resize_to(bounds.size.width.0 - 14.0, bounds.size.height.0 - 2.0); });
                        bounds
                    },
                    move |bounds, _, window, cx| {
                        let theme = cx.theme().clone();
                        let focused = entity2.read(cx).focus.is_focused(window);
                        let (cw, ch) = { let t = entity2.read(cx); (t.cell_w, t.cell_h) };
                        let term = entity2.read(cx).term.clone();
                        let term = term.lock();
                        let content = term.renderable_content();
                        let offset = content.display_offset as i32;
                        let origin = bounds.origin;
                        // 逐行收集单元格
                        let mut rows: std::collections::BTreeMap<i32, Vec<(usize, char, Hsla, Hsla, Flags)>> = Default::default();
                        for c in content.display_iter {
                            let line = c.point.line.0 + offset;
                            if line < 0 { continue; }
                            let cell = c.cell;
                            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) { continue; }
                            let (mut fg, mut bg) = (ansi(&theme, cell.fg, true), ansi(&theme, cell.bg, false));
                            if cell.flags.contains(Flags::INVERSE) { std::mem::swap(&mut fg, &mut bg); }
                            if cell.flags.contains(Flags::DIM) { fg = fg.opacity(0.6); }
                            rows.entry(line).or_default().push((c.point.column.0, cell.c, fg, bg, cell.flags));
                        }
                        let cursor = content.cursor;
                        let sel = content.selection;
                        let f = font(MONO_FONT);
                        for (line, cells) in rows {
                            let y = origin.y + px(line as f32 * ch);
                            // 背景（与主题底色不同的才画）
                            let mut i = 0;
                            while i < cells.len() {
                                let bg = cells[i].3;
                                let mut j = i;
                                while j + 1 < cells.len() && cells[j + 1].3 == bg && cells[j + 1].0 == cells[j].0 + 1 { j += 1; }
                                if bg != theme.bg && bg.a > 0.0 {
                                    let w = (cells[j].0 - cells[i].0 + 1) as f32 * cw;
                                    window.paint_quad(fill(Bounds::new(point(origin.x + px(cells[i].0 as f32 * cw), y), size(px(w), px(ch))), bg));
                                }
                                i = j + 1;
                            }
                            // 选区
                            if let Some(s) = sel {
                                let l = line - offset;
                                if l >= s.start.line.0 && l <= s.end.line.0 {
                                    let from = if l == s.start.line.0 { s.start.column.0 } else { 0 };
                                    let to = if l == s.end.line.0 { s.end.column.0 + 1 } else { 10_000 };
                                    let to = to.min(cells.last().map(|c| c.0 + 1).unwrap_or(0));
                                    if to > from { window.paint_quad(fill(Bounds::new(point(origin.x + px(from as f32 * cw), y), size(px((to - from) as f32 * cw), px(ch))), theme.selection)); }
                                }
                            }
                            // 文本：相邻同色字符合并成一段排版，段首按列位置定位，避免累积偏差
                            let mut i = 0;
                            while i < cells.len() {
                                let (col, c0, fg, _, fl) = cells[i];
                                if c0 == ' ' || fl.contains(Flags::HIDDEN) { i += 1; continue; }
                                let mut text = String::new();
                                let mut j = i;
                                while j < cells.len() && cells[j].2 == fg && cells[j].4.intersects(Flags::BOLD) == fl.intersects(Flags::BOLD) && cells[j].0 == col + (j - i) && cells[j].1 != ' ' && !cells[j].4.contains(Flags::WIDE_CHAR) || j == i {
                                    text.push(cells[j].1);
                                    j += 1;
                                    if j >= cells.len() { break; }
                                    if cells[j - 1].4.contains(Flags::WIDE_CHAR) { break; }
                                }
                                let mut font = f.clone();
                                if fl.intersects(Flags::BOLD) { font.weight = FontWeight::BOLD; }
                                if fl.intersects(Flags::ITALIC) { font.style = FontStyle::Italic; }
                                let run = TextRun { len: text.len(), font, color: fg, background_color: None, underline: fl.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle { color: Some(fg), thickness: px(1.), wavy: false }), strikethrough: None };
                                let shaped = window.text_system().shape_line(text.into(), px(12.5), &[run], None);
                                let _ = shaped.paint(point(origin.x + px(col as f32 * cw), y), px(ch), window, cx);
                                i = j.max(i + 1);
                            }
                        }
                        // 光标
                        if cursor.shape != alacritty_terminal::vte::ansi::CursorShape::Hidden {
                            let line = cursor.point.line.0 + offset;
                            if line >= 0 {
                                let rect = Bounds::new(point(origin.x + px(cursor.point.column.0 as f32 * cw), origin.y + px(line as f32 * ch)), size(px(cw), px(ch)));
                                if focused { window.paint_quad(fill(rect, theme.accent.opacity(0.85))); }
                                else { window.paint_quad(outline(rect, theme.accent, BorderStyle::default())); }
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

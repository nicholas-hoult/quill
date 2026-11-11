//! Inline Assist（⌘K / Ctrl+Enter）：对选区描述修改 → 流式替换 → 进入逐块审阅。
use crate::ai::{self, Msg, Settings, StreamEvent};
use crate::blocks::first_code;
use crate::context::INLINE_PROMPT;
use crate::editor::{Editor, EditorEvent, EditorMode};
use crate::syntax;
use crate::theme::ActiveTheme;
use crate::ui::icon;
use gpui::{prelude::*, *};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub enum InlineEvent { Close }

pub struct InlineAssist {
    target: Entity<Editor>,
    input: Entity<Editor>,
    settings: Settings,
    start: usize,
    end: usize,
    sel_text: String,
    running: bool,
    error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
    started_review: bool,
    pos: Point<Pixels>,
    _sub: Subscription,
}

impl EventEmitter<InlineEvent> for InlineAssist {}

impl InlineAssist {
    pub fn new(target: Entity<Editor>, settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| Editor::new(EditorMode::input("描述怎么改写选中的代码…"), cx));
        window.focus(&input.read(cx).focus);
        let (start, end, sel_text, pos) = target.read_with(cx, |e, _| {
            let lh = e.mode.line_height;
            let (x, y) = e.layout.position_of(e.cursor(), lh).unwrap_or((px(80.) + e.layout.bounds.left(), e.layout.bounds.top() + px(60.)));
            (e.sel.start, e.sel.end, e.selected_text(), point(x - e.layout.bounds.left(), y - e.layout.bounds.top() + px(lh + 6.)))
        });
        let sub = cx.subscribe(&input, |this, _, ev: &EditorEvent, cx| match ev {
            EditorEvent::Submit => this.run(cx),
            EditorEvent::Cancel => this.cancel(cx),
            _ => {}
        });
        Self { target, input, settings, start, end, sel_text, running: false, error: None, cancel: None, started_review: false, pos, _sub: sub }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.cancel.take() { c.store(true, Ordering::Relaxed); }
        if self.started_review { self.target.update(cx, |e, cx| e.reject_all(cx)); }
        cx.emit(InlineEvent::Close);
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let instruction = self.input.read(cx).text().trim().to_string();
        if instruction.is_empty() || self.running { return; }
        self.running = true;
        self.error = None;
        let (path, lang) = self.target.read_with(cx, |e, _| (e.path.clone().unwrap_or_default(), syntax::lang_label(e.lang)));
        let name = path.rsplit('/').next().unwrap_or("").to_string();
        let body = if self.sel_text.is_empty() {
            format!("{instruction}\n\n这是要插入到 {name}（{lang}）光标处的新代码，只输出要插入的代码。\n选中的代码：\n```\n\n```")
        } else {
            format!("{instruction}\n\n选中的代码，来自 {name}（{lang}）：\n```\n{}\n```", self.sel_text)
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded();
        let settings = self.settings.clone();
        cx.background_executor().spawn(async move { ai::run_stream(settings, INLINE_PROMPT.to_string(), vec![Msg { role: "user".into(), content: body }], tx, cancel); }).detach();
        let (start, mut cur_len) = (self.start, self.end - self.start);
        let mut acc = String::new();
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                match ev {
                    StreamEvent::Chunk(c) => {
                        acc.push_str(&c);
                        if let Some(code) = first_code(&acc) {
                            let ok = this.update(cx, |p, cx| {
                                if !p.started_review { p.started_review = true; p.target.update(cx, |e, cx| e.begin_review(cx)); }
                                let l = cur_len;
                                p.target.update(cx, |e, cx| e.replace_range_streaming(start, l, &code, cx))
                            });
                            match ok { Ok(n) => cur_len = n, Err(_) => break }
                        }
                    }
                    StreamEvent::Done => {
                        let _ = this.update(cx, |p, cx| {
                            if first_code(&acc).is_none() { p.error = Some("模型没有返回代码".into()); p.running = false; p.cancel = None; cx.notify(); }
                            else { cx.emit(InlineEvent::Close); }
                        });
                        break;
                    }
                    StreamEvent::Error(e) => {
                        let _ = this.update(cx, |p, cx| {
                            if p.started_review { p.target.update(cx, |t, cx| t.reject_all(cx)); p.started_review = false; }
                            p.error = Some(e); p.running = false; p.cancel = None; cx.notify();
                        });
                        break;
                    }
                }
            }
        }).detach();
        cx.notify();
    }
}

impl Render for InlineAssist {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        div().absolute().left(self.pos.x.max(px(16.))).top(self.pos.y).w(px(520.)).p(px(8.)).flex().flex_col().gap(px(4.)).rounded(px(13.)).bg(t.elev).border_1().border_color(t.accent_line)
            .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.5), offset: point(px(0.), px(14.)), blur_radius: px(40.), spread_radius: px(0.) }])
            .child(div().flex().items_center().gap(px(8.)).px(px(4.)).child(icon("agent", 15., t.accent)).child(div().flex_1().h(px(30.)).flex().items_center().child(div().flex_1().child(self.input.clone()))))
            .child(if self.running {
                div().h(px(2.)).mx(px(4.)).mt(px(4.)).rounded(px(2.)).bg(t.accent).opacity(0.8).into_any_element()
            } else {
                div().px(px(6.)).pt(px(2.)).flex().gap(px(10.)).text_size(px(11.5)).text_color(t.faint)
                    .when_some(self.error.clone(), |d, e| d.child(div().text_color(t.del).child(e)))
                    .child("↵ 生成").child("Esc 取消").child("生成后可逐块接受 / 拒绝").into_any_element()
            })
    }
}

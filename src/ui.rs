//! 小组件：图标、按钮、键帽、文件类型小图标。
use crate::theme::ActiveTheme;
use gpui::{prelude::*, *};

pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg().path(SharedString::from(format!("icons/{name}.svg"))).size(px(size)).flex_none().text_color(color)
}

/// 28×28 的图标按钮（悬停高亮、按下缩放感）
pub fn icon_button(id: impl Into<ElementId>, name: &str, active: bool, cx: &App) -> Stateful<Div> {
    let t = cx.theme();
    let (hover, press, dim, text, accent, soft) = (t.hover, t.press, t.dim, t.text, t.accent, t.accent_soft);
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .when(active, |d| d.bg(soft))
        .hover(move |s| s.bg(if active { soft } else { hover }).text_color(text))
        .active(move |s| s.bg(press))
        .child(icon(name, 16., if active { accent } else { dim }))
}

pub fn kbd(label: &str, cx: &App) -> Div {
    let t = cx.theme();
    div().flex_none().px(px(5.)).h(px(18.)).flex().items_center().rounded(px(5.)).bg(t.hover).border_1().border_color(t.border).text_size(px(11.)).text_color(t.faint).child(label.to_string())
}

fn ext_color(ext: &str) -> u32 {
    match ext {
        "rs" => 0xe58a5a, "ts" | "tsx" => 0x5aa0e6, "js" | "jsx" | "mjs" => 0xe6c27a, "json" => 0xc9a45a, "md" => 0x8aa0b8, "py" => 0x6aa6d6,
        "html" => 0xe5784f, "css" => 0x8d7ae6, "toml" | "lock" => 0xa0a0a0, "yaml" | "yml" => 0xc97a7a, "sh" => 0x7fd08d, "go" => 0x5fc9d6,
        _ => 0x77756f,
    }
}

/// 文件类型小方块（扩展名首两个字母 + 配色）
pub fn file_icon(name: &str, cx: &App) -> Div {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let label: String = if ext.is_empty() { name.chars().take(1).collect::<String>().to_uppercase() } else { ext.chars().take(2).collect::<String>().to_uppercase() };
    div().flex_none().size(px(14.)).rounded(px(3.)).flex().items_center().justify_center().bg(rgb(ext_color(&ext))).text_color(cx.theme().bg).text_size(px(7.)).font_weight(FontWeight::BOLD).font_family(crate::theme::MONO_FONT).child(label)
}

pub fn btn(id: impl Into<ElementId>, label: &str, primary: bool, cx: &App) -> Stateful<Div> {
    let t = cx.theme();
    let d = div().id(id).h(px(28.)).px(px(12.)).flex().items_center().justify_center().gap(px(6.)).rounded(px(8.)).cursor_pointer().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(label.to_string());
    if primary {
        let (acc, ink) = (t.accent, t.accent_ink);
        d.bg(acc).text_color(ink).hover(|s| s.opacity(0.9)).active(|s| s.opacity(0.8))
    } else {
        let (h, p, b, tx) = (t.hover, t.press, t.border, t.text);
        d.bg(h).border_1().border_color(b).text_color(tx).hover(move |s| s.bg(p)).active(move |s| s.opacity(0.85))
    }
}

/// 轻量 Markdown 渲染（段落 / 列表 / 行内代码 / 粗体），悬停卡片等处使用。
pub fn md_text(text: &str, cx: &App) -> Div {
    use crate::md::{self, Block};
    let t = cx.theme();
    let inline = |s: &str| -> StyledText {
        let (plain, spans) = md::parse_inline(s);
        let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = spans.into_iter().map(|sp| {
            let st = if sp.code { HighlightStyle { color: Some(t.accent), background_color: Some(t.hover), ..Default::default() } } else { HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() } };
            (sp.range, st)
        }).collect();
        StyledText::new(plain).with_highlights(hl)
    };
    let mut col = div().flex().flex_col().gap(px(6.));
    for b in md::parse_blocks(text) {
        col = col.child(match b {
            Block::Heading(_, s) => div().font_weight(FontWeight::BOLD).child(inline(&s)),
            Block::Bullet(s) => div().flex().gap(px(6.)).pl(px(2.)).child(div().flex_none().text_color(t.faint).child("•")).child(div().flex_1().min_w_0().child(inline(&s))),
            Block::Numbered(n, s) => div().flex().gap(px(6.)).child(div().flex_none().text_color(t.faint).child(n)).child(div().flex_1().min_w_0().child(inline(&s))),
            Block::Paragraph(s) => div().child(inline(&s)),
        });
    }
    col
}

/// 带语法高亮的代码片段（等宽）
pub fn code_snippet(code: &str, lang: &str, cx: &App) -> Div {
    let t = cx.theme();
    let l = crate::syntax::lang_for_name(lang);
    let hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = match l {
        Some(l) => crate::syntax::highlight(l, code).into_iter().map(|(r, k)| (r, HighlightStyle { color: Some(t.syntax(k)), ..Default::default() })).collect(),
        None => vec![],
    };
    div().px(px(10.)).py(px(7.)).rounded(px(8.)).bg(t.bg).border_1().border_color(t.border).font_family(crate::theme::MONO_FONT).text_size(px(12.)).line_height(px(18.)).text_color(t.text)
        .child(StyledText::new(code.to_string()).with_highlights(hl))
}

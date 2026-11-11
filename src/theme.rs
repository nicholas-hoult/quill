//! 主题：暖石墨 + 琥珀强调色。深色 / 浅色两套，全局唯一，界面各处通过 `cx.theme()` 读取。
use crate::syntax::Kind;
use gpui::{App, Global, Hsla, Rgba, rgb, rgba};

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub bg: Hsla,
    pub panel: Hsla,
    pub elev: Hsla,
    pub hover: Hsla,
    pub press: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub dim: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_ink: Hsla,
    pub accent_soft: Hsla,
    pub accent_line: Hsla,
    pub add: Hsla,
    pub add_bg: Hsla,
    pub del: Hsla,
    pub del_bg: Hsla,
    pub modified: Hsla,
    pub selection: Hsla,
    pub line_active: Hsla,
    pub scrim: Hsla,
    pub syn: [Hsla; 14],
}

impl Global for Theme {}

fn h(c: u32) -> Hsla { rgb(c).into() }
fn ha(c: u32) -> Hsla { Rgba::from(rgba(c)).into() }

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            bg: h(0x0e0e10), panel: h(0x131315), elev: h(0x1b1b1e),
            hover: ha(0xffffff0d), press: ha(0xffffff17),
            border: ha(0xffffff10), border_strong: ha(0xffffff1c),
            text: h(0xe9e7e4), dim: h(0x9a9894), faint: h(0x5f5e5b),
            accent: h(0xff9b5e), accent_ink: h(0x1b0f06), accent_soft: ha(0xff9b5e21), accent_line: ha(0xff9b5e61),
            add: h(0x7fd08d), add_bg: ha(0x7fd08d1f), del: h(0xf27a82), del_bg: ha(0xf27a821f), modified: h(0xe6c27a),
            selection: ha(0xff9b5e3d), line_active: ha(0xffffff06), scrim: ha(0x00000066),
            syn: [
                h(0xff9a7a), h(0xa6d6a1), h(0xe6c27a), h(0x82b6f2), h(0x6fd3c9), h(0xc7a9f2), h(0x6c6b68),
                h(0x9c9b98), h(0x9c9b98), h(0xe6c27a), h(0xe9e7e4), h(0xff9a7a), h(0xc7a9f2), h(0x6fd3c9),
            ],
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            bg: h(0xfbfaf8), panel: h(0xf3f1ee), elev: h(0xffffff),
            hover: ha(0x0000000b), press: ha(0x00000015),
            border: ha(0x00000012), border_strong: ha(0x00000021),
            text: h(0x1e1d1b), dim: h(0x6d6b67), faint: h(0xa8a6a1),
            accent: h(0xd9581f), accent_ink: h(0xffffff), accent_soft: ha(0xd9581f1a), accent_line: ha(0xd9581f66),
            add: h(0x2f8a43), add_bg: ha(0x2f8a431c), del: h(0xcc3a45), del_bg: ha(0xcc3a451a), modified: h(0xa87a14),
            selection: ha(0xd9581f2e), line_active: ha(0x00000006), scrim: ha(0x3c322830),
            syn: [
                h(0xc2410c), h(0x2e7d32), h(0x9a6700), h(0x1f5fbf), h(0x0f766e), h(0x7c3aed), h(0xa09e99),
                h(0x78766f), h(0x78766f), h(0x9a6700), h(0x1e1d1b), h(0xc2410c), h(0x7c3aed), h(0x0f766e),
            ],
        }
    }

    pub fn syntax(&self, k: Kind) -> Hsla {
        self.syn[match k {
            Kind::Keyword => 0, Kind::String => 1, Kind::Number => 2, Kind::Function => 3, Kind::Type => 4, Kind::Property => 5,
            Kind::Comment => 6, Kind::Operator => 7, Kind::Punct => 8, Kind::Constant => 9, Kind::Variable => 10, Kind::Tag => 11,
            Kind::Attribute => 12, Kind::Escape => 13,
        }]
    }
}

pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme { self.global::<Theme>() }
}

pub const UI_FONT: &str = ".SystemUIFont";
pub const MONO_FONT: &str = "Menlo";

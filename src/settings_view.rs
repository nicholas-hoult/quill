//! 设置浮层：主题、模型服务、Key、模型名、Base URL。
use crate::ai::Settings;
use crate::editor::{Editor, EditorEvent, EditorMode};
use crate::theme::ActiveTheme;
use crate::ui::btn;
use gpui::{prelude::*, *};

pub enum SettingsEvent {
    Close,
    Save(Settings),
}

pub struct SettingsView {
    focus: FocusHandle,
    s: Settings,
    key: Entity<Editor>,
    model: Entity<Editor>,
    base: Entity<Editor>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(s: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mk = |cx: &mut Context<Self>, ph: &str, v: &str| { let e = cx.new(|cx| Editor::new(EditorMode::input(ph), cx)); e.update(cx, |e, cx| e.set_text(v, cx)); e };
        let key = mk(cx, "sk-…", &s.api_key);
        let model = mk(cx, "模型名", &s.model);
        let base = mk(cx, "留空使用官方地址", &s.base_url);
        let mut subs = vec![];
        for e in [&key, &model, &base] {
            subs.push(cx.subscribe(e, |this, _, ev: &EditorEvent, cx| match ev {
                EditorEvent::Cancel => cx.emit(SettingsEvent::Close),
                EditorEvent::Submit => this.save(cx),
                _ => {}
            }));
        }
        let focus = cx.focus_handle();
        if s.provider != "mock" { window.focus(&key.read(cx).focus); } else { window.focus(&focus); }
        Self { focus, s, key, model, base, _subs: subs }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let mut s = self.s.clone();
        s.api_key = self.key.read(cx).text();
        s.model = self.model.read(cx).text();
        s.base_url = self.base.read(cx).text();
        cx.emit(SettingsEvent::Save(s));
    }
}

fn seg(id: &'static str, items: &[(&'static str, &'static str)], current: &str, cx: &mut Context<SettingsView>, set: fn(&mut SettingsView, &str)) -> Div {
    let t = cx.theme().clone();
    div().flex().p(px(2.)).gap(px(2.)).rounded(px(9.)).bg(t.hover).children(items.iter().map(|(val, label)| {
        let on = *val == current;
        let v = *val;
        div().id(SharedString::from(format!("{id}-{val}"))).flex_1().h(px(26.)).flex().items_center().justify_center().rounded(px(7.)).cursor_pointer().text_color(if on { t.text } else { t.dim })
            .when(on, |d| d.bg(t.elev).border_1().border_color(t.border)).on_click(cx.listener(move |this, _, _, cx| { set(this, v); cx.notify(); })).child(*label)
    }))
}

impl Render for SettingsView {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let field = |label: &str, e: &Entity<Editor>| div().flex().items_center().gap(px(10.)).child(div().w(px(80.)).text_color(t.dim).child(label.to_string())).child(div().flex_1().h(px(32.)).px(px(10.)).flex().items_center().rounded(px(8.)).bg(t.bg).border_1().border_color(t.border).child(div().flex_1().child(e.clone())));
        let mock = self.s.provider == "mock";
        div().id("settings-scrim").track_focus(&self.focus).on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| { if ev.keystroke.key == "escape" { cx.emit(SettingsEvent::Close); } })).absolute().inset_0().flex().justify_center().items_center().bg(t.scrim).occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
            .child(
                div().w(px(500.)).p(px(22.)).flex().flex_col().gap(px(14.)).rounded(px(16.)).bg(t.elev).border_1().border_color(t.border_strong)
                    .shadow(vec![BoxShadow { color: hsla(0., 0., 0., 0.5), offset: point(px(0.), px(20.)), blur_radius: px(60.), spread_radius: px(0.) }])
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("设置"))
                    .child(div().flex().items_center().gap(px(10.)).child(div().w(px(80.)).text_color(t.dim).child("主题")).child(div().flex_1().child(seg("theme", &[("system", "跟随系统"), ("dark", "深色"), ("light", "浅色")], &self.s.theme.clone(), cx, |this, v| this.s.theme = v.to_string()))))
                    .child(div().flex().items_center().gap(px(10.)).child(div().w(px(80.)).text_color(t.dim).child("模型服务")).child(div().flex_1().child(seg("prov", &[("mock", "演示"), ("anthropic", "Anthropic"), ("openai", "OpenAI 兼容")], &self.s.provider.clone(), cx, |this, v| this.s.provider = v.to_string()))))
                    .when(mock, |d| d.child(div().pl(px(90.)).text_size(px(12.)).text_color(t.faint).child("演示模式不联网，只回放固定回复，用来体验交互。")))
                    .when(!mock, |d| d.child(field("API Key", &self.key)).child(field("模型", &self.model)).child(field("Base URL", &self.base))
                        .child(div().pl(px(90.)).text_size(px(12.)).text_color(t.faint).child("Key 只保存在本机配置文件里。")))
                    .child(div().flex().justify_end().gap(px(8.)).mt(px(6.))
                        .child(btn("set-cancel", "取消", false, cx).on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))))
                        .child(btn("set-save", "保存", true, cx).on_click(cx.listener(|this, _, _, cx| this.save(cx))))),
            )
    }
}

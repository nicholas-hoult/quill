mod acp;
mod acp_client;
mod actions;
mod agent;
mod analysis;
mod agent_proto;
mod tools;
mod ai;
mod assets;
mod blocks;
mod checkpoint;
mod buffer;
mod caret;
mod context;
mod diff;
mod editor;
mod editor_element;
mod fuzzy;
mod git;
mod inline;
mod lsp;
mod md;
mod models;
mod multicursor;
mod outline;
mod palette;
mod rules;
mod settings_view;
mod shot;
mod syntax;
mod terminal;
mod theme;
mod tree;
mod ui;
mod workspace;

use gpui::{prelude::*, *};
use std::path::PathBuf;
use std::time::Duration;
use workspace::Workspace;

/// 启动时预热字体排版：第一次排版某个字体 / 中文回退字体会有一次性的初始化成本，
/// 放在启动阶段付掉，而不是让用户第一次打开文件时承担。
fn warm_up_text(window: &Window) {
    let ts = window.text_system();
    let sample_ascii: SharedString = "fn main() { let x = 42; // abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789 +-*/=<>(){}[]:;,.\"'`_#!?@&|\\^~% }".into();
    let sample_cjk: SharedString = "你好，世界。这是一段用来预热中文回退字体的文字：文件、编辑、搜索、对话。".into();
    for (family, size) in [(theme::MONO_FONT, 13.0f32), (theme::MONO_FONT, 12.0), (theme::UI_FONT, 13.0), (theme::UI_FONT, 12.0)] {
        for text in [&sample_ascii, &sample_cjk] {
            let run = TextRun { len: text.len(), font: font(family), color: hsla(0., 0., 0., 1.), background_color: None, underline: None, strikethrough: None };
            let _ = ts.shape_line(text.clone(), px(size), &[run.clone()], None);
            let _ = ts.shape_text(text.clone(), px(size), &[run], None, None);
        }
    }
}

fn main() {
    Application::new().with_assets(assets::Assets).run(|cx: &mut App| {
        actions::bind_keys(cx);
        cx.set_global(theme::Theme::dark());
        // 兜底：没有任何视图有焦点时（例如点菜单栏）直接退出；有工作区时由它先检查未保存的改动
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu { name: "Quill".into(), items: vec![MenuItem::action("打开设置", actions::OpenSettings), MenuItem::separator(), MenuItem::action("退出 Quill", actions::Quit)] },
            Menu { name: "文件".into(), items: vec![MenuItem::action("打开文件夹…", actions::OpenFolder), MenuItem::action("保存", actions::Save), MenuItem::action("关闭标签页", actions::CloseTab)] },
            Menu { name: "视图".into(), items: vec![MenuItem::action("命令面板", actions::CommandPalette), MenuItem::action("跳转到文件", actions::QuickOpen), MenuItem::action("切换侧栏", actions::ToggleSidebar), MenuItem::action("切换 AI 对话", actions::ToggleAgent)] },
        ]);

        let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(560.))),
                    titlebar: Some(TitlebarOptions { title: Some("Quill".into()), appears_transparent: true, traffic_light_position: Some(point(px(18.), px(15.))) }),
                    ..Default::default()
                },
                |window, cx| {
                    warm_up_text(window);
                    let ws = cx.new(|cx| Workspace::new(window, cx));
                    window.focus(&ws.read(cx).focus_handle(cx));
                    if let Ok(p) = std::env::var("QUILL_OPEN") { ws.update(cx, |w, cx| w.open_root(PathBuf::from(p), window, cx)); }
                    ws
                },
            )
            .expect("无法创建窗口");
        cx.activate(true);

        // 开发辅助：QUILL_SCRIPT 驱动界面并截图（见 TRD「验证手段」）。正常使用不会设置。
        if let Ok(script) = std::env::var("QUILL_SCRIPT") {
            let any: AnyWindowHandle = handle.into();
            cx.spawn(async move |cx| {
                for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
                    let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
                    match cmd {
                        "wait" => cx.background_executor().timer(Duration::from_millis(arg.parse().unwrap_or(500))).await,
                        "key" => { let k = arg.to_string(); let _ = cx.update_window(any, |_, window, cx| { if let Ok(ks) = Keystroke::parse(&k) { window.dispatch_keystroke(ks, cx); } }); }
                        "text" => { for ch in arg.chars() { let _ = cx.update_window(any, |_, window, cx| window.dispatch_keystroke(Keystroke { modifiers: Modifiers::default(), key: ch.to_string(), key_char: Some(ch.to_string()) }, cx)); cx.background_executor().timer(Duration::from_millis(15)).await; } }
                        "click" | "dblclick" => {
                            let (x, y) = arg.split_once(',').map(|(a, b)| (a.parse::<f32>().unwrap_or(0.), b.parse::<f32>().unwrap_or(0.))).unwrap_or((0., 0.));
                            let n = if cmd == "dblclick" { 2 } else { 1 };
                            let _ = handle.update(cx, |ws, _, cx| ws.debug_click(x, y, n, cx));
                        }
                        "shot" => { if let Err(e) = shot::capture_own_window(arg) { eprintln!("截图失败：{e}"); } }
                        "quit" => { let _ = cx.update(|cx| cx.quit()); }
                        _ => eprintln!("未知步骤：{step}"),
                    }
                }
            }).detach();
        }
    });
}

# Quill

对话式 AI 代码编辑器。**Rust + GPUI（官方 `gpui 0.2.2`）**，对标 Zed，要求更好看、更丝滑。文档在 `private/docs/`（#1）。
旧的 Tauri + CodeMirror 版本在标签 `tauri-v1`。

## 命令

```bash
cargo run                # 开发运行（dev 配置已开 opt-level，手感接近正式版）
cargo test               # 全部单测（纯逻辑层）
cargo build --release    # 发布构建（首次很慢）
```

首次编译 gpui 约 600 个依赖，Intel 4 核上很慢；本机有多个会话同时编译时更慢。

## 结构

* 纯逻辑（可单测，改动必须带测试）：`buffer`（rope + 撤销）、`diff`（逐块审阅）、`syntax`（tree-sitter 高亮）、`fuzzy`、`blocks`（围栏解析）、`context`（@ 引用 + 提示词）、`md`、`tree`、`git`、`ai`（Anthropic / OpenAI 兼容 / mock，阻塞式 SSE 放后台线程）
* 界面：`editor` + `editor_element`（自绘编辑器；同一套代码也当单行输入框和聊天输入框，见 `EditorMode`）、`workspace`（标题栏 / 侧栏 / 标签 / 状态栏 / 面板开合动画 / 拖拽分栏 / 浮层）、`agent`（对话面板）、`palette`、`settings_view`、`inline`（⌘K）、`terminal`（alacritty_terminal 内核）、`theme`（深浅两套）、`ui`（小组件）、`assets`（内嵌 SVG 图标）
* AI 改动的落地：`Editor::apply_with_review` / `replace_range_streaming` 把新文本写进缓冲区并进入审阅，`refresh_review` 按 `diff::hunks` 生成显示行（含被删除行），`accept_hunk` / `reject_hunk` 逐块处理

## 开发时看界面（重要）

GPUI 没有截图 API，本机终端也没有屏幕录制权限。应用内置了**自截图 + 脚本驱动**（只在设置环境变量时生效）：

```bash
QUILL_OPEN=/某项目 QUILL_SCRIPT="wait:2000;key:cmd-p;text:app;key:enter;wait:500;shot:/tmp/x.png;quit" ./target/debug/quill
```

步骤：`wait:毫秒` / `key:cmd-p`（GPUI 键位写法）/ `text:字符串` / `shot:路径.png`（CoreGraphics 只截本进程窗口，不需要权限）/ `quit`。没有鼠标步骤。
跑完会在 `~/Library/Application Support/Quill/` 留下最近项目、对话记录，**测完要删**，别污染用户数据。

## 坑

* GPUI 同一元素只能设一次 `.hover()`，设两次会 panic
* `WindowHandle::update` 会占用根视图，脚本里要用 `AnyWindowHandle` + `update_window`，否则动作处理器里再更新根视图会 panic「already being updated」
* `Pixels` 的 `.0` 是私有的，用 `f32::from(px)`
* `uniform_list` 必须给明确高度，否则不显示
* 不要用宽泛的 `pkill`，本机还跑着别的项目；按 PID / 端口精确停
* 本机 git 是 2.15，没有 `branch --show-current`

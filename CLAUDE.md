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

* 纯逻辑（可单测，改动必须带测试）：`buffer`（rope + 撤销）、`diff`（逐块审阅）、`syntax`（tree-sitter 高亮）、`fuzzy`、`blocks`（围栏解析）、`context`（@ 引用 + 提示词）、`md`、`tree`、`git`（含暂存 / 提交）、`outline`（符号大纲）、`agent_proto`（工具调用协议 + 演示脚本）、`tools`（Agent 工具：read_file / list_dir / search，路径限制在项目内）、`ai`（Anthropic / OpenAI 兼容 / mock，阻塞式 SSE 放后台线程）
* 界面：`editor` + `editor_element`（自绘编辑器；同一套代码也当单行输入框和聊天输入框，见 `EditorMode`）、`workspace`（标题栏 / 侧栏 / 标签 / 状态栏 / 面板开合动画 / 拖拽分栏 / 浮层）、`agent`（对话面板）、`palette`、`settings_view`、`inline`（⌘K）、`terminal`（alacritty_terminal 内核）、`theme`（深浅两套）、`ui`（小组件）、`assets`（内嵌 SVG 图标）
* AI 改动的落地：`Editor::apply_with_review` / `replace_range_streaming` 把新文本写进缓冲区并进入审阅，`refresh_review` 按 `diff::hunks` 生成显示行（含被删除行），`accept_hunk` / `reject_hunk` 逐块处理

## 语言服务（LSP）

`lsp.rs`：JSON-RPC over stdio 客户端（纯逻辑部分——帧解析、UTF-16 列换算、响应解析——有单测）。`workspace.rs` 里按语言惰性启动服务器（rust-analyzer / typescript-language-server / pyright / gopls，环境变量 `QUILL_LSP_<RUST|TYPESCRIPT|PYTHON|GO>` 可覆盖命令），全文同步（120ms 防抖）。已实现：诊断（波浪线 + 行号栏圆点 + 状态栏计数 + 「问题」面板）、补全（输入标识符 / `.` / `:` 自动触发，⌃Space 手动）、悬停（⌘I）、转到定义（F12）。
没装真实服务器时的验证：`QUILL_LSP_RUST="python3 private/probes/mock_lsp.py"`（模拟服务器，含 UTF-16 列校验）。本机的 `rust-analyzer` 只是 rustup 空代理，需要 `rustup component add rust-analyzer`。

## 性能

界面线程不做整文件解析：`analysis.rs` 的后台工作线程一次算出 高亮 + 大纲 + 折叠范围，带文本版本号回传；`Editor::sync_analysis` 每帧先用 `Buffer.edit_log` 把旧高亮平移（`shift_spans`），再按需（首次立即、之后 120ms 防抖）提交任务，版本对不上的结果直接丢弃。`is_dirty` 按版本缓存；光标闪烁只在聚焦时重绘；磁盘变更轮询先比 mtime+大小。
测量：`QUILL_PERF=1` 在 stderr 打印 >4ms 的慢帧和 prepaint 分段耗时。大文件测试数据可自己生成（把几个源码文件循环拼接成几万行）。
打开文件速度：`Workspace::open_file` 同步部分（读文件 + rope + 创建编辑器）约 0.3–1.5ms；首帧 prepaint（CoreText 排版）约 2–3ms、paint 约 3–6ms。字形首次栅格化是大头，所以启动时 `warm_element` 用近透明文字预热图集——要点：字形缓存按 **X、Y 各 4 档亚像素位置** 分版本，预热行高必须固定为编辑器的 21px（默认行高会让 Y 档位对不上，预热白做）；不能裁剪。再补 Y 的 4 档没有额外收益，已撤掉。`QUILL_PERF=1` 的打开帧同时打印墙钟和线程 CPU 时间（机器被别的进程占满时看 CPU 时间）。
实测首次打开（release，新进程）：安静时约 6.5ms，机器忙（负载 5–10）时 8–15ms 抖动；冷启动（窗口 + GPU 初始化）不在此列。
实测（release，8000 行 Rust）：翻页最慢帧 ~11ms（主要是 CoreText 排版 45 行新文字），连续打字无慢帧；打开文件后常驻内存约 118MB。

## 模型切换（#4）

输入框右下角的模型 chip（`AgentPanel::model_chip`）→ 选择器浮层（`⌘⌥/`，与 Zed 一致；↑↓ 选、↵ 确认、Esc 关）。选项由 `models::choices` 生成：Anthropic 内置 Fable 5.2 / Opus 5.5 / Sonnet 5.5 / Haiku 5.5，OpenAI 兼容只列「用过的模型」（`Settings.models`，最近在前、最多 10 个，设置页保存和选择器选中都会记录），当前模型不在列表里就置顶。选中后 `AgentEvent::ModelChanged` 让工作区同步并保存设置，只影响之后发出的请求。演示模式没有可选模型，点击 / 快捷键直接打开设置页。浮层必须是输入框容器的**最后一个子元素**，否则会被后绘制的输入框文字盖住。

## 折叠与分屏

折叠：`outline::fold_ranges`（tree-sitter 语法块）→ `Editor.folds` → `visible_lines()` 行映射，`editor_element` 的 `fold_map` 把行号映射成显示行；任何文本修改都会清空折叠。分屏：`Workspace.split / left_tab / right_tab / focus_right`，`self.active` 始终等于聚焦窗格的标签，所以其余命令不用感知分屏；同一文件不能同时出现在两个窗格（Editor 的滚动 / 布局状态是独占的）。

## Agent 工具调用

`agent.rs` 的 `start_round` → `ai::run_agent_stream`（Anthropic 带 tools；mock 用 `agent_proto::mock_turn`；OpenAI 兼容退回纯文本）。遇到 `stop_reason == tool_use` 时 `run_tools` 执行工具并回填，再发起下一轮（上限 8 轮）。`edit_file` **不直接写盘**，发 `AgentEvent::Apply` 让工作区走逐块审阅。
无 Key 验证：在对话里发含 `[tool-demo]` 的消息，mock 会依次 列目录 → 读文件 → 提交编辑。
终端 / 诊断工具（#3）：`run_command`（`tools::exec`，`sh -c`、cwd=项目根、stdout+stderr 合并、60s 超时、取消时杀整个进程组、输出头尾各留约 10KB）每次都要用户确认——`run_tools` 是状态机：遇到 run_command 记入 `awaiting` 并 return，确认卡片上 `⌘↵` 允许 / `Esc` 拒绝 / 「本会话始终允许」（只存内存），之后 `decide` → `spawn_command`（后台线程）→ `command_done` 回到 `run_tools` 继续。`diagnostics` 读 `Workspace::all_diags`（LSP 推送的最新诊断）。OpenAI 兼容接口没有工具调用，这些工具不可用。轮数上限 `MAX_ROUNDS`=12。
验证：消息含 `[cmd-demo]`（命令成功）/ `[cmd-fail]`（命令失败）走「运行命令 → 查看诊断 → 总结」；`QUILL_DEBUG_SYSTEM=1` 发送时在 stderr 打印最终系统提示。脚本里 `key:cmd-enter` 等于点「允许运行」（脚本的 click 只走编辑器，点不到面板按钮）。

## 打包

`scripts/bundle.sh`：release 产物 → `dist/Quill.app`（ad-hoc 签名）+ zip + dmg。`BIN=target/debug/quill SUFFIX=-preview scripts/bundle.sh` 可用开发构建先出一版。release 用独立目录 `CARGO_TARGET_DIR=target-release`，不会阻塞开发构建。

## 开发时看界面（重要）

GPUI 没有截图 API，本机终端也没有屏幕录制权限。应用内置了**自截图 + 脚本驱动**（只在设置环境变量时生效）：

```bash
QUILL_OPEN=/某项目 QUILL_SCRIPT="wait:2000;key:cmd-p;text:app;key:enter;wait:500;shot:/tmp/x.png;quit" ./target/debug/quill
```

步骤：`wait:毫秒` / `key:cmd-p`（GPUI 键位写法）/ `text:字符串` / `shot:路径.png`（CoreGraphics 只截本进程窗口，不需要权限）/ `quit`。没有鼠标步骤。
应用会读写 `~/Library/Application Support/Quill/`（设置含 API Key、对话记录、最近项目）。**测试一律加 `HOME=<临时目录>` 隔离**（路径都由 `$HOME` 推出），既不污染也不会读到用户的真实数据；**绝不要对真实目录 `rm -rf`**——用户的 Quill 可能正开着，数据一删就没了（2026-11-12 出过这个事故）。

## 坑

* GPUI 同一元素只能设一次 `.hover()`，设两次会 panic
* `WindowHandle::update` 会占用根视图，脚本里要用 `AnyWindowHandle` + `update_window`，否则动作处理器里再更新根视图会 panic「already being updated」
* `Pixels` 的 `.0` 是私有的，用 `f32::from(px)`
* `uniform_list` 必须给明确高度，否则不显示
* 不要用宽泛的 `pkill`，本机还跑着别的项目；按 PID / 端口精确停
* 本机 git 是 2.15，没有 `branch --show-current`

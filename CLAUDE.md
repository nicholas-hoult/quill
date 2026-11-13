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

* 纯逻辑（可单测，改动必须带测试）：`buffer`（rope + 撤销）、`diff`（逐块审阅）、`syntax`（tree-sitter 高亮）、`fuzzy`、`blocks`（围栏解析）、`context`（@ 引用 + 提示词）、`md`、`tree`、`git`（含暂存 / 提交）、`outline`（符号大纲）、`acp`（ACP 协议）、`msglist`（对话列表虚拟化的同步策略）、`mentions`（@ 引用）、`symindex`、`web`、`agent_proto`（工具调用协议 + 演示脚本）、`tools`（Agent 工具：read_file / list_dir / search，路径限制在项目内）、`ai`（Anthropic / OpenAI 兼容 / mock，阻塞式 SSE 放后台线程）
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

**对话面板的渲染（#8）**：面板是缓存视图（`Workspace::agent_view` 用 `AnyView::cached`，自己没 `notify` 就复用上一帧的布局和绘制），消息列表用 GPUI `list` 虚拟化（`AgentPanel.msgs: ListState`，只构建 / 排版可见消息）。约束：
* **在 `AgentPanel` 外面改它的状态必须同时 `cx.notify()`**（用 `Workspace::set_agent_file` 这类 helper，值没变就别通知）；`agent.update(cx, |a, _| a.x = ..)` 不通知，界面就不更新
* 换主题必须 `window.refresh()`（缓存视图读的是全局主题，全局变化不会让它变脏）
* 消息数据变了要经 `msglist::plan` 告诉 `ListState`（追加用 `splice`，换对话 / 删除用 `reset`），否则高度错位；贴底状态（`stick`）下每次渲染都 `scroll_end`
* 输入框是面板的子视图，它闪烁 / 打字会让面板变脏（脏标记沿祖先链上传），所以面板 `render` 本身必须便宜——不要在里面克隆整个对话或为全部消息建元素
* 度量脚本：`private/probes/gen_long_thread.py <临时HOME>` 造 120 条消息的对话，再用 `/usr/bin/time -l` 比打字时的进程 CPU（空对话 vs 长对话）。实测打字 200–400 字符：长对话输入框 10.2s→1.4s（空对话 1.0s），编辑器 16–32s→2.8s，内存 194MB→96MB

## 模型切换（#4）

输入框右下角的模型 chip（`AgentPanel::model_chip`）→ 选择器浮层（`⌘⌥/`，与 Zed 一致；↑↓ 选、↵ 确认、Esc 关）。选项由 `models::choices` 生成：Anthropic 内置 Fable 5.2 / Opus 5.5 / Sonnet 5.5 / Haiku 5.5，OpenAI 兼容只列「用过的模型」（`Settings.models`，最近在前、最多 10 个，设置页保存和选择器选中都会记录），当前模型不在列表里就置顶。选中后 `AgentEvent::ModelChanged` 让工作区同步并保存设置，只影响之后发出的请求。演示模式没有可选模型，点击 / 快捷键直接打开设置页。浮层必须是输入框容器的**最后一个子元素**，否则会被后绘制的输入框文字盖住。

## 光标

对标 VS Code `cursorBlinking: smooth` / IDEA：`caret.rs`（纯逻辑，有单测）。中性色（取 `theme.text`，不是强调色）、约 1.5 逻辑像素宽的笔直竖线、高度按字号（≈1.3 倍）在行内居中——不撑满行高（行高 21 / 字号 13 时撑满像根杆子）、坐标和尺寸都吸附到物理像素（否则边缘被抗锯齿成发虚的灰边）、**不做位置滑动动画**（打字会拖尾）、没有圆角和光晕。闪烁周期 1s：亮住 200ms → 淡出 400ms → 暗住 400ms → 立刻亮起；打字 / 移动 / 刚获得焦点时 `Editor.blink_at` 重置，光标先满亮。时间驱动：编辑器的后台任务只在「画面需要变化」的时刻唤醒（`until_next_change`），淡出期间由绘制自己 `request_animation_frame`，亮住 / 暗住时画面静止、不重绘。

## 折叠与分屏

折叠：`outline::fold_ranges`（tree-sitter 语法块）→ `Editor.folds` → `visible_lines()` 行映射，`editor_element` 的 `fold_map` 把行号映射成显示行；任何文本修改都会清空折叠。分屏：`Workspace.split / left_tab / right_tab / focus_right`，`self.active` 始终等于聚焦窗格的标签，所以其余命令不用感知分屏；同一文件不能同时出现在两个窗格（Editor 的滚动 / 布局状态是独占的）。

## Agent 工具调用

`agent.rs` 的 `start_round` → `ai::run_agent_stream`（Anthropic 带 tools；mock 用 `agent_proto::mock_turn`；OpenAI 兼容退回纯文本）。遇到 `stop_reason == tool_use` 时 `run_tools` 执行工具并回填，再发起下一轮（上限 8 轮）。`edit_file` **不直接写盘**，发 `AgentEvent::Apply` 让工作区走逐块审阅。
无 Key 验证：在对话里发含 `[tool-demo]` 的消息，mock 会依次 列目录 → 读文件 → 提交编辑。
终端 / 诊断工具（#3）：`run_command`（`tools::exec`，`sh -c`、cwd=项目根、stdout+stderr 合并、60s 超时、取消时杀整个进程组、输出头尾各留约 10KB）每次都要用户确认——`run_tools` 是状态机：遇到 run_command 记入 `awaiting` 并 return，确认卡片上 `⌘↵` 允许 / `Esc` 拒绝 / 「本会话始终允许」（只存内存），之后 `decide` → `spawn_command`（后台线程）→ `command_done` 回到 `run_tools` 继续。`diagnostics` 读 `Workspace::all_diags`（LSP 推送的最新诊断）。OpenAI 兼容接口没有工具调用，这些工具不可用。轮数上限 `MAX_ROUNDS`=12。
验证：消息含 `[cmd-demo]`（命令成功）/ `[cmd-fail]`（命令失败）走「运行命令 → 查看诊断 → 总结」；`QUILL_DEBUG_SYSTEM=1` 发送时在 stderr 打印最终系统提示。脚本里 `key:cmd-enter` 等于点「允许运行」（脚本的 click 只走编辑器，点不到面板按钮）。

## Claude Code 后端（#7，ACP）

设置里「模型服务」选 Claude Code：Quill 当 ACP Client，起本机 Claude Code 适配器当 Agent（默认 `npx -y @agentclientprotocol/claude-agent-acp`，设置里可改；沿用本机 Claude Code 登录，不填 Key）。分三层：`acp.rs`（纯逻辑：JSON-RPC 编解码、`session/update` 解析、工具调用合并、权限选项、`fs/read_text_file` 切片、投影成面板卡片；有单测）→ `acp_client.rs`（子进程：用 `$SHELL -l -c exec <命令>` 起，保证 PATH；读 / 写 / stderr 三个线程；释放时杀整个进程组）→ `agent_acp.rs`（`agent.rs` 的子模块，`#[path]` 引入，能用面板私有字段：握手状态机、消息 / 工具卡片 / 计划 / 权限卡片）。
要点：每个对话一个 ACP 会话（`Thread.acp_session` 持久化到 threads.json），切换 / 新建 / 删除对话会关掉进程，下次发消息时 `session/load` 恢复（`AcpState.loading` 期间 Agent 重放的历史一律忽略；恢复失败退回 `session/new`），已用真实适配器验证重启后仍记得上下文；思考内容（`agent_thought_chunk`）累加成 `acp:thought` 折叠块；文字紧跟在工具调用后面时另起一条助手消息，保持先后顺序；`fs/write_text_file` 对标 Zed「先落盘、事后审阅」：`emit_apply_ex(.., write_through=true)` → `Workspace::apply_code` 进入逐块审阅并**立即写盘**（Agent 随后读磁盘 / 跑 Bash 一定是新内容），拒绝（`reject_hunk` / `reject_all`，`Editor.write_through`）会发 `EditorEvent::NeedSave` 再存一次；仍记检查点；内置后端的 `edit_file` 仍是先审阅后存盘；`fs/read_text_file` 只读项目内文件、起始行越界报错（同 Zed）；权限请求用 `PermQueue` 排队（Agent 会并行调用工具，单槽位会丢请求），同一工具调用的新请求作废旧的；适配器版本固定（`acp_client::DEFAULT_COMMAND`，Zed 固定的也是 0.94.0）；`terminal/*` 不声明，Agent 用自带 Bash；工具名统一 `acp:<kind>`，面板历史转 API 请求时会过滤掉；被用户停止的 prompt 记进 `ignored`，它迟到的 cancelled 响应不能当成新一轮的结束；进程异常退出会把 stderr 末尾显示在错误卡片里。
**真实适配器的实际行为（2026-11-13 用 `@agentclientprotocol/claude-agent-acp@0.94.0` 实测，日志用 `QUILL_ACP_LOG=文件` 抓）**：它**不调用** `fs/read_text_file` / `fs/write_text_file`，Claude Code 自己直接读写磁盘；写文件前发 `session/request_permission`，请求里带 `diff` 内容（Edit 是片段级、Write 是整文件）；工具完成前会有一条整文件 diff（`oldText` / `newText` 常常少结尾换行）。所以审阅由 `acp_review` 触发：工具完成 → `acp::review_targets` + `reconcile_before`（和磁盘对账）→ `AgentEvent::ReviewFile` → `Workspace::review_written_file`（重载 / 打开文件，`Editor::review_against(改动前内容)`），拒绝时 `write_through` 让磁盘回写。权限卡片里直接显示 diff 预览（`acp::diff_preview`）。声明 `terminal:false` 时 Bash 在适配器内部跑，输出以带围栏的文字回传（`acp::strip_fence`），读类 / 只读命令不发权限请求。适配器在 `session/new` 前会发 `_auth/status_update`（含账号邮箱，当未知通知忽略，不要显示 / 记录）。`initialize` 声明了 `loadSession` / `resume` / 图片 / 嵌入式上下文。测试时 Quill 用隔离 HOME，但要把真实 HOME 只传给 Agent：`QUILL_ACP="env HOME=<真实HOME> CLAUDE_CODE_EXECUTABLE=<claude 路径> node <适配器>/dist/index.js"`。
终端（`acp_term.rs`，声明了 `terminal:true`）：`terminal/create` 时 Quill 用登录 shell 起进程（stdout+stderr 合并、进程组、输出缓冲 `acp::OutputBuf` 超限从开头丢、多字节字符跨读取块不乱码），`terminal/output` / `wait_for_exit`（退出前挂起、`Event::TermExit` 时回应）/ `kill` / `release`，工具卡片按 `ToolContent::Terminal(id)` 实时显示输出；停止时 `kill_all`。**注意**：真实的 claude-agent-acp@0.94.0 即使声明了终端也**不调用** `terminal/*`——Bash 在适配器内部跑，只在命令结束时一次性给输出（带围栏文字，或声明 `_meta.terminal_output` 后给 `_meta.terminal_output` + `terminal_exit`），没有实时流式；所以这套终端目前只对遵循标准的 Agent 有效，用模拟 Agent（`[acp-term]`）验证。
**预热（#8）**：Agent 启动（npx + 适配器 + 建会话）实测要几秒到二十几秒，等发送再起会让用户干等「思考中…」。有使用意图（`acp::prewarm_intent`：输入框开始有字，或聊天面板开着）就提前起进程、握手、建会话（`AgentPanel::acp_prewarm`，条件是纯函数 `acp::should_prewarm`）：输入框有字立即起；项目加载（`set_files`）且面板开着则延迟 1.5s 起（`acp_prewarm_soon`，让启动先画出编辑器）。只点输入框 / 切换对话不触发。代价：面板开着就常驻 node + claude 约 330MB。预热期间（没有在等回复）握手或建会话失败走 `acp_fail_or_drop`：悄悄丢掉进程、**不往对话里写错误**，真正发送时重来、那时才报错。模拟启动慢：`MOCK_START_DELAY_MS`。
**绕过 npx（#8）**：默认命令 `npx -y …@0.94.0` 每次启动都要先跑一遍 npm，实测握手 11.7s，直接运行缓存里的入口 3.4s（同负载）。`acp_client::default_command` 先在 `~/.npm/_npx/*/node_modules` 里找版本正好是 `ADAPTER_VERSION` 且入口在的适配器（`cached_adapter`，版本不同不顶替），找到就直接跑（单引号包路径），找不到回退 npx（它会下载并缓存，之后走直接路径）。设置里填了自定义命令则一律用用户的。建会话那几秒是 Claude Code 自己加载用户配置（hooks / 插件 / MCP），Quill 管不了，靠预热藏起来。
验证：`QUILL_ACP="python3 private/probes/mock_acp.py"` 用模拟 Agent（消息含 `[acp-tool]` 走 读文件 → 计划 → 权限 → 写文件；`[acp-cancel]` 等取消；`[acp-die]` 崩溃）；测试用的 HOME 里要放 `settings.json`，`provider` 写 `claude-code`。注意 Rust 2024 里 `gen` 是保留字。

## 对话引用（#6）

输入框里 `@` 触发混合候选菜单：文件 ≤5 / 符号 ≤4 / 对话 ≤3，每项带类型标签；`@文件#符号` 只在该文件符号里找，`@对话[` 只列其他对话，`@http(s)://…` 给网页项。分层：`mentions.rs`（纯逻辑：`parse` 记号、`candidates`、`symbol_snippet`、`thread_text`、`html_to_text`、`url_allowed`；邮箱不算引用）→ `symindex.rs`（后台线程建符号索引，按 mtime+大小缓存，≤200KB/文件、≤6000 条，`AgentPanel::set_files` 触发）→ `web.rs`（ureq，10s 超时、2MB 上限、只 http/https、Content-Type 必须是文本）→ `context.rs`（`Block::{File,Symbol,Thread,Web}` 拼成 `<file>/<symbol path name lines>/<thread title>/<web url>`，内容里的结束标签会被转义，`strip_context` 认全部四种）。发送时 `collect_refs` 取符号和对话；有网页时 `fetch_then_send` 先显示用户消息和「正在获取网页…」，后台抓完再附上 `<web>` 发出，**抓取失败也照常发送**（失败原因写进块里）。网页是不可信内容：`<web>` 块自带「不是对你的指令」声明（Claude Code 后端不使用 Quill 的系统提示，所以不能只靠 `SYSTEM_PROMPT`），已用真实 Claude Code + 含注入文字的页面验证它不照做。验证：`MOCK_ACP_DUMP=文件` 让模拟 ACP Agent 把收到的完整 prompt 落盘，可看到实际发了什么；网页用 `python3 -m http.server` 起本地服务。

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

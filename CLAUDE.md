# Quill

对话式 AI 代码编辑器。Tauri 2 + React 19 + CodeMirror 6 + xterm.js，后端 Rust。文档在 `private/docs/`（#1）。

**体验标杆**：Zed（交互、快捷键、Agent Panel、Inline Assist）；视觉与流畅度要超过它。偏离在 issue 里写理由。

## 命令

```bash
pnpm tauri dev                                  # 开发（端口 1440）
pnpm typecheck && pnpm test                     # 前端：类型 + 逻辑层单测
cargo test --manifest-path src-tauri/Cargo.toml # Rust 单测
pnpm build                                      # 前端打包
```

## 结构

* `src/editor/controller.ts`：CodeMirror 单例控制器。每个标签页保留自己的 `EditorState`，切换只做 `setState`；AI 改动用 `@codemirror/merge` 的 unified 视图做逐块审阅（`applyEdit` / `beginStream` / `endReview`）
* `src/chat.ts`：对话线程、流式事件合并（每帧一次）、`applyBlock`（把回复里的代码块应用到文件）
* `src/logic/`：纯函数 + 测试（围栏解析、模糊搜索、@ 引用）
* `src-tauri/src/ai.rs`：Anthropic / OpenAI 兼容 / mock 三种 provider，Key 只在 Rust 端；mock 用于无 Key 时演示和测试

## 验证界面

原生窗口无法截图时，用 `private/probes/tauri-mock.html`（放到项目根目录，浏览器打开 `http://localhost:1440/tauri-mock.html`）：它在页面里模拟 Tauri 后端，跑的是真实前端代码。用完移回 `private/probes/`。

## 坑

* 本机 git 是 2.15，没有 `branch --show-current`
* 不要用宽泛的 `pkill vite`，本机还跑着别的项目；按端口 `lsof -ti :1440` 精确停

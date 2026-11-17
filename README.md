# Quill

用对话改代码的编辑器，Rust + GPUI。每一处 AI 改动都以 diff 展示，可逐块接受或拒绝。

## 功能

- **对话改代码**：⌘L 呼出对话，`@` 引用文件；回复里的代码块可「应用」（⇧⌘↵ 应用最近一个），改动进入逐块审阅
- **就地改写**：选中代码后 ⌘K / Ctrl+Enter
- **编辑器基础**：⌘P 跳转文件 · ⇧⌘P 命令面板 · ⇧⌘F 项目搜索 · ⌘J 终端；tree-sitter 语法高亮、符号大纲、Git 暂存与提交
- **模型服务三选一**（设置里切换）：
  - Anthropic
  - OpenAI 兼容接口
  - Claude Code：Quill 作为 ACP（Agent Client Protocol）客户端，启动本机 Claude Code 适配器，沿用本机登录，不需要填 Key
- 不填 Key 时进入演示模式

## 状态

早期版本（0.1），只在 macOS 上开发和测试，界面和配置格式还会变。欢迎提 Issue。

## 构建

需要 macOS 和 Rust 1.85 以上（edition 2024）。

```bash
cargo run --release
```

使用 Claude Code 后端时，本机需要已安装并登录 Claude Code，以及 Node.js（适配器默认通过 `npx` 启动，命令可在设置里改）。

## 许可

MIT 或 Apache-2.0 双协议，任选其一（见 `LICENSE-MIT`、`LICENSE-APACHE`）。

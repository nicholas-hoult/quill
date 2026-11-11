# Quill

用对话改代码的编辑器，Rust + GPUI。每一处 AI 改动都以 diff 展示，可逐块接受或拒绝。

```bash
cargo run
```

- ⌘P 跳转文件 · ⇧⌘P 命令面板 · ⇧⌘F 项目搜索 · ⌘J 终端
- ⌘L 呼出对话，`@` 引用文件；回复里的代码块可「应用」（⇧⌘↵ 应用最近一个）
- ⌘K / Ctrl+Enter：选中代码后就地改写
- 设置里可切换 Anthropic / OpenAI 兼容服务；不填 Key 为演示模式

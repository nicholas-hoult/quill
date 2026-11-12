//! 模型选择器的纯逻辑（#4）：简短名称、可选项、已用过的模型列表。
//! 错了表现为：选择器里同一个模型出现两次、自定义模型选不到、当前模型没有对勾，或名称显示成一长串 ID。

/// 一个可选项
#[derive(Clone, Debug, PartialEq)]
pub struct ModelChoice {
    /// 发给 API 的模型 ID
    pub id: String,
    /// 界面上的简短名称
    pub label: String,
    /// 右侧的一句话说明（预设才有）
    pub note: String,
}

/// Anthropic 预设：(ID, 说明)
const ANTHROPIC: [(&str, &str); 4] = [
    ("claude-fable-5-2", "能力最强"),
    ("claude-opus-5-5", "复杂任务"),
    ("claude-sonnet-5-5", "均衡"),
    ("claude-haiku-5-5", "最快"),
];

/// 已用过的模型最多保留几个
pub const MAX_SAVED: usize = 10;

/// 简短名称：`claude-sonnet-5-5` → `Sonnet 5.5`；认不出的 ID 原样返回。
pub fn label(id: &str) -> String {
    let id = id.trim();
    // claude-<系列>-<主版本>-<次版本>，且系列是纯字母、两个版本号都是数字
    let parts: Vec<&str> = id.split('-').collect();
    if let ["claude", family, major, minor] = parts.as_slice() {
        let digits = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
        if !family.is_empty() && family.chars().all(|c| c.is_ascii_alphabetic()) && digits(major) && digits(minor) {
            let mut f = family.chars();
            let head: String = f.next().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default();
            return format!("{head}{} {major}.{minor}", f.as_str());
        }
    }
    id.to_string()
}

/// 选择器里的选项。mock 没有可选项；当前模型一定在列表里（不在预设 / 已用过里就置顶）。
pub fn choices(provider: &str, current: &str, saved: &[String]) -> Vec<ModelChoice> {
    if provider == "mock" { return vec![]; }
    let mk = |id: &str, note: &str| ModelChoice { id: id.to_string(), label: label(id), note: note.to_string() };
    let current = current.trim();
    let mut out: Vec<ModelChoice> = vec![];
    let presets: &[(&str, &str)] = if provider == "anthropic" { &ANTHROPIC } else { &[] };
    // 当前模型不在预设 / 已用过里（自定义）就置顶，保证它一定能看到
    let known = |id: &str| presets.iter().any(|(p, _)| *p == id) || saved.iter().any(|s| s.trim() == id);
    if !current.is_empty() && !known(current) { out.push(mk(current, "")); }
    // openai 兼容没有预设：当前模型在「已用过」里时也要排到最前
    if provider != "anthropic" && !current.is_empty() && known(current) { out.push(mk(current, "")); }
    for (id, note) in presets { out.push(mk(id, note)); }
    for s in saved {
        let s = s.trim();
        if !s.is_empty() { out.push(mk(s, "")); }
    }
    let mut seen: Vec<String> = vec![];
    out.retain(|c| if seen.contains(&c.id) { false } else { seen.push(c.id.clone()); true });
    out
}

/// 记下用过的模型：去掉空白、去重、最近的放最前、最多 `MAX_SAVED` 个。
pub fn remember(saved: &mut Vec<String>, id: &str) {
    let id = id.trim();
    if id.is_empty() { return; }
    saved.retain(|s| s.trim() != id);
    saved.insert(0, id.to_string());
    saved.truncate(MAX_SAVED);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[ModelChoice]) -> Vec<&str> { v.iter().map(|c| c.id.as_str()).collect() }

    #[test]
    fn 简短名称() {
        assert_eq!(label("claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(label("claude-fable-5-2"), "Fable 5.2");
        assert_eq!(label("claude-haiku-5-5"), "Haiku 5.5");
        assert_eq!(label("gpt-4o"), "gpt-4o");
        assert_eq!(label("claude-3-5-sonnet-20241022"), "claude-3-5-sonnet-20241022");
        assert_eq!(label("  qwen-max "), "qwen-max");
    }

    #[test]
    fn anthropic_预设齐全_当前在其中不重复() {
        let c = choices("anthropic", "claude-sonnet-5-5", &[]);
        assert_eq!(ids(&c), ["claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]);
        assert_eq!(c[2].label, "Sonnet 5.5");
        assert_eq!(c[2].note, "均衡");
    }

    #[test]
    fn anthropic_自定义当前模型置顶_已用过的接在预设后() {
        let c = choices("anthropic", "claude-experimental", &["my-ft".into(), "claude-opus-5-5".into()]);
        assert_eq!(ids(&c), ["claude-experimental", "claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "my-ft"]);
        assert_eq!(c[0].note, "");
    }

    #[test]
    fn openai_只列用过的_当前置顶_去重() {
        let c = choices("openai", "gpt-4o", &["qwen-max".into(), "gpt-4o".into(), "qwen-max".into()]);
        assert_eq!(ids(&c), ["gpt-4o", "qwen-max"]);
        let only_current = choices("openai", "deepseek-chat", &[]);
        assert_eq!(ids(&only_current), ["deepseek-chat"]);
    }

    #[test]
    fn 空的当前模型不产生空选项() {
        assert_eq!(ids(&choices("openai", "  ", &["a".into()])), ["a"]);
    }

    #[test]
    fn 演示模式没有可选项() {
        assert!(choices("mock", "claude-sonnet-5-5", &["x".into()]).is_empty());
    }

    #[test]
    fn 记住用过的模型() {
        let mut v: Vec<String> = vec![];
        remember(&mut v, "a");
        remember(&mut v, " b ");
        remember(&mut v, "a"); // 重复：提到最前，不增加
        remember(&mut v, "   "); // 空白：忽略
        assert_eq!(v, ["a", "b"]);
        for i in 0..20 { remember(&mut v, &format!("m{i}")); }
        assert_eq!(v.len(), MAX_SAVED);
        assert_eq!(v[0], "m19");
        assert!(!v.contains(&"a".to_string()));
    }
}

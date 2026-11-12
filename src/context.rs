//! @文件 引用决定模型能看到什么。错了表现为「我明明 @ 了文件，模型却说没看到」，或把邮箱里的 @ 当成文件。

pub fn find_mentions(text: &str, files: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let is_path = |c: char| c.is_ascii_alphanumeric() || "_./-".contains(c);
    for (k, &(i, c)) in chars.iter().enumerate() {
        if c != '@' { continue; }
        if k > 0 {
            let p = chars[k - 1].1;
            if p.is_ascii_alphanumeric() || p == '_' || p == '@' { continue; } // 邮箱
        }
        let rest = &text[i + 1..];
        let end = rest.char_indices().find(|&(_, ch)| !is_path(ch)).map(|(j, _)| j).unwrap_or(rest.len());
        let name = rest[..end].trim_end_matches('.');
        if name.is_empty() { continue; }
        let hit = files.iter().find(|f| f.as_str() == name).or_else(|| files.iter().find(|f| f.ends_with(&format!("/{name}"))));
        if let Some(h) = hit { if !out.contains(h) { out.push(h.clone()); } }
    }
    out
}

pub fn build_user_message(text: &str, files: &[(String, String)]) -> String {
    if files.is_empty() { return text.to_string(); }
    let ctx: Vec<String> = files.iter().map(|(p, c)| format!("<file path=\"{p}\">\n{c}\n</file>")).collect();
    format!("{text}\n\n{}", ctx.join("\n"))
}

/// 历史里展示给人看的文本：去掉附带的文件正文。
pub fn strip_context(content: &str) -> &str {
    match content.find("\n\n<file path=\"") { Some(i) => &content[..i], None => content }
}

pub const SYSTEM_PROMPT: &str = "你是 Quill，嵌在代码编辑器里的编程助手。用中文回答，简洁直接。\n当你要修改文件时，输出该文件的**完整新内容**，放在一个围栏代码块里，围栏信息写成 ```语言 path=相对路径，例如 ```rust path=src/main.rs。\n不要用省略号或\"其余不变\"代替未改动的部分。一次回复里每个文件最多一个代码块。";

pub const TOOL_HINT: &str = "你可以调用工具了解项目：read_file / list_dir / search。需要修改文件时调用 edit_file，给出该文件修改后的完整新内容（会进入用户的逐块审阅，不会直接写盘）。先读再改，不要凭空猜测文件内容。";

pub const INLINE_PROMPT: &str = "你是代码改写器。只输出改写后的代码，放进一个围栏代码块里，不要任何解释。保持原有缩进与风格。";

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> { vec!["src/app.ts".into(), "src/lib/util.ts".into(), "README.md".into()] }

    #[test]
    fn 按全路径或文件名匹配() {
        let r = find_mentions("看看 @src/app.ts 和 @util.ts，再 @README.md。", &files());
        assert_eq!(r, vec!["src/app.ts", "src/lib/util.ts", "README.md"]);
    }

    #[test]
    fn 邮箱不算_重复只计一次() {
        assert_eq!(find_mentions("发给 a@b.com @app.ts @app.ts", &files()), vec!["src/app.ts"]);
    }

    #[test]
    fn 上下文拼接后可还原() {
        let m = build_user_message("改一下", &[("a.ts".into(), "x".into())]);
        assert!(m.contains("<file path=\"a.ts\">"));
        assert_eq!(strip_context(&m), "改一下");
        assert_eq!(build_user_message("hi", &[]), "hi");
    }
}

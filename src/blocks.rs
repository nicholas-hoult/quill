//! 解析模型回复里的 Markdown 围栏代码块。回复边流式边解析，围栏可能没闭合、info 串写法不统一；
//! 解析错了，UI 上表现为「应用」按钮找不到目标文件，或半截代码被当成正文。

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodeBlock {
    pub lang: String,
    pub path: Option<String>,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    Text(String),
    Code { block: CodeBlock, closed: bool },
}

/// 支持 `ts path=a/b.ts`、`ts:a/b.ts`、`path="a b.ts"`、`a/b.ts`、`python`。
pub fn parse_info(info: &str) -> (String, Option<String>) {
    let s = info.trim();
    if s.is_empty() {
        return (String::new(), None);
    }
    let first = s.split_whitespace().next().unwrap_or("");
    for key in ["path=", "file=", "filename="] {
        if let Some(i) = s.find(key) {
            if i == 0 || s.as_bytes()[i - 1] == b' ' {
                let rest = &s[i + key.len()..];
                let path = if let Some(r) = rest.strip_prefix('"') {
                    r.split('"').next().unwrap_or("").to_string()
                } else {
                    rest.split_whitespace().next().unwrap_or("").to_string()
                };
                let lang = if first.contains('=') { String::new() } else { first.to_string() };
                return (lang, Some(path));
            }
        }
    }
    if let Some((l, p)) = first.split_once(':') {
        if !l.is_empty() && !p.is_empty() && l.chars().all(|c| c.is_alphanumeric() || "+#-".contains(c)) {
            return (l.to_string(), Some(p.to_string()));
        }
    }
    let looks_path = first.contains('/') || first.rsplit_once('.').map(|(_, e)| !e.is_empty() && e.chars().all(|c| c.is_alphanumeric())).unwrap_or(false);
    if looks_path { (String::new(), Some(first.to_string())) } else { (first.to_string(), None) }
}

pub fn parse_segments(md: &str) -> Vec<Segment> {
    let mut out = vec![];
    let mut text: Vec<&str> = vec![];
    let mut code: Option<Vec<&str>> = None;
    let mut info = String::new();
    let flush = |text: &mut Vec<&str>, out: &mut Vec<Segment>| {
        let t = text.join("\n");
        let t = t.trim_matches('\n');
        if !t.trim().is_empty() { out.push(Segment::Text(t.to_string())); }
        text.clear();
    };
    for line in md.split('\n') {
        match &mut code {
            None => {
                if let Some(rest) = line.strip_prefix("```") {
                    flush(&mut text, &mut out);
                    code = Some(vec![]);
                    info = rest.to_string();
                } else { text.push(line); }
            }
            Some(c) => {
                if line.trim_end() == "```" {
                    let (lang, path) = parse_info(&info);
                    out.push(Segment::Code { block: CodeBlock { lang, path, code: c.join("\n") }, closed: true });
                    code = None;
                } else { c.push(line); }
            }
        }
    }
    if let Some(c) = code {
        let (lang, path) = parse_info(&info);
        out.push(Segment::Code { block: CodeBlock { lang, path, code: c.join("\n") }, closed: false });
    } else { flush(&mut text, &mut out); }
    out
}

/// 第一个代码块的内容（Inline Assist 用），流式中未闭合也返回已有部分。
pub fn first_code(md: &str) -> Option<String> {
    parse_segments(md).into_iter().find_map(|s| match s { Segment::Code { block, .. } => Some(block.code), _ => None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_串的几种写法() {
        assert_eq!(parse_info("ts path=src/a.ts"), ("ts".into(), Some("src/a.ts".into())));
        assert_eq!(parse_info("rust:src/main.rs"), ("rust".into(), Some("src/main.rs".into())));
        assert_eq!(parse_info("path=\"a b/c.py\""), ("".into(), Some("a b/c.py".into())));
        assert_eq!(parse_info("src/x.css"), ("".into(), Some("src/x.css".into())));
        assert_eq!(parse_info("python"), ("python".into(), None));
        assert_eq!(parse_info(""), ("".into(), None));
    }

    #[test]
    fn 文本与代码块交错() {
        let s = parse_segments("先说明\n\n```ts path=a.ts\nlet x = 1\n```\n\n结束");
        assert_eq!(s.len(), 3);
        match &s[1] {
            Segment::Code { block, closed } => {
                assert_eq!(block.path.as_deref(), Some("a.ts"));
                assert_eq!(block.code, "let x = 1");
                assert!(*closed);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn 流式中未闭合的围栏() {
        let s = parse_segments("好的：\n```js\nconsole.l");
        match s.last().unwrap() { Segment::Code { closed, .. } => assert!(!closed), _ => panic!() }
        assert_eq!(first_code("```js\nab").as_deref(), Some("ab"));
        assert_eq!(first_code("纯文字"), None);
    }
}

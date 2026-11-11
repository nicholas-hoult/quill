//! 聊天里的轻量 Markdown：只处理行内 `代码` 与 **粗体**，块级（标题、列表）按行判断。
//! 单独测：解析错了，表现为行内代码的反引号漏在界面上，或高亮范围错位一两个字。
use std::ops::Range;

#[derive(Debug, PartialEq, Clone)]
pub struct InlineSpan {
    pub range: Range<usize>,
    pub code: bool,
    pub bold: bool,
}

/// 去掉标记，返回纯文本和样式区间（字节范围，指向纯文本）。
pub fn parse_inline(text: &str) -> (String, Vec<InlineSpan>) {
    let mut out = String::new();
    let mut spans = vec![];
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('`') {
            if let Some(end) = r.find('`') {
                if end > 0 && !r[..end].contains('\n') {
                    let start = out.len();
                    out.push_str(&r[..end]);
                    spans.push(InlineSpan { range: start..out.len(), code: true, bold: false });
                    rest = &r[end + 1..];
                    continue;
                }
            }
        }
        if let Some(r) = rest.strip_prefix("**") {
            if let Some(end) = r.find("**") {
                if end > 0 && !r[..end].contains('\n') {
                    let start = out.len();
                    out.push_str(&r[..end]);
                    spans.push(InlineSpan { range: start..out.len(), code: false, bold: true });
                    rest = &r[end + 2..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    (out, spans)
}

#[derive(Debug, PartialEq, Clone)]
pub enum Block {
    Heading(usize, String),
    Bullet(String),
    Numbered(String, String),
    Paragraph(String),
}

pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut out = vec![];
    let mut para: Vec<&str> = vec![];
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() { out.push(Block::Paragraph(para.join("\n"))); para.clear(); }
    };
    for line in text.lines() {
        let t = line.trim_start();
        if line.trim().is_empty() { flush(&mut para, &mut out); continue; }
        let hashes = t.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&hashes) && t[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(hashes, t[hashes + 1..].trim().to_string()));
        } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            flush(&mut para, &mut out);
            out.push(Block::Bullet(r.to_string()));
        } else if let Some((n, r)) = t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            flush(&mut para, &mut out);
            out.push(Block::Numbered(format!("{n}."), r.to_string()));
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 行内代码与粗体() {
        let (t, s) = parse_inline("用 `greet` 加 **注释** 吧");
        assert_eq!(t, "用 greet 加 注释 吧");
        assert_eq!(s[0], InlineSpan { range: 4..9, code: true, bold: false });
        assert_eq!(&t[s[1].range.clone()], "注释");
        assert!(s[1].bold);
    }

    #[test]
    fn 未闭合的标记按原样保留() {
        assert_eq!(parse_inline("a ` b").0, "a ` b");
        assert_eq!(parse_inline("**未完").0, "**未完");
        assert_eq!(parse_inline("``").0, "``");
    }

    #[test]
    fn 块级识别() {
        let b = parse_blocks("# 标题\n\n第一行\n第二行\n\n- 项目\n1. 步骤");
        assert_eq!(b[0], Block::Heading(1, "标题".into()));
        assert_eq!(b[1], Block::Paragraph("第一行\n第二行".into()));
        assert_eq!(b[2], Block::Bullet("项目".into()));
        assert_eq!(b[3], Block::Numbered("1.".into(), "步骤".into()));
    }
}

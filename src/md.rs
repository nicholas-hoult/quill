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
    /// GFM 表格：表头、每列对齐、数据行（每行补齐 / 截断到表头列数）
    Table { header: Vec<String>, aligns: Vec<Align>, rows: Vec<Vec<String>> },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Align { Left, Center, Right }

/// 按竖线切一行表格：首尾竖线可省，`\|` 是单元格里的竖线本身
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let (mut cells, mut cur, mut it) = (vec![], String::new(), t.chars().peekable());
    while let Some(c) = it.next() {
        if c == '\\' && it.peek() == Some(&'|') { cur.push('|'); it.next(); }
        else if c == '|' { cells.push(cur.trim().to_string()); cur.clear(); }
        else { cur.push(c); }
    }
    cells.push(cur.trim().to_string());
    if t.ends_with('|') && !t.ends_with("\\|") { cells.pop(); } // 末尾竖线切出的空单元格
    cells
}

/// 分隔行（`|---|:-:|--:|`）：每格都是若干个 `-`，两端可带冒号；返回每列的对齐
fn separator_aligns(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() { return None; }
    cells.iter().map(|c| {
        let core = c.trim_start_matches(':').trim_end_matches(':');
        if core.is_empty() || !core.chars().all(|ch| ch == '-') { return None; }
        Some(match (c.starts_with(':'), c.ends_with(':')) { (true, true) => Align::Center, (false, true) => Align::Right, _ => Align::Left })
    }).collect()
}

/// 显示宽度：ASCII 算 1，中日韩 / 全角算 2（用来估列宽）
pub fn display_width(s: &str) -> usize { s.chars().map(|c| if (c as u32) < 0x2E80 { 1 } else { 2 }).sum() }

/// 每列的相对宽度：该列最长内容的显示宽度，限制在 4..=40（再短也留得下几个字，再长也不把别的列挤没）
pub fn col_weights(header: &[String], rows: &[Vec<String>]) -> Vec<f32> {
    (0..header.len()).map(|i| {
        let w = std::iter::once(&header[i]).chain(rows.iter().filter_map(|r| r.get(i))).map(|c| display_width(c)).max().unwrap_or(0);
        (w as f32).clamp(4.0, 40.0)
    }).collect()
}

/// 每列的最小宽度（字符数）：该列最长「词」（按空白切）的显示宽度，限制在 2..=12。
/// 没有空格的长中文会封顶，允许在列内换行；数字 / 英文词不会被拦腰拆开
pub fn col_min_chars(header: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    (0..header.len()).map(|i| {
        std::iter::once(&header[i]).chain(rows.iter().filter_map(|r| r.get(i)))
            .flat_map(|c| c.split_whitespace().map(display_width)).max().unwrap_or(0).clamp(2, 12)
    }).collect()
}

pub fn parse_blocks(text: &str) -> Vec<Block> {
    let mut out = vec![];
    let mut para: Vec<&str> = vec![];
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() { out.push(Block::Paragraph(para.join("\n"))); para.clear(); }
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let t = line.trim_start();
        if line.trim().is_empty() { flush(&mut para, &mut out); continue; }
        // 表格：这一行含竖线，下一行是列数相同的分隔行（流式输出时表头 + 分隔行齐了才算）
        if line.contains('|') {
            let header = split_row(line);
            if let Some(aligns) = lines.get(i).and_then(|l| separator_aligns(&split_row(l))).filter(|a| a.len() == header.len()) {
                flush(&mut para, &mut out);
                i += 1;
                let mut rows = vec![];
                while i < lines.len() && !lines[i].trim().is_empty() && lines[i].contains('|') {
                    let mut r = split_row(lines[i]);
                    r.resize(header.len(), String::new());
                    rows.push(r);
                    i += 1;
                }
                out.push(Block::Table { header, aligns, rows });
                continue;
            }
        }
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

    // ---------- 表格 ----------
    // 为什么单独测：模型回答里的表格很常见（/context 的用量表、对比表）。解析错了界面上就是一堆竖线；
    // 流式输出时表格是一行行到的，写到一半不能让整段消息抖成别的样子。

    fn t(h: &[&str], rows: &[&[&str]]) -> Block {
        Block::Table { header: h.iter().map(|x| x.to_string()).collect(), aligns: vec![Align::Left; h.len()], rows: rows.iter().map(|r| r.iter().map(|x| x.to_string()).collect()).collect() }
    }

    #[test]
    fn 基本表格() {
        let b = parse_blocks("| a | b |\n|---|:-:|\n| 1 | 2 |\n| 3 | 4 |");
        assert_eq!(b, vec![Block::Table {
            header: vec!["a".into(), "b".into()],
            aligns: vec![Align::Left, Align::Center],
            rows: vec![vec!["1".into(), "2".into()], vec!["3".into(), "4".into()]],
        }]);
    }

    #[test]
    fn 对齐标记() {
        let Block::Table { aligns, .. } = &parse_blocks("a|b|c|d\n:--|:-:|--:|---\n1|2|3|4")[0] else { panic!() };
        assert_eq!(aligns, &[Align::Left, Align::Center, Align::Right, Align::Left]);
    }

    #[test]
    fn 省略首尾竖线也是表格() {
        assert_eq!(parse_blocks("a | b\n--|--\n1 | 2"), vec![t(&["a", "b"], &[&["1", "2"]])]);
    }

    #[test]
    fn 转义的竖线留在单元格里() {
        assert_eq!(parse_blocks("| a | b |\n|---|---|\n| x \\| y | 2 |"), vec![t(&["a", "b"], &[&["x | y", "2"]])]);
    }

    #[test]
    fn 行内代码里的竖线也按单元格边界处理() {
        // GFM 规定：代码里的竖线也要转义，否则就是分隔。保持和规范一致
        assert_eq!(parse_blocks("| a | b |\n|---|---|\n| `x` | y |"), vec![t(&["a", "b"], &[&["`x`", "y"]])]);
    }

    #[test]
    fn 数据行列数不齐按表头补齐或截断() {
        let b = parse_blocks("| a | b |\n|---|---|\n| 1 |\n| 1 | 2 | 3 |");
        assert_eq!(b, vec![t(&["a", "b"], &[&["1", ""], &["1", "2"]])]);
    }

    #[test]
    fn 空行或不含竖线的行结束表格() {
        let b = parse_blocks("前文\n\n| a |\n|---|\n| 1 |\n\n后文");
        assert_eq!(b, vec![Block::Paragraph("前文".into()), t(&["a"], &[&["1"]]), Block::Paragraph("后文".into())]);
        let b = parse_blocks("| a |\n|---|\n| 1 |\n紧接的文字");
        assert_eq!(b, vec![t(&["a"], &[&["1"]]), Block::Paragraph("紧接的文字".into())]);
    }

    #[test]
    fn 没有分隔行就不是表格() {
        assert_eq!(parse_blocks("a | b"), vec![Block::Paragraph("a | b".into())]);
        assert_eq!(parse_blocks("| a | b |\n| 1 | 2 |"), vec![Block::Paragraph("| a | b |\n| 1 | 2 |".into())]);
        // 分隔行列数和表头不一致：不是表格
        assert_eq!(parse_blocks("| a | b |\n|---|\n| 1 | 2 |"), vec![Block::Paragraph("| a | b |\n|---|\n| 1 | 2 |".into())]);
    }

    #[test]
    fn 流式写到一半() {
        // 只有表头：还是段落
        assert_eq!(parse_blocks("| a | b |"), vec![Block::Paragraph("| a | b |".into())]);
        // 分隔行写到一半（列数不够）：还是段落
        assert_eq!(parse_blocks("| a | b |\n|---"), vec![Block::Paragraph("| a | b |\n|---".into())]);
        // 表头 + 分隔行齐了：已经是（没有数据的）表格
        assert_eq!(parse_blocks("| a | b |\n|---|---|"), vec![t(&["a", "b"], &[])]);
        // 数据行写到一半：补齐
        assert_eq!(parse_blocks("| a | b |\n|---|---|\n| 1 | 2 |\n| 3"), vec![t(&["a", "b"], &[&["1", "2"], &["3", ""]])]);
    }

    #[test]
    fn 每列至少放得下最长的不可拆词() {
        // 数字 / 英文词被拆成两行（~33 / 0）很难看：列的最小宽度 = 该列最长「词」的显示宽度
        let h = vec!["类别".to_string(), "占用".to_string(), "说明".to_string()];
        let rows = vec![vec!["claude.ai sync".to_string(), "~1,280".to_string(), "很长很长很长很长很长很长很长很长".to_string()]];
        let m = col_min_chars(&h, &rows);
        assert_eq!(m[0], 9);  // claude.ai
        assert_eq!(m[1], 6);  // ~1,280
        assert_eq!(m[2], 12); // 没有空格的长中文：封顶，允许在列内换行
        // 再短也留 2 格
        assert_eq!(col_min_chars(&["a".to_string()], &[]), vec![2]);
    }

    #[test]
    fn 列宽权重按最长内容和中文双宽() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("文件ab"), 6);
        let h = vec!["名称".to_string(), "x".to_string()];
        let rows = vec![vec!["很长很长很长的中文单元格".to_string(), "y".to_string()]];
        let w = col_weights(&h, &rows);
        assert!(w[0] > w[1] * 3.0, "{w:?}");
        // 下限：再短的列也留得下几个字
        assert!(w[1] >= 4.0);
        // 上限：超长单元格不会把别的列挤没
        let rows = vec![vec!["a".repeat(500), "b".to_string()]];
        assert!(col_weights(&h, &rows)[0] <= 40.0);
    }
}

//! 多光标的纯逻辑：对若干互不重叠的区间同时做替换，以及「下一个相同内容」的查找。
//! 错了表现为：同时输入时后面的光标位置整体偏移，或 ⌘D 选到已经选过的那一处。
use std::ops::Range;

/// 对已排序、互不重叠的区间批量替换。返回 (覆盖区间, 覆盖区间的新文本, 每处替换之后的光标位置[绝对偏移，基于新文本])。
pub fn apply_edits(text: &str, edits: &[(Range<usize>, String)]) -> (Range<usize>, String, Vec<usize>) {
    let start = edits.first().map(|e| e.0.start).unwrap_or(0);
    let end = edits.last().map(|e| e.0.end).unwrap_or(0);
    let mut out = String::new();
    let mut positions = Vec::with_capacity(edits.len());
    let mut prev = start;
    for (r, with) in edits {
        out.push_str(&text[prev..r.start]);
        out.push_str(with);
        positions.push(start + out.len());
        prev = r.end;
    }
    out.push_str(&text[prev..end]);
    (start..end, out, positions)
}

/// 在 `from` 之后（到末尾再绕回开头）找下一处 `needle`，跳过已被选中的区间。
pub fn next_occurrence(text: &str, needle: &str, from: usize, taken: &[Range<usize>]) -> Option<Range<usize>> {
    if needle.is_empty() { return None; }
    let mut starts: Vec<usize> = vec![];
    let mut i = 0;
    while let Some(p) = text[i..].find(needle) {
        starts.push(i + p);
        i += p + needle.len().max(1);
        if i > text.len() { break; }
    }
    let free = |s: &usize| !taken.iter().any(|t| t.start < s + needle.len() && *s < t.end);
    starts.iter().copied().find(|s| *s >= from && free(s)).or_else(|| starts.iter().copied().find(free)).map(|s| s..s + needle.len())
}

/// 全部相同内容的位置
pub fn all_occurrences(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() { return vec![]; }
    let mut out = vec![];
    let mut i = 0;
    while let Some(p) = text[i..].find(needle) {
        out.push(i + p..i + p + needle.len());
        i += p + needle.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同时替换多处_光标位置正确() {
        let text = "a foo b foo c";
        let edits = vec![(2..5, "XY".to_string()), (8..11, "XY".to_string())];
        let (cover, new, pos) = apply_edits(text, &edits);
        assert_eq!(cover, 2..11);
        assert_eq!(new, "XY b XY");
        // 替换后整段文本是 "a XY b XY c"；两个光标分别在第 4 和第 9 字节
        assert_eq!(pos, vec![4, 9]);
        let mut full = text.to_string();
        full.replace_range(cover, &new);
        assert_eq!(full, "a XY b XY c");
        assert_eq!(&full[..pos[0]], "a XY");
        assert_eq!(&full[..pos[1]], "a XY b XY");
    }

    #[test]
    fn 批量删除与不同长度的替换() {
        let (cover, new, pos) = apply_edits("abcdef", &[(1..2, String::new()), (4..6, "Z".into())]);
        assert_eq!((cover, new.as_str()), (1..6, "cdZ"));
        assert_eq!(pos, vec![1, 4]);
        let (c, n, p) = apply_edits("x", &[]);
        assert_eq!((c, n.as_str(), p.len()), (0..0, "", 0));
    }

    #[test]
    fn 下一处相同内容_跳过已选_绕回开头() {
        let t = "foo bar foo baz foo";
        assert_eq!(next_occurrence(t, "foo", 3, &[0..3]), Some(8..11));
        assert_eq!(next_occurrence(t, "foo", 11, &[0..3, 8..11]), Some(16..19));
        assert_eq!(next_occurrence(t, "foo", 19, &[0..3, 8..11, 16..19]), None);
        assert_eq!(next_occurrence(t, "foo", 17, &[8..11, 16..19]), Some(0..3), "到末尾后绕回开头");
        assert_eq!(next_occurrence(t, "", 0, &[]), None);
    }

    #[test]
    fn 全部相同内容() {
        assert_eq!(all_occurrences("aXbXc", "X"), vec![1..2, 3..4]);
        assert!(all_occurrences("abc", "").is_empty());
        assert_eq!(all_occurrences("aaaa", "aa"), vec![0..2, 2..4]);
    }

    #[test]
    fn 汉字按字节处理() {
        let t = "你好 你好";
        let (cover, new, pos) = apply_edits(t, &[(0..6, "A".into()), (7..13, "A".into())]);
        let mut full = t.to_string();
        full.replace_range(cover, &new);
        assert_eq!(full, "A A");
        assert_eq!(pos, vec![1, 3]);
    }
}

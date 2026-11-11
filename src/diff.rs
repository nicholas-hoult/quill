//! 逐块审阅：把「原文 vs AI 给出的新文本」切成若干改动块，每块可单独接受 / 拒绝。
//! 错了表现为：拒绝一块后其他块的内容错位，或全部拒绝后和原文不一致。
use similar::{ChangeTag, TextDiff};

/// 一个改动块，行号为 0 起的半开区间。
#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    /// 在原文中被替换 / 删除的行
    pub old: std::ops::Range<usize>,
    /// 在新文本中对应的行
    pub new: std::ops::Range<usize>,
}

pub fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() { return vec![]; }
    s.split_inclusive('\n').collect()
}

/// 计算改动块（按行）。
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let diff = TextDiff::from_lines(old, new);
    let mut out: Vec<Hunk> = vec![];
    let (mut oi, mut ni) = (0usize, 0usize);
    let mut cur: Option<Hunk> = None;
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Equal => {
                if let Some(h) = cur.take() { out.push(h); }
                oi += 1; ni += 1;
            }
            ChangeTag::Delete => {
                let h = cur.get_or_insert(Hunk { old: oi..oi, new: ni..ni });
                oi += 1; h.old.end = oi;
            }
            ChangeTag::Insert => {
                let h = cur.get_or_insert(Hunk { old: oi..oi, new: ni..ni });
                ni += 1; h.new.end = ni;
            }
        }
    }
    if let Some(h) = cur { out.push(h); }
    out
}

/// 依据每块的决定拼出最终文本：`accept[i]` 为 true 取新内容，否则保留原文。
pub fn resolve(old: &str, new: &str, accept: &[bool]) -> String {
    let (ol, nl) = (split_lines(old), split_lines(new));
    let hs = hunks(old, new);
    let mut out = String::new();
    let mut oi = 0;
    for (k, h) in hs.iter().enumerate() {
        for l in &ol[oi..h.old.start] { out.push_str(l); }
        if *accept.get(k).unwrap_or(&true) {
            for l in &nl[h.new.clone()] { out.push_str(l); }
        } else {
            for l in &ol[h.old.clone()] { out.push_str(l); }
        }
        oi = h.old.end;
    }
    for l in &ol[oi..] { out.push_str(l); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "a\nb\nc\nd\ne\n";
    const NEW: &str = "a\nB\nc\nd\nE\nf\n";

    #[test]
    fn 找出两个改动块() {
        let h = hunks(OLD, NEW);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0], Hunk { old: 1..2, new: 1..2 });
        assert_eq!(h[1], Hunk { old: 4..5, new: 4..6 });
    }

    #[test]
    fn 全部接受等于新文本_全部拒绝等于原文() {
        assert_eq!(resolve(OLD, NEW, &[true, true]), NEW);
        assert_eq!(resolve(OLD, NEW, &[false, false]), OLD);
    }

    #[test]
    fn 只拒绝其中一块() {
        assert_eq!(resolve(OLD, NEW, &[false, true]), "a\nb\nc\nd\nE\nf\n");
        assert_eq!(resolve(OLD, NEW, &[true, false]), "a\nB\nc\nd\ne\n");
    }

    #[test]
    fn 纯新增与纯删除() {
        assert_eq!(hunks("a\n", "a\nb\n"), vec![Hunk { old: 1..1, new: 1..2 }]);
        assert_eq!(hunks("a\nb\n", "a\n"), vec![Hunk { old: 1..2, new: 1..1 }]);
        assert!(hunks("same\n", "same\n").is_empty());
    }

    #[test]
    fn 没有末尾换行也不丢内容() {
        assert_eq!(resolve("a\nb", "a\nc", &[true]), "a\nc");
        assert_eq!(resolve("a\nb", "a\nc", &[false]), "a\nb");
    }
}

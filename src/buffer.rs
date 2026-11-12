//! 文本缓冲区：rope + 撤销 / 重做。所有偏移都是 UTF-8 字节偏移（和 GPUI 的文本排版一致）。
//! 单独成模块并带测试：偏移换算和撤销合并出错时，表现为光标跳位、撤销把整段话一次吞掉。
use ropey::Rope;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    sel_before: Range<usize>,
    sel_after: Range<usize>,
    at_ms: u64,
    /// 同组的编辑一次撤销 / 重做（多光标连续输入）
    group: u64,
}

pub struct Buffer {
    rope: Rope,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// 每次修改 +1，供高亮缓存判断是否过期
    pub version: u64,
    next_group: u64,
    /// 为 true 时，下一次修改并入上一次修改的撤销组（只生效一次）
    pub join_next: bool,
    /// 自上次取走以来发生的编辑 (起点, 原长度, 新长度)，供高亮区间平移
    pub edit_log: Vec<(usize, usize, usize)>,
}

/// 连续输入合并成一次撤销的时间窗口
const MERGE_MS: u64 = 700;

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self { rope: Rope::from_str(text), undo: vec![], redo: vec![], version: 0, next_group: 0, join_next: false, edit_log: vec![] }
    }

    /// O(1) 快照（rope 是写时复制的树，克隆不复制文本）
    pub fn snapshot(&self) -> Rope { self.rope.clone() }

    pub fn len(&self) -> usize { self.rope.len_bytes() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn text(&self) -> String { self.rope.to_string() }

    pub fn slice(&self, r: Range<usize>) -> String {
        let r = self.clamp(r);
        self.rope.byte_slice(r).to_string()
    }

    fn clamp(&self, r: Range<usize>) -> Range<usize> {
        let l = self.len();
        r.start.min(l)..r.end.min(l)
    }

    /// 行数。末尾有换行时，最后会多出一个空行（和常见编辑器一致）。
    pub fn line_count(&self) -> usize { self.rope.len_lines() }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_byte(line.min(self.line_count() - 1))
    }

    /// 行尾（不含换行符）
    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.line_count() - 1);
        let start = self.rope.line_to_byte(line);
        let s = self.rope.line(line);
        let mut n = s.len_bytes();
        if n > 0 && s.byte(n - 1) == b'\n' { n -= 1; }
        if n > 0 && s.byte(n - 1) == b'\r' { n -= 1; }
        start + n
    }

    pub fn line_text(&self, line: usize) -> String {
        self.slice(self.line_start(line)..self.line_end(line))
    }

    pub fn line_of(&self, offset: usize) -> usize { self.rope.byte_to_line(offset.min(self.len())) }

    pub fn col_of(&self, offset: usize) -> usize { offset.min(self.len()) - self.line_start(self.line_of(offset)) }

    pub fn prev_boundary(&self, offset: usize) -> usize {
        if offset == 0 { return 0; }
        let line = self.line_of(offset);
        let start = self.line_start(line);
        if offset == start { return self.line_end(line - 1); }
        let text = self.line_text(line);
        let rel = offset - start;
        text.grapheme_indices(true).map(|(i, _)| i).filter(|&i| i < rel).last().map(|i| start + i).unwrap_or(start)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        if offset >= self.len() { return self.len(); }
        let line = self.line_of(offset);
        let end = self.line_end(line);
        if offset >= end { return self.line_start(line + 1).min(self.len()); }
        let text = self.line_text(line);
        let start = self.line_start(line);
        let rel = offset - start;
        text.grapheme_indices(true).map(|(i, g)| i + g.len()).find(|&i| i > rel).map(|i| start + i).unwrap_or(end)
    }

    fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

    /// 向左跳一个单词（⌥←）
    pub fn word_left(&self, offset: usize) -> usize {
        let text = self.slice(0..offset);
        let mut it = text.char_indices().rev().peekable();
        let mut pos = offset;
        while let Some(&(i, c)) = it.peek() { if c.is_whitespace() { pos = i; it.next(); } else { break; } }
        if let Some(&(_, c)) = it.peek() {
            let w = Self::is_word(c);
            while let Some(&(i, c)) = it.peek() { if !c.is_whitespace() && Self::is_word(c) == w { pos = i; it.next(); } else { break; } }
        }
        pos
    }

    /// 向右跳一个单词（⌥→）
    pub fn word_right(&self, offset: usize) -> usize {
        let text = self.slice(offset..self.len());
        let mut it = text.char_indices().peekable();
        let mut pos = offset;
        while let Some(&(i, c)) = it.peek() { if c.is_whitespace() { pos = offset + i + c.len_utf8(); it.next(); } else { break; } }
        if let Some(&(_, c)) = it.peek() {
            let w = Self::is_word(c);
            while let Some(&(i, c)) = it.peek() { if !c.is_whitespace() && Self::is_word(c) == w { pos = offset + i + c.len_utf8(); it.next(); } else { break; } }
        }
        pos
    }

    /// 双击选词的范围
    pub fn word_range(&self, offset: usize) -> Range<usize> {
        let line = self.line_of(offset);
        let (s, e) = (self.line_start(line), self.line_end(line));
        let text = self.slice(s..e);
        let rel = offset - s;
        let mut start = rel;
        let mut end = rel;
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let at = chars.iter().position(|&(i, _)| i >= rel).unwrap_or(chars.len());
        let cls = |c: char| if Self::is_word(c) { 1 } else if c.is_whitespace() { 2 } else { 3 };
        if let Some(&(_, c0)) = chars.get(at).or(chars.last()) {
            let k = cls(c0);
            let mut a = at.min(chars.len().saturating_sub(1));
            while a > 0 && cls(chars[a - 1].1) == k { a -= 1; }
            let mut b = at.min(chars.len().saturating_sub(1));
            while b < chars.len() && cls(chars[b].1) == k { b += 1; }
            start = chars[a].0;
            end = chars.get(b).map(|c| c.0).unwrap_or(text.len());
        }
        s + start..s + end
    }

    /// 修改文本。`sel_before/after` 记录选区，用于撤销时还原；`now_ms` 用于合并连续输入。
    pub fn replace(&mut self, range: Range<usize>, text: &str, sel_before: Range<usize>, sel_after: Range<usize>, now_ms: u64) {
        let range = self.clamp(range);
        let removed = self.slice(range.clone());
        if removed.is_empty() && text.is_empty() { return; }
        let (cs, ce) = (self.rope.byte_to_char(range.start), self.rope.byte_to_char(range.end));
        if ce > cs { self.rope.remove(cs..ce); }
        if !text.is_empty() { self.rope.insert(cs, text); }
        self.version += 1;
        self.edit_log.push((range.start, removed.len(), text.len()));
        self.redo.clear();
        // 合并：上一次是插入，这次紧接着在末尾继续插入单个非换行字符
        if let Some(last) = self.undo.last_mut() {
            let contiguous = removed.is_empty()
                && last.removed.is_empty()
                && range.start == last.start + last.inserted.len()
                && now_ms.saturating_sub(last.at_ms) < MERGE_MS
                && !text.contains('\n')
                && !last.inserted.ends_with('\n');
            if contiguous {
                last.inserted.push_str(text);
                last.sel_after = sel_after;
                last.at_ms = now_ms;
                return;
            }
        }
        let group = if std::mem::take(&mut self.join_next) { self.undo.last().map(|e| e.group) } else { None }.unwrap_or_else(|| { self.next_group += 1; self.next_group });
        self.undo.push(Edit { start: range.start, removed, inserted: text.to_string(), sel_before, sel_after, at_ms: now_ms, group });
    }

    fn apply_inverse(&mut self, e: &Edit) {
        let (cs, ce) = (self.rope.byte_to_char(e.start), self.rope.byte_to_char(e.start + e.inserted.len()));
        if ce > cs { self.rope.remove(cs..ce); }
        if !e.removed.is_empty() { self.rope.insert(cs, &e.removed); }
        self.version += 1;
        self.edit_log.push((e.start, e.inserted.len(), e.removed.len()));
    }

    /// 撤销，返回应恢复的选区
    pub fn undo(&mut self) -> Option<Range<usize>> {
        let e = self.undo.pop()?;
        let g = e.group;
        self.apply_inverse(&e);
        let mut sel = e.sel_before.clone();
        self.redo.push(e);
        while self.undo.last().map(|t| t.group == g).unwrap_or(false) {
            let e = self.undo.pop().unwrap();
            self.apply_inverse(&e);
            sel = e.sel_before.clone();
            self.redo.push(e);
        }
        Some(sel)
    }

    fn apply_forward(&mut self, e: &Edit) {
        let (cs, ce) = (self.rope.byte_to_char(e.start), self.rope.byte_to_char(e.start + e.removed.len()));
        if ce > cs { self.rope.remove(cs..ce); }
        if !e.inserted.is_empty() { self.rope.insert(cs, &e.inserted); }
        self.version += 1;
        self.edit_log.push((e.start, e.removed.len(), e.inserted.len()));
    }

    pub fn redo(&mut self) -> Option<Range<usize>> {
        let e = self.redo.pop()?;
        let g = e.group;
        self.apply_forward(&e);
        let mut sel = e.sel_after.clone();
        self.undo.push(e);
        while self.redo.last().map(|t| t.group == g).unwrap_or(false) {
            let e = self.redo.pop().unwrap();
            self.apply_forward(&e);
            sel = e.sel_after.clone();
            self.undo.push(e);
        }
        Some(sel)
    }

    /// 整体替换（AI 改动落地用），作为一次可撤销的修改。
    pub fn replace_all(&mut self, text: &str, now_ms: u64) {
        let n = self.len();
        self.replace(0..n, text, 0..0, 0..0, now_ms + MERGE_MS * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 行与偏移换算() {
        let b = Buffer::new("ab\n你好\n\nend");
        assert_eq!(b.line_count(), 4);
        assert_eq!((b.line_start(1), b.line_end(1)), (3, 9));
        assert_eq!(b.line_text(2), "");
        assert_eq!(b.line_of(4), 1);
        assert_eq!(b.col_of(6), 3);
        assert_eq!(b.line_text(3), "end");
    }

    #[test]
    fn crlf_行尾不含回车() {
        let b = Buffer::new("a\r\nb");
        assert_eq!(b.line_text(0), "a");
    }

    #[test]
    fn 光标按字素移动_跨行() {
        let b = Buffer::new("a你\nb");
        assert_eq!(b.next_boundary(0), 1);
        assert_eq!(b.next_boundary(1), 4);
        assert_eq!(b.next_boundary(4), 5); // 行尾 → 下一行行首
        assert_eq!(b.prev_boundary(5), 4); // 行首 → 上一行行尾
        assert_eq!(b.prev_boundary(4), 1);
        assert_eq!(b.prev_boundary(0), 0);
        assert_eq!(b.next_boundary(6), 6);
    }

    #[test]
    fn 撤销重做与选区还原() {
        let mut b = Buffer::new("hello");
        b.replace(5..5, "!", 5..5, 6..6, 0);
        assert_eq!(b.text(), "hello!");
        assert_eq!(b.undo(), Some(5..5));
        assert_eq!(b.text(), "hello");
        assert_eq!(b.redo(), Some(6..6));
        assert_eq!(b.text(), "hello!");
        assert_eq!(b.undo(), Some(5..5));
        b.replace(0..1, "J", 0..1, 1..1, 5000);
        assert!(b.redo().is_none(), "新修改应清空重做栈");
    }

    #[test]
    fn 连续输入合并为一次撤销() {
        let mut b = Buffer::new("");
        for (i, c) in "abc".chars().enumerate() { b.replace(i..i, &c.to_string(), i..i, i + 1..i + 1, 100 + i as u64 * 50); }
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "", "三个字符应一次撤销");
        // 间隔过久不合并
        b.replace(0..0, "x", 0..0, 1..1, 10_000);
        b.replace(1..1, "y", 1..1, 2..2, 20_000);
        b.undo();
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn 换行不合并() {
        let mut b = Buffer::new("");
        b.replace(0..0, "a", 0..0, 1..1, 0);
        b.replace(1..1, "\n", 1..1, 2..2, 10);
        b.replace(2..2, "b", 2..2, 3..3, 20);
        b.undo();
        assert_eq!(b.text(), "a\n");
    }

    #[test]
    fn 单词跳转() {
        let b = Buffer::new("foo_bar  baz.qux");
        assert_eq!(b.word_right(0), 7);
        assert_eq!(b.word_right(7), 12); // 跳过空格 + baz
        assert_eq!(b.word_left(16), 13);
        assert_eq!(b.word_left(7), 0);
    }

    #[test]
    fn 双击选词() {
        let b = Buffer::new("let foo_bar = 1;");
        assert_eq!(b.word_range(6), 4..11);
        assert_eq!(b.word_range(11), 11..12); // 落在空格上选空白
    }

    #[test]
    fn 同组编辑一次撤销一次重做() {
        let mut b = Buffer::new("ab");
        b.replace(0..1, "X", 0..0, 1..1, 5000);
        b.join_next = true;
        b.replace(1..2, "Y", 0..0, 2..2, 5001);
        b.replace(2..2, "Z", 0..0, 3..3, 99_999); // 不同组
        assert_eq!(b.text(), "XYZ");
        b.undo();
        assert_eq!(b.text(), "XY");
        b.undo();
        assert_eq!(b.text(), "ab", "X、Y 同组，应一起撤销");
        b.redo();
        assert_eq!(b.text(), "XY", "重做也成组");
        b.redo();
        assert_eq!(b.text(), "XYZ");
    }

    #[test]
    fn 整体替换可撤销() {
        let mut b = Buffer::new("old");
        b.replace_all("new text", 0);
        assert_eq!(b.text(), "new text");
        b.undo();
        assert_eq!(b.text(), "old");
    }
}

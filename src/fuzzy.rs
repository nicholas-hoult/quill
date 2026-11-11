//! 子序列模糊匹配。Cmd+P 的排序决定「想要的文件是不是第一个」，
//! 错了表现为输入 `app` 却先看到深层目录里的 snap.ts。

/// 返回分数（越大越好）和命中的字符位置（字符下标，用于高亮），不匹配返回 None。
pub fn fuzzy_match(query: &str, target: &str) -> Option<(f32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0.0, vec![]));
    }
    let t: Vec<char> = target.chars().flat_map(|c| c.to_lowercase()).collect();
    if t.len() != target.chars().count() {
        // 极少数字符小写化会变长，退化为按原字符比较
        return fuzzy_match_plain(query, target);
    }
    let base = t.iter().rposition(|&c| c == '/').map(|i| i + 1).unwrap_or(0);
    let mut ti = 0usize;
    let mut score = 0f32;
    let mut prev: isize = -2;
    let mut hits = vec![];
    for qc in query.chars().flat_map(|c| c.to_lowercase()) {
        let found = (ti..t.len()).find(|&i| t[i] == qc)?;
        score += 1.0;
        if found as isize == prev + 1 { score += 3.0; }
        if found == 0 || "/._- ".contains(t[found - 1]) { score += 4.0; }
        if found >= base { score += 2.0; }
        prev = found as isize;
        ti = found + 1;
        hits.push(found);
    }
    let name: String = t[base..].iter().collect();
    let q = query.to_lowercase();
    if name.starts_with(&q) { score += 12.0; } else if name.contains(&q) { score += 6.0; }
    Some((score - t.len() as f32 * 0.02, hits))
}

fn fuzzy_match_plain(query: &str, target: &str) -> Option<(f32, Vec<usize>)> {
    let t: Vec<char> = target.chars().collect();
    let mut ti = 0;
    let mut hits = vec![];
    for qc in query.chars() {
        let f = (ti..t.len()).find(|&i| t[i] == qc)?;
        hits.push(f);
        ti = f + 1;
    }
    Some((hits.len() as f32, hits))
}

pub fn fuzzy_filter<'a, T>(items: &'a [T], query: &str, key: impl Fn(&T) -> &str, limit: usize) -> Vec<(&'a T, Vec<usize>)> {
    if query.is_empty() {
        return items.iter().take(limit).map(|x| (x, vec![])).collect();
    }
    let mut scored: Vec<(f32, &T, Vec<usize>)> = items.iter().filter_map(|it| fuzzy_match(query, key(it)).map(|(s, h)| (s, it, h))).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, it, h)| (it, h)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不匹配与空查询() {
        assert!(fuzzy_match("xyz", "src/app.ts").is_none());
        assert_eq!(fuzzy_match("", "a").unwrap().0, 0.0);
    }

    #[test]
    fn 文件名前缀优先于深层子序列() {
        let items = vec!["src/deep/snap.ts", "src/App.tsx", "README.md"];
        let r = fuzzy_filter(&items, "app", |x| x, 10);
        assert_eq!(*r[0].0, "src/App.tsx");
    }

    #[test]
    fn 连续匹配分数更高() {
        let a = fuzzy_match("edit", "editor.rs").unwrap().0;
        let b = fuzzy_match("edit", "e_d_i_t.rs").unwrap().0;
        assert!(a > b);
    }

    #[test]
    fn 返回命中位置用于高亮() {
        let (_, hits) = fuzzy_match("ap", "src/app.rs").unwrap();
        assert_eq!(hits, vec![4, 5]);
    }

    #[test]
    fn 中文也能匹配() {
        assert!(fuzzy_match("说明", "docs/说明.md").is_some());
    }
}

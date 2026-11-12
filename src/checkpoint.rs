//! Agent 检查点（#5）：记录「Agent 改动某个文件之前它是什么样」，并算出回退到某条消息之前要还原哪些文件。
//! 错了表现为：回退后文件没回到原样（漏了某一轮改的文件）、回退成一个从没出现过的混合状态、
//! 把原本就存在的文件当成「新建」删掉，或超大文件把内存吃光。

/// 单个文件的改动前内容超过这个大小就不记（回退时这个文件无法还原）
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// 相对项目根的路径
    pub path: String,
    /// 改动前的内容；`None` 表示这个文件是 Agent 新建的（之前不存在）
    pub before: Option<String>,
}

/// 回退时对一个文件要做的事
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// 把内容还原成这份文本
    Restore(String),
    /// 文件是 Agent 新建的，删除它
    Delete,
}

fn norm(path: &str) -> String {
    path.trim().trim_start_matches("./").trim_start_matches('/').to_string()
}

/// 记录一次改动前的内容。同一轮里同一个文件只记第一次（后面看到的已经是 Agent 改过的内容了）。
pub fn record(snaps: &mut Vec<Snapshot>, path: &str, before: Option<String>) {
    let path = norm(path);
    if path.is_empty() || snaps.iter().any(|s| s.path == path) { return; }
    if before.as_ref().map(|t| t.len() > MAX_BYTES).unwrap_or(false) { return; }
    snaps.push(Snapshot { path, before });
}

/// 回退到「第一轮之前」要做的事：`turns` 按时间顺序，每个路径取最早的那份改动前内容。
/// 结果按路径排序，方便展示和测试。
pub fn plan(turns: &[&[Snapshot]]) -> Vec<(String, Action)> {
    let mut out: Vec<(String, Action)> = vec![];
    for turn in turns {
        for s in turn.iter() {
            if out.iter().any(|(p, _)| *p == s.path) { continue; }
            out.push((s.path.clone(), match &s.before { Some(t) => Action::Restore(t.clone()), None => Action::Delete }));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(p: &str, b: Option<&str>) -> Snapshot { Snapshot { path: p.into(), before: b.map(String::from) } }

    #[test]
    fn 同一文件本轮只记第一次() {
        let mut v = vec![];
        record(&mut v, "src/a.rs", Some("原始".into()));
        record(&mut v, "src/a.rs", Some("Agent 改过的".into()));
        assert_eq!(v, [snap("src/a.rs", Some("原始"))]);
    }

    #[test]
    fn 路径规范化_相同文件的不同写法算同一个() {
        let mut v = vec![];
        record(&mut v, "./src/a.rs", Some("x".into()));
        record(&mut v, "/src/a.rs", Some("y".into()));
        record(&mut v, " src/a.rs ", Some("z".into()));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "src/a.rs");
    }

    #[test]
    fn 空路径忽略_新建文件记为_none() {
        let mut v = vec![];
        record(&mut v, "  ", Some("x".into()));
        record(&mut v, "new.rs", None);
        assert_eq!(v, [snap("new.rs", None)]);
    }

    #[test]
    fn 超过大小上限不记() {
        let mut v = vec![];
        record(&mut v, "big.bin", Some("x".repeat(MAX_BYTES + 1)));
        assert!(v.is_empty());
        record(&mut v, "ok.txt", Some("x".repeat(MAX_BYTES)));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn 跨轮合并_每个路径取最早的改动前内容() {
        let t1 = [snap("a.rs", Some("a0")), snap("new.rs", None)];
        let t2 = [snap("a.rs", Some("a1")), snap("b.rs", Some("b0"))];
        let p = plan(&[&t1, &t2]);
        assert_eq!(p, [
            ("a.rs".to_string(), Action::Restore("a0".into())),
            ("b.rs".to_string(), Action::Restore("b0".into())),
            ("new.rs".to_string(), Action::Delete),
        ]);
    }

    #[test]
    fn 只回退后面几轮_不影响更早的轮次() {
        let t1 = [snap("a.rs", Some("a0"))];
        let t2 = [snap("a.rs", Some("a1"))];
        // 回退到第 2 轮之前：只传第 2 轮起的快照
        assert_eq!(plan(&[&t2]), [("a.rs".to_string(), Action::Restore("a1".into()))]);
        assert_eq!(plan(&[&t1, &t2]), [("a.rs".to_string(), Action::Restore("a0".into()))]);
    }

    #[test]
    fn 没有快照就没有计划() {
        assert!(plan(&[]).is_empty());
        assert!(plan(&[&[]]).is_empty());
    }

    #[test]
    fn 文件先新建后被改_仍是删除() {
        // 第 1 轮新建了 n.rs，第 2 轮又改了它（第 2 轮的 before 是第 1 轮的产物）：回退到第 1 轮之前应删除
        let t1 = [snap("n.rs", None)];
        let t2 = [snap("n.rs", Some("第一轮写的"))];
        assert_eq!(plan(&[&t1, &t2]), [("n.rs".to_string(), Action::Delete)]);
    }
}

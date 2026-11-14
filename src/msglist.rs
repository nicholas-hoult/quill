//! 对话消息列表的虚拟化同步策略（纯函数）。
//!
//! 为什么单独测：面板用 GPUI 的 `ListState` 只渲染可见消息，`ListState` 自己不知道消息数据变了——
//! 必须由我们告诉它「第几项被换了 / 多了几项」。告错的后果在界面上是：切换对话后显示上一个对话的高度
//! （消息重叠或留白）、发新消息后列表没有增长而新消息不可见。
//!
//! 数据变化只有三种，用一个 `Key`（当前对话序号、消息数、首尾消息 id）区分。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    /// 当前对话在列表里的序号
    pub thread: usize,
    pub len: usize,
    pub first: u64,
    pub last: u64,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Sync {
    /// 什么都没变
    Keep,
    /// 同一个对话末尾追加了 n 项：保留已量好的高度和滚动位置
    Append(usize),
    /// 换了对话 / 消息被删改 / 首次：全部作废重来
    Reset,
}

/// `old` 为 `None` 表示还没同步过
pub fn plan(old: Option<Key>, new: Key) -> Sync {
    let Some(o) = old else { return Sync::Reset };
    if o == new { return Sync::Keep; }
    // 追加：同一对话、首条没变、变多了。旧的最后一条的 id 仍然是旧序列的最后一条——这里无法验证中间没被改，
    // 但消息只会追加或整体替换（切换对话），不会在中间插入
    if o.thread == new.thread && o.first == new.first && new.len > o.len && o.len > 0 { return Sync::Append(new.len - o.len); }
    // 同一个对话里消息 id 变了但数量不变（最后一条被重新生成等）也要全部作废
    Sync::Reset
}

/// 流式片段触发重绘的最小间隔：每个片段都重排整条长消息会把一个核心吃满（实测 20KB 的回答每帧 ~15ms，
/// 66 片段 / 秒时 CPU 110%），合并到 40ms 一次（25 帧 / 秒）肉眼无差别
pub const STREAM_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(40);

/// 流式重绘间隔的上限（再长的消息文字也至少每 400ms 动一次）
pub const STREAM_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// 流式重绘的最小间隔随消息长度放大：单帧要把整条消息重新布局（实测约 1.5µs / 字符，20KB ≈ 30ms），
/// 固定 40ms 间隔对长消息等于背靠背渲染、仍把一个核心吃满。按「最多占 30% CPU」取 5µs / 字符，夹在 40ms..400ms
pub fn stream_interval(chars: usize) -> std::time::Duration {
    std::time::Duration::from_micros((chars as u64).saturating_mul(5)).clamp(STREAM_MIN_INTERVAL, STREAM_MAX_INTERVAL)
}

/// 流式片段到了，现在该不该重绘：返回还要等多久。`Duration::ZERO` = 立刻重绘；
/// 否则调用方安排一次延迟重绘（已经安排过就不再安排：最后一个片段一定会被补刷）
pub fn stream_wait(last_paint: Option<std::time::Instant>, now: std::time::Instant, min: std::time::Duration) -> std::time::Duration {
    match last_paint {
        None => std::time::Duration::ZERO,
        Some(t) => min.saturating_sub(now.saturating_duration_since(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    // 为什么单独测：节流判错了，要么长回答照样卡（该等没等），要么最后一个片段不显示 / 流式时界面一顿一顿（等过头）

    #[test]
    fn 间隔随消息长度放大_短消息保持下限() {
        // 实测单帧约 1.5µs / 字符（20KB 的消息 ~30ms），按「重绘最多占 30% CPU」标定：间隔 ≈ 5µs / 字符
        assert_eq!(stream_interval(0), STREAM_MIN_INTERVAL);
        assert_eq!(stream_interval(2_000), STREAM_MIN_INTERVAL); // 10ms < 下限
        assert_eq!(stream_interval(8_000), STREAM_MIN_INTERVAL); // 刚好 40ms
        assert_eq!(stream_interval(20_000), Duration::from_millis(100));
        assert_eq!(stream_interval(60_000), Duration::from_millis(300));
        // 封顶：再长也不会让文字几秒才动一下
        assert_eq!(stream_interval(10_000_000), STREAM_MAX_INTERVAL);
    }

    #[test]
    fn 第一个片段立刻重绘() {
        assert_eq!(stream_wait(None, Instant::now(), STREAM_MIN_INTERVAL), Duration::ZERO);
    }

    #[test]
    fn 刚重绘过就要等到满间隔() {
        let t0 = Instant::now();
        assert_eq!(stream_wait(Some(t0), t0 + Duration::from_millis(10), STREAM_MIN_INTERVAL), Duration::from_millis(30));
        assert_eq!(stream_wait(Some(t0), t0, STREAM_MIN_INTERVAL), STREAM_MIN_INTERVAL);
    }

    #[test]
    fn 过了间隔就立刻重绘() {
        let t0 = Instant::now();
        assert_eq!(stream_wait(Some(t0), t0 + STREAM_MIN_INTERVAL, STREAM_MIN_INTERVAL), Duration::ZERO);
        assert_eq!(stream_wait(Some(t0), t0 + Duration::from_secs(3), STREAM_MIN_INTERVAL), Duration::ZERO);
    }

    #[test]
    fn 时钟倒退不会算出负数或溢出() {
        let t0 = Instant::now();
        assert_eq!(stream_wait(Some(t0 + Duration::from_millis(5)), t0, STREAM_MIN_INTERVAL), STREAM_MIN_INTERVAL);
    }

    fn k(thread: usize, len: usize, first: u64, last: u64) -> Key { Key { thread, len, first, last } }

    #[test]
    fn 首次同步要重置() { assert_eq!(plan(None, k(0, 3, 1, 3)), Sync::Reset); }

    #[test]
    fn 没变就保持() { assert_eq!(plan(Some(k(0, 3, 1, 3)), k(0, 3, 1, 3)), Sync::Keep); }

    #[test]
    fn 同一对话追加消息只增不重置() {
        // 发送一条消息会同时追加用户消息和助手消息：已量好的高度和滚动位置要保留
        assert_eq!(plan(Some(k(0, 4, 1, 4)), k(0, 6, 1, 6)), Sync::Append(2));
    }

    #[test]
    fn 切换对话要重置() {
        // 序号不同；即使消息数一样也不能沿用旧的高度
        assert_eq!(plan(Some(k(0, 4, 1, 4)), k(1, 4, 9, 12)), Sync::Reset);
    }

    #[test]
    fn 删除对话后序号相同但内容不同要重置() {
        // 删掉前一个对话，后一个顶上来占了同一个序号、同样的消息数
        assert_eq!(plan(Some(k(1, 4, 1, 4)), k(1, 4, 9, 12)), Sync::Reset);
    }

    #[test]
    fn 消息变少要重置() { assert_eq!(plan(Some(k(0, 6, 1, 6)), k(0, 4, 1, 4)), Sync::Reset); }

    #[test]
    fn 空对话变有内容要重置() {
        // 从 0 条到 2 条：旧的没有可保留的高度，按重置处理也最稳
        assert_eq!(plan(Some(k(0, 0, 0, 0)), k(0, 2, 1, 2)), Sync::Reset);
    }

    #[test]
    fn 最后一条换了但数量不变要重置() { assert_eq!(plan(Some(k(0, 4, 1, 4)), k(0, 4, 1, 7)), Sync::Reset); }
}

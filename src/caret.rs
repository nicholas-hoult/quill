//! 输入光标的外观参数（纯逻辑，便于测试）：闪烁时的渐隐渐现曲线、光标的几何尺寸。
//! 错了表现为：光标生硬地一闪一灭、比文字高出一大截、或打字时光标半透明（该立刻满亮）。

use std::time::Duration;

/// 一次渐变（亮→灭 或 灭→亮）持续多久
pub const FADE: Duration = Duration::from_millis(160);

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 光标当前的不透明度。`on` 是这一轮闪烁要显示还是隐藏，`since` 是距离上一次切换过去了多久。
/// 切换后的前 `FADE` 时间里平滑过渡；过了就稳定在 1 或 0。
/// 打字 / 移动光标会把「距离切换的时间」置为很大且 `on = true`，所以光标立刻满亮、不会半透明。
pub fn alpha(on: bool, since: Duration) -> f32 {
    let p = smoothstep(since.as_secs_f32() / FADE.as_secs_f32());
    if on { p } else { 1.0 - p }
}

/// 过渡是否还没结束（需要继续请求下一帧）
pub fn animating(since: Duration) -> bool { since < FADE }

/// 光标的尺寸 (宽, 高, 距行顶的偏移)：比整行矮一点、按字号取高度并在行内垂直居中，
/// 不再顶满整行（顶满显得又粗又笨）。
pub fn geometry(font_size: f32, line_height: f32) -> (f32, f32, f32) {
    let h = (font_size * 1.3).clamp(10.0, (line_height - 4.0).max(10.0));
    (1.5, h, (line_height - h) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration { Duration::from_millis(n) }

    #[test]
    fn 亮起是平滑的_从零到一() {
        assert_eq!(alpha(true, ms(0)), 0.0);
        let mid = alpha(true, ms(80));
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert_eq!(alpha(true, FADE), 1.0);
        assert_eq!(alpha(true, ms(5000)), 1.0);
    }

    #[test]
    fn 熄灭也是平滑的_从一到零() {
        assert_eq!(alpha(false, ms(0)), 1.0);
        let mid = alpha(false, ms(80));
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert_eq!(alpha(false, FADE), 0.0);
        assert_eq!(alpha(false, ms(5000)), 0.0);
    }

    #[test]
    fn 渐变单调且在_0_到_1_之间() {
        let mut prev = -1.0;
        for t in 0..=200 {
            let a = alpha(true, ms(t));
            assert!((0.0..=1.0).contains(&a));
            assert!(a >= prev, "亮起应单调不减");
            prev = a;
        }
    }

    #[test]
    fn 打字时立刻满亮() {
        // 打字会把距离切换的时间设得很大且处于「亮」
        assert_eq!(alpha(true, Duration::from_secs(10)), 1.0);
        assert!(!animating(Duration::from_secs(10)));
        assert!(animating(ms(10)));
    }

    #[test]
    fn 光标比整行矮且垂直居中() {
        let (w, h, dy) = geometry(13.0, 21.0);
        assert_eq!(w, 1.5);
        assert!(h < 21.0 - 3.0, "不能顶满整行：{h}");
        assert!(h > 13.0, "不能比字号还矮：{h}");
        assert!((dy * 2.0 + h - 21.0).abs() < 1e-4, "应当垂直居中");
        // 大行高下不会超过 字号×1.3
        let (_, h2, _) = geometry(13.0, 40.0);
        assert!((h2 - 16.9).abs() < 1e-3);
        // 极小行高下高度也有下限，不会缩成一个点
        let (_, h3, _) = geometry(13.0, 8.0);
        assert_eq!(h3, 10.0);
    }
}

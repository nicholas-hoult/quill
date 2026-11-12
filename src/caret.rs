//! 输入光标的外观与闪烁时序（纯逻辑，便于测试），对标 VS Code 的 `cursorBlinking: smooth` 和 IDEA 的光标：
//! 中性色、2px、笔直、高度就是一整行；闪烁一个周期 1 秒——亮住 200ms → 缓缓淡出 400ms → 暗住 400ms → 立刻亮起。
//! 打字 / 移动 / 刚获得焦点都会把周期重新从头算，所以光标总是先满亮。
//! 错了表现为：光标生硬地一闪一灭、打字时光标半透明、或者空闲时还在疯狂重绘。

use std::time::Duration;

/// 闪烁周期
pub const PERIOD: Duration = Duration::from_millis(1000);
const HOLD_ON: u64 = 200; // 0..200 满亮
const FADE_END: u64 = 600; // 200..600 淡出
// 600..1000 暗住，到 1000 立刻回到满亮

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 距离上次重置过去了 `since`，换算成周期内的相位
pub fn phase(since: Duration) -> Duration {
    Duration::from_millis((since.as_millis() % PERIOD.as_millis()) as u64)
}

/// 周期内某个相位的不透明度
pub fn alpha(phase: Duration) -> f32 {
    let t = phase.as_millis() as u64;
    if t <= HOLD_ON { 1.0 }
    else if t < FADE_END { 1.0 - smoothstep((t - HOLD_ON) as f32 / (FADE_END - HOLD_ON) as f32) }
    else { 0.0 }
}

/// 是否正处在淡出阶段（这段时间需要连续请求下一帧；其余阶段画面不变，不必重绘）
pub fn fading(phase: Duration) -> bool {
    let t = phase.as_millis() as u64;
    (HOLD_ON..FADE_END).contains(&t)
}

/// 距离下一次「画面需要变化」的时刻还有多久（亮住结束 / 淡出结束 / 周期结束）。
/// 空闲时靠它精确唤醒，而不是每 16ms 都重绘。
pub fn until_next_change(phase: Duration) -> Duration {
    let t = phase.as_millis() as u64;
    let next = if t < HOLD_ON { HOLD_ON } else if t < FADE_END { FADE_END } else { PERIOD.as_millis() as u64 };
    Duration::from_millis(next - t)
}

/// 光标尺寸 (宽, 高, 距行顶偏移)：2px 宽，高度取一整行（上下各让半个像素，避免压到相邻行的背景）。
pub fn geometry(line_height: f32) -> (f32, f32, f32) {
    ((2.0), (line_height - 1.0).max(4.0), 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration { Duration::from_millis(n) }

    #[test]
    fn 周期开头先满亮_打字后不会半透明() {
        assert_eq!(alpha(ms(0)), 1.0);
        assert_eq!(alpha(ms(199)), 1.0);
        assert_eq!(alpha(ms(200)), 1.0);
    }

    #[test]
    fn 淡出平滑且单调_中点约一半() {
        let mid = alpha(ms(400));
        assert!(mid > 0.4 && mid < 0.6, "{mid}");
        let mut prev = 1.0;
        for t in 200..=600 {
            let a = alpha(ms(t));
            assert!((0.0..=1.0).contains(&a));
            assert!(a <= prev + 1e-6, "淡出应单调不增：{t}ms {a} > {prev}");
            prev = a;
        }
        assert_eq!(alpha(ms(600)), 0.0);
    }

    #[test]
    fn 暗住一段后立刻亮起() {
        assert_eq!(alpha(ms(601)), 0.0);
        assert_eq!(alpha(ms(999)), 0.0);
        // 下一个周期开头
        assert_eq!(alpha(phase(ms(1000))), 1.0);
        assert_eq!(alpha(phase(ms(2150))), 1.0);
    }

    #[test]
    fn 相位取模() {
        assert_eq!(phase(ms(0)), ms(0));
        assert_eq!(phase(ms(1234)), ms(234));
        assert_eq!(phase(Duration::from_secs(10)), ms(0));
    }

    #[test]
    fn 只有淡出阶段需要连续重绘() {
        assert!(!fading(ms(0)));
        assert!(!fading(ms(199)));
        assert!(fading(ms(200)));
        assert!(fading(ms(599)));
        assert!(!fading(ms(600)));
        assert!(!fading(ms(900)));
    }

    #[test]
    fn 空闲时精确唤醒() {
        assert_eq!(until_next_change(ms(0)), ms(200));
        assert_eq!(until_next_change(ms(150)), ms(50));
        assert_eq!(until_next_change(ms(300)), ms(300)); // 淡出中：到 600 结束
        assert_eq!(until_next_change(ms(600)), ms(400)); // 暗住：到 1000 亮起
        assert_eq!(until_next_change(ms(950)), ms(50));
    }

    #[test]
    fn 光标几何_两像素宽_整行高() {
        let (w, h, dy) = geometry(21.0);
        assert_eq!(w, 2.0);
        assert_eq!(h, 20.0);
        assert_eq!(dy, 0.5);
        let (_, h2, _) = geometry(22.0);
        assert_eq!(h2, 21.0);
        // 极小行高不会出现 0 或负数
        assert!(geometry(1.0).1 >= 4.0);
    }
}

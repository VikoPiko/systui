//! Text micro-graphics with ASCII fallback.

use super::theme::Theme;
use ratatui::style::Style;

/// Horizontal bar, `frac` in 0..=1 (clamped).
pub fn bar(frac: f64, width: usize, unicode: bool) -> String {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (frac * width as f64).round() as usize;
    let (on, off) = if unicode { ('█', '░') } else { ('#', '-') };
    let mut s = String::with_capacity(width * 3);
    for i in 0..width {
        s.push(if i < filled { on } else { off });
    }
    s
}

/// Sparkline of the most recent `width` values, left-padded with spaces.
/// `max` fixes the scale (e.g. 100 for percent); `None` auto-scales.
pub fn spark(values: &[f64], width: usize, max: Option<f64>, unicode: bool) -> String {
    const U: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    const A: [char; 8] = ['.', ':', '-', '=', '+', '*', '#', '@'];
    let set = if unicode { &U } else { &A };
    let tail = &values[values.len().saturating_sub(width)..];
    let scale = max
        .unwrap_or_else(|| tail.iter().copied().fold(0.0, f64::max))
        .max(f64::EPSILON);
    let mut s = " ".repeat(width.saturating_sub(tail.len()));
    for v in tail {
        let level = ((v / scale).clamp(0.0, 1.0) * 7.0).round() as usize;
        s.push(set[level.min(7)]);
    }
    s
}

/// Threshold color for a 0-100 percentage.
pub fn pct_style(th: &Theme, pct: f64) -> Style {
    if pct >= 90.0 {
        th.bad
    } else if pct >= 70.0 {
        th.warn
    } else {
        th.good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_bounds() {
        assert_eq!(bar(0.0, 4, false), "----");
        assert_eq!(bar(1.0, 4, false), "####");
        assert_eq!(bar(0.5, 4, false), "##--");
        assert_eq!(bar(7.0, 4, false), "####");
        assert_eq!(bar(f64::NAN, 4, false), "----");
        assert_eq!(bar(0.5, 0, true), "");
    }

    #[test]
    fn spark_pads_and_scales() {
        assert_eq!(spark(&[], 3, Some(100.0), false), "   ");
        let s = spark(&[0.0, 100.0], 4, Some(100.0), false);
        assert_eq!(s, "  .@");
        assert_eq!(
            spark(&[1.0, 2.0, 3.0, 4.0], 2, Some(4.0), false)
                .chars()
                .count(),
            2
        );
        assert_eq!(spark(&[0.0, 0.0], 2, None, true).chars().count(), 2);
    }
}

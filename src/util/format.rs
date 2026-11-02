//! Consistent unit formatting. Binary units (KiB/MiB/GiB) throughout.

const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

/// Format a byte quantity, e.g. `1.5 GiB`.
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    scaled(n as f64)
}

/// Format a rate in bytes per second, e.g. `4.6 MiB/s`.
pub fn rate(bytes_per_sec: f64) -> String {
    if !bytes_per_sec.is_finite() || bytes_per_sec < 0.0 {
        return "—".to_string();
    }
    if bytes_per_sec < 1.0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", scaled(bytes_per_sec))
}

fn scaled(mut v: f64) -> String {
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", v.round() as u64, UNITS[0])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// Percent with no decimals; `None` renders as an em dash (never as 0%).
pub fn percent(p: Option<f64>) -> String {
    match p {
        Some(v) if v.is_finite() => format!("{v:.0}%"),
        _ => "—".to_string(),
    }
}

/// Memory cell for a process: `denied` when the OS refused access.
pub fn bytes_or_denied(b: Option<u64>) -> String {
    b.map_or_else(|| "denied".to_string(), bytes)
}

/// Process-style percent with one decimal below 10%.
pub fn percent_fine(p: Option<f32>) -> String {
    match p {
        Some(v) if v.is_finite() && v < 10.0 => format!("{v:.1}%"),
        Some(v) if v.is_finite() => format!("{v:.0}%"),
        _ => "…".to_string(),
    }
}

/// Compact duration: `4m 10s`, `2h 05m`, `3d 4h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (
        secs / 86_400,
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
    );
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(bytes(200 * 1024 * 1024), "200 MiB");
        assert_eq!(bytes(u64::MAX), "16384 PiB");
    }

    #[test]
    fn rates_have_per_second_suffix() {
        assert_eq!(rate(0.0), "0 B/s");
        assert_eq!(rate(0.4), "0 B/s");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
        assert_eq!(rate(4.6 * 1024.0 * 1024.0), "4.6 MiB/s");
        assert_eq!(rate(f64::NAN), "—");
        assert_eq!(rate(-1.0), "—");
    }

    #[test]
    fn percent_none_is_dash_not_zero() {
        assert_eq!(percent(None), "—");
        assert_eq!(percent(Some(23.6)), "24%");
        assert_eq!(percent(Some(f64::NAN)), "—");
        assert_eq!(percent_fine(None), "…");
        assert_eq!(percent_fine(Some(3.2)), "3.2%");
        assert_eq!(percent_fine(Some(250.0)), "250%");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(5), "5s");
        assert_eq!(duration(250), "4m 10s");
        assert_eq!(duration(7500), "2h 05m");
        assert_eq!(duration(3 * 86_400 + 4 * 3600), "3d 4h");
    }
}

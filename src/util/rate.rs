//! Rates derived from two counter samples and a monotonic elapsed time.

use std::time::{Duration, Instant};

/// Rate in units/second between two counter readings.
///
/// Returns `None` when no meaningful rate exists: zero elapsed time, or the
/// counter went backwards (reset/wrap — we cannot tell how much was missed).
pub fn rate_between(prev: u64, cur: u64, elapsed: Duration) -> Option<f64> {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 || cur < prev {
        return None;
    }
    Some((cur - prev) as f64 / secs)
}

/// Remembers the previous counter reading to produce successive rates.
#[derive(Debug, Default, Clone)]
pub struct RateTracker {
    prev: Option<(u64, Instant)>,
}

impl RateTracker {
    /// Feed a new reading; the first reading and any reset yield `None`.
    pub fn update(&mut self, counter: u64, at: Instant) -> Option<f64> {
        let out = self
            .prev
            .and_then(|(p, t)| rate_between(p, counter, at.saturating_duration_since(t)));
        self.prev = Some((counter, at));
        out
    }
}

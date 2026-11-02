use std::time::{Duration, Instant};
use systui::util::rate::{rate_between, RateTracker};

#[test]
fn simple_rate() {
    assert_eq!(
        rate_between(1000, 3000, Duration::from_secs(2)),
        Some(1000.0)
    );
    assert_eq!(rate_between(0, 0, Duration::from_secs(1)), Some(0.0));
}

#[test]
fn zero_elapsed_has_no_rate() {
    assert_eq!(rate_between(0, 100, Duration::ZERO), None);
}

#[test]
fn counter_reset_or_wrap_has_no_rate() {
    assert_eq!(rate_between(5000, 100, Duration::from_secs(1)), None);
    // u32-style wrap
    assert_eq!(
        rate_between(u32::MAX as u64, 10, Duration::from_secs(1)),
        None
    );
}

#[test]
fn no_overflow_at_extremes() {
    let r = rate_between(0, u64::MAX, Duration::from_millis(1)).unwrap();
    assert!(r.is_finite() && r > 0.0);
}

#[test]
fn tracker_first_sample_is_none_then_rates_then_recovers_from_reset() {
    let mut t = RateTracker::default();
    let t0 = Instant::now();
    assert_eq!(t.update(1000, t0), None, "no first delta");
    let r = t.update(3000, t0 + Duration::from_secs(2)).unwrap();
    assert!((r - 1000.0).abs() < 1e-9);
    assert_eq!(t.update(50, t0 + Duration::from_secs(3)), None, "reset");
    let r = t.update(150, t0 + Duration::from_secs(4)).unwrap();
    assert!((r - 100.0).abs() < 1e-9, "baseline restarts after a reset");
}

#[test]
fn tracker_ignores_time_going_backwards() {
    let mut t = RateTracker::default();
    let t0 = Instant::now() + Duration::from_secs(10);
    t.update(0, t0);
    assert_eq!(t.update(100, t0 - Duration::from_secs(5)), None);
}

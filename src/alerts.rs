//! Low-noise alerts: sustained-condition raising, hysteresis on clearing, and a
//! cool-down before the same alert may re-raise. Pure and time-injected.
//!
//! Missing data never raises an alert and never advances a clearing timer:
//! "unknown" is not "fine" and not "bad". (Collector problems are shown
//! separately from alerts.)

use crate::config::AlertsConfig;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlertKey {
    Cpu,
    Memory,
    DiskFree(String),
    Service(String),
}

impl AlertKey {
    pub fn label(&self) -> String {
        match self {
            AlertKey::Cpu => "CPU high".into(),
            AlertKey::Memory => "Memory high".into(),
            AlertKey::DiskFree(m) => format!("Low disk space on {m}"),
            AlertKey::Service(n) => format!("Service unhealthy: {n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveAlert {
    pub key: AlertKey,
    /// Engine time (seconds) when it was raised.
    pub since: f64,
    pub value: f64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlertEvent {
    Raised(ActiveAlert),
    Cleared(AlertKey),
}

#[derive(Debug, Default, Clone)]
struct Rule {
    active: Option<ActiveAlert>,
    /// Since when the trigger condition has held continuously.
    bad_since: Option<f64>,
    /// Since when the value has been continuously back inside the limit (by the margin).
    ok_since: Option<f64>,
    cooldown_until: f64,
}

/// One observation of the machine. `None` = not available right now.
#[derive(Debug, Default)]
pub struct Inputs<'a> {
    pub cpu_percent: Option<f64>,
    pub memory_percent: Option<f64>,
    /// (mount, free %)
    pub volumes: &'a [(String, f64)],
    /// (check name, consecutive failures). Only checks that have actually been run.
    pub services: &'a [(String, u32)],
}

pub struct AlertEngine {
    cfg: AlertsConfig,
    rules: HashMap<AlertKey, Rule>,
}

impl AlertEngine {
    pub fn new(cfg: AlertsConfig) -> Self {
        Self {
            cfg,
            rules: HashMap::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn active(&self) -> Vec<&ActiveAlert> {
        let mut v: Vec<&ActiveAlert> = self
            .rules
            .values()
            .filter_map(|r| r.active.as_ref())
            .collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        v
    }

    /// Feed one observation at engine time `t` (monotonic seconds).
    pub fn observe(&mut self, t: f64, inp: &Inputs) -> Vec<AlertEvent> {
        if !self.cfg.enabled {
            return Vec::new();
        }
        let mut ev = Vec::new();
        let c = self.cfg.clone();
        // "high" metrics: bad when value >= limit, recovered below limit - margin
        self.metric(
            t,
            AlertKey::Cpu,
            inp.cpu_percent,
            c.cpu_percent,
            true,
            "CPU",
            &mut ev,
        );
        self.metric(
            t,
            AlertKey::Memory,
            inp.memory_percent,
            c.memory_percent,
            true,
            "memory",
            &mut ev,
        );
        // "low" metric: bad when free% < limit, recovered above limit + margin
        let mut seen = Vec::new();
        for (mount, free) in inp.volumes {
            let key = AlertKey::DiskFree(mount.clone());
            self.metric(
                t,
                key.clone(),
                Some(*free),
                c.disk_free_percent_below,
                false,
                "free space",
                &mut ev,
            );
            seen.push(key);
        }
        for (name, fails) in inp.services {
            let key = AlertKey::Service(name.clone());
            let rule = self.rules.entry(key.clone()).or_default();
            if rule.active.is_none() && *fails >= c.service_failures && t >= rule.cooldown_until {
                let a = ActiveAlert {
                    key: key.clone(),
                    since: t,
                    value: f64::from(*fails),
                    detail: format!("{fails} consecutive failed checks"),
                };
                rule.active = Some(a.clone());
                ev.push(AlertEvent::Raised(a));
            } else if rule.active.is_some() && *fails == 0 {
                rule.active = None;
                rule.cooldown_until = t + c.cooldown_seconds as f64;
                ev.push(AlertEvent::Cleared(key.clone()));
            } else if let Some(a) = rule.active.as_mut() {
                a.value = f64::from(*fails);
                a.detail = format!("{fails} consecutive failed checks");
            }
            seen.push(key);
        }
        // Sources that disappeared (unmounted volume, removed check): retire their rules.
        let gone: Vec<AlertKey> = self
            .rules
            .keys()
            .filter(|k| {
                matches!(k, AlertKey::DiskFree(_) | AlertKey::Service(_)) && !seen.contains(k)
            })
            .cloned()
            .collect();
        for k in gone {
            if let Some(r) = self.rules.remove(&k) {
                if r.active.is_some() {
                    ev.push(AlertEvent::Cleared(k));
                }
            }
        }
        ev
    }

    #[allow(clippy::too_many_arguments)]
    fn metric(
        &mut self,
        t: f64,
        key: AlertKey,
        value: Option<f64>,
        limit: f64,
        high_is_bad: bool,
        what: &str,
        ev: &mut Vec<AlertEvent>,
    ) {
        let sustain = self.cfg.sustain_seconds as f64;
        let clear_for = self.cfg.clear_seconds as f64;
        let margin = self.cfg.hysteresis_points;
        let cooldown = self.cfg.cooldown_seconds as f64;
        let rule = self.rules.entry(key.clone()).or_default();
        let Some(v) = value.filter(|v| v.is_finite()) else {
            // Unknown: cannot start an alert, cannot make progress toward clearing one.
            rule.bad_since = None;
            rule.ok_since = None;
            return;
        };
        let bad = if high_is_bad { v >= limit } else { v < limit };
        let recovered = if high_is_bad {
            v < limit - margin
        } else {
            v > limit + margin
        };
        if rule.active.is_none() {
            if bad {
                let since = *rule.bad_since.get_or_insert(t);
                if t - since >= sustain && t >= rule.cooldown_until {
                    let a = ActiveAlert {
                        key: key.clone(),
                        since: t,
                        value: v,
                        detail: if high_is_bad {
                            format!(
                                "{what} at {v:.0}% (limit {limit:.0}%) for {:.0}s",
                                t - since
                            )
                        } else {
                            format!("{what} {v:.1}% (limit {limit:.0}%)")
                        },
                    };
                    rule.active = Some(a.clone());
                    rule.ok_since = None;
                    ev.push(AlertEvent::Raised(a));
                }
            } else {
                rule.bad_since = None;
            }
        } else {
            if let Some(a) = rule.active.as_mut() {
                a.value = v;
            }
            if recovered {
                let since = *rule.ok_since.get_or_insert(t);
                if t - since >= clear_for {
                    rule.active = None;
                    rule.bad_since = None;
                    rule.ok_since = None;
                    rule.cooldown_until = t + cooldown;
                    ev.push(AlertEvent::Cleared(key));
                }
            } else {
                // Still in the hysteresis band or above the limit: not recovered.
                rule.ok_since = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AlertsConfig {
        AlertsConfig {
            enabled: true,
            ..Default::default()
        } // 90 / 90 / 10, sustain 15, margin 5, clear 5, cooldown 30, 3 failures
    }
    fn cpu(e: &mut AlertEngine, t: f64, v: Option<f64>) -> Vec<AlertEvent> {
        e.observe(
            t,
            &Inputs {
                cpu_percent: v,
                ..Default::default()
            },
        )
    }
    fn raised(ev: &[AlertEvent]) -> usize {
        ev.iter()
            .filter(|e| matches!(e, AlertEvent::Raised(_)))
            .count()
    }
    fn cleared(ev: &[AlertEvent]) -> usize {
        ev.iter()
            .filter(|e| matches!(e, AlertEvent::Cleared(_)))
            .count()
    }

    #[test]
    fn disabled_engine_is_inert() {
        let mut e = AlertEngine::new(AlertsConfig::default());
        assert!(!e.enabled());
        for t in 0..100 {
            assert!(cpu(&mut e, t as f64, Some(100.0)).is_empty());
        }
        assert!(e.active().is_empty());
    }

    #[test]
    fn needs_sustained_condition_not_a_spike() {
        let mut e = AlertEngine::new(cfg());
        for t in 0..14 {
            assert!(cpu(&mut e, t as f64, Some(99.0)).is_empty(), "t={t}");
        }
        assert!(
            cpu(&mut e, 14.0, Some(10.0)).is_empty(),
            "dip resets the timer"
        );
        for t in 15..29 {
            assert!(cpu(&mut e, t as f64, Some(99.0)).is_empty(), "t={t}");
        }
        assert_eq!(
            raised(&cpu(&mut e, 30.0, Some(99.0))),
            1,
            "15 s after the dip ended"
        );
        assert_eq!(e.active().len(), 1);
        assert!(e.active()[0].detail.contains("99%"));
    }

    #[test]
    fn exactly_at_the_limit_counts_and_just_below_does_not() {
        let mut e = AlertEngine::new(cfg());
        for t in 0..=15 {
            cpu(&mut e, t as f64, Some(90.0));
        }
        assert_eq!(e.active().len(), 1);
        let mut e = AlertEngine::new(cfg());
        for t in 0..=40 {
            cpu(&mut e, t as f64, Some(89.9));
        }
        assert!(e.active().is_empty());
    }

    fn raise(e: &mut AlertEngine) {
        for t in 0..=15 {
            cpu(e, t as f64, Some(95.0));
        }
        assert_eq!(e.active().len(), 1);
    }

    #[test]
    fn hysteresis_band_does_not_clear() {
        let mut e = AlertEngine::new(cfg());
        raise(&mut e);
        // 87% is below the limit (90) but not 5 points below it: stays active.
        for t in 16..60 {
            assert_eq!(cleared(&cpu(&mut e, t as f64, Some(87.0))), 0, "t={t}");
        }
        assert_eq!(e.active().len(), 1);
        // 84% (< 85) must hold for clear_seconds (5) before clearing.
        for t in 60..65 {
            assert_eq!(cleared(&cpu(&mut e, t as f64, Some(84.0))), 0);
        }
        assert_eq!(cleared(&cpu(&mut e, 65.0, Some(84.0))), 1);
        assert!(e.active().is_empty());
    }

    #[test]
    fn flapping_around_the_clear_threshold_never_clears() {
        let mut e = AlertEngine::new(cfg());
        raise(&mut e);
        for t in 16..200 {
            let v = if t % 3 == 0 { 70.0 } else { 92.0 }; // never 5 s in a row below
            assert_eq!(cleared(&cpu(&mut e, t as f64, Some(v))), 0);
        }
        assert_eq!(e.active().len(), 1);
    }

    #[test]
    fn cooldown_blocks_an_immediate_reraise_then_allows_it() {
        let mut e = AlertEngine::new(cfg());
        raise(&mut e);
        for t in 16..=21 {
            cpu(&mut e, t as f64, Some(10.0)); // clears at t=21
        }
        assert!(e.active().is_empty());
        // Hot again right away: sustain would be met by t=37 but cooldown runs to t=51.
        let mut r = 0;
        for t in 22..51 {
            r += raised(&cpu(&mut e, t as f64, Some(99.0)));
        }
        assert_eq!(r, 0, "cooldown active");
        assert_eq!(
            raised(&cpu(&mut e, 51.0, Some(99.0))),
            1,
            "allowed after the cool-down"
        );
    }

    #[test]
    fn missing_data_neither_raises_nor_clears() {
        let mut e = AlertEngine::new(cfg());
        for t in 0..60 {
            assert!(cpu(&mut e, t as f64, None).is_empty());
        }
        assert!(e.active().is_empty(), "no alert from missing data");
        raise(&mut e);
        for t in 16..100 {
            assert_eq!(cleared(&cpu(&mut e, t as f64, None)), 0);
        }
        assert_eq!(e.active().len(), 1, "unknown is not 'recovered'");
        // unknown in the middle of a pending condition resets it
        let mut e = AlertEngine::new(cfg());
        for t in 0..10 {
            cpu(&mut e, t as f64, Some(99.0));
        }
        cpu(&mut e, 10.0, None);
        for t in 11..25 {
            assert!(cpu(&mut e, t as f64, Some(99.0)).is_empty());
        }
    }

    #[test]
    fn disk_alert_is_per_volume_and_uses_free_percent() {
        let mut e = AlertEngine::new(cfg());
        let vols = vec![("C:\\".to_string(), 5.0), ("D:\\".to_string(), 60.0)];
        let mut ev = Vec::new();
        for t in 0..=15 {
            ev.extend(e.observe(
                t as f64,
                &Inputs {
                    volumes: &vols,
                    ..Default::default()
                },
            ));
        }
        assert_eq!(raised(&ev), 1);
        assert_eq!(e.active()[0].key, AlertKey::DiskFree("C:\\".into()));
        assert!(e.active()[0].key.label().contains("C:"));
        // recovery needs free > 10 + 5
        let ok = vec![("C:\\".to_string(), 12.0)];
        for t in 16..40 {
            e.observe(
                t as f64,
                &Inputs {
                    volumes: &ok,
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            e.active().len(),
            1,
            "12% free is inside the hysteresis band"
        );
        let fine = vec![("C:\\".to_string(), 30.0)];
        let mut ev = Vec::new();
        for t in 40..50 {
            ev.extend(e.observe(
                t as f64,
                &Inputs {
                    volumes: &fine,
                    ..Default::default()
                },
            ));
        }
        assert_eq!(cleared(&ev), 1);
    }

    #[test]
    fn unmounted_volume_retires_its_alert() {
        let mut e = AlertEngine::new(cfg());
        let v = vec![("E:\\".to_string(), 1.0)];
        for t in 0..=15 {
            e.observe(
                t as f64,
                &Inputs {
                    volumes: &v,
                    ..Default::default()
                },
            );
        }
        assert_eq!(e.active().len(), 1);
        let ev = e.observe(16.0, &Inputs::default());
        assert_eq!(cleared(&ev), 1);
        assert!(e.active().is_empty());
    }

    #[test]
    fn service_alert_after_n_failures_and_clears_on_recovery() {
        let mut e = AlertEngine::new(cfg());
        let s = |n| vec![("api".to_string(), n)];
        assert!(e
            .observe(
                0.0,
                &Inputs {
                    services: &s(1),
                    ..Default::default()
                }
            )
            .is_empty());
        assert!(e
            .observe(
                1.0,
                &Inputs {
                    services: &s(2),
                    ..Default::default()
                }
            )
            .is_empty());
        assert_eq!(
            raised(&e.observe(
                2.0,
                &Inputs {
                    services: &s(3),
                    ..Default::default()
                }
            )),
            1
        );
        assert_eq!(
            raised(&e.observe(
                3.0,
                &Inputs {
                    services: &s(4),
                    ..Default::default()
                }
            )),
            0,
            "no duplicate"
        );
        assert_eq!(e.active()[0].detail, "4 consecutive failed checks");
        assert_eq!(
            cleared(&e.observe(
                4.0,
                &Inputs {
                    services: &s(0),
                    ..Default::default()
                }
            )),
            1
        );
        // cooldown applies to services too
        assert_eq!(
            raised(&e.observe(
                5.0,
                &Inputs {
                    services: &s(3),
                    ..Default::default()
                }
            )),
            0
        );
        assert_eq!(
            raised(&e.observe(
                40.0,
                &Inputs {
                    services: &s(3),
                    ..Default::default()
                }
            )),
            1
        );
    }

    #[test]
    fn active_list_is_stable_and_sorted() {
        let mut e = AlertEngine::new(cfg());
        let v = vec![("C:\\".to_string(), 1.0)];
        let sv = vec![("db".to_string(), 5)];
        for t in 0..=15 {
            e.observe(
                t as f64,
                &Inputs {
                    cpu_percent: Some(99.0),
                    memory_percent: Some(99.0),
                    volumes: &v,
                    services: &sv,
                },
            );
        }
        let keys: Vec<AlertKey> = e.active().iter().map(|a| a.key.clone()).collect();
        assert_eq!(keys.len(), 4);
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}

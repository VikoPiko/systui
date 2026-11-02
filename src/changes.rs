//! "What changed?": in-memory, session-only comparison of process snapshots.
//!
//! A bounded ring holds a light summary of the process list taken at most every
//! `MIN_GAP` seconds. Nothing is written to disk. Processes that start AND exit
//! between two stored snapshots are invisible to this view (documented).

use crate::domain::{ProcessId, ProcessSample};
use std::collections::{HashMap, VecDeque};

pub const MIN_GAP: f64 = 5.0;
pub const CAPACITY: usize = 40; // ~3 minutes at one entry per 5 s
pub const WINDOWS: [f64; 3] = [5.0, 30.0, 120.0];
const MIN_CPU_DELTA: f32 = 1.0;
const MIN_MEM_DELTA: u64 = 10 << 20;
const TOP: usize = 5;

#[derive(Debug, Clone)]
struct Point {
    name: String,
    cpu: Option<f32>,
    rss: Option<u64>,
}

#[derive(Debug, Clone)]
struct Entry {
    t: f64,
    procs: HashMap<ProcessId, Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub pid: u32,
    pub name: String,
    pub from: f64,
    pub to: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Appeared {
    pub pid: u32,
    pub name: String,
    pub rss: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Real time between the two compared snapshots.
    pub span_secs: f64,
    pub started: Vec<Appeared>,
    pub exited: Vec<Appeared>,
    pub cpu_up: Vec<Delta>,
    pub cpu_down: Vec<Delta>,
    pub mem_up: Vec<Delta>,
    pub mem_down: Vec<Delta>,
    pub started_total: usize,
    pub exited_total: usize,
}

#[derive(Debug, Default)]
pub struct ChangeLog {
    ring: VecDeque<Entry>,
}

impl ChangeLog {
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }

    /// Record the list at engine time `t` (seconds), at most once per `MIN_GAP`.
    /// Only pass FRESH process lists.
    pub fn push(&mut self, t: f64, procs: &[ProcessSample]) {
        if self.ring.back().is_some_and(|e| t - e.t < MIN_GAP) {
            return;
        }
        if self.ring.len() == CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(Entry {
            t,
            procs: procs
                .iter()
                .map(|p| {
                    (
                        p.id,
                        Point {
                            name: p.name.clone(),
                            cpu: p.cpu,
                            rss: p.rss,
                        },
                    )
                })
                .collect(),
        });
    }

    /// Age of the newest stored snapshot relative to `t`.
    pub fn newest_age(&self, t: f64) -> Option<f64> {
        self.ring.back().map(|e| (t - e.t).max(0.0))
    }

    /// Compare the newest entry with the stored one closest to `window` seconds earlier.
    pub fn compare(&self, window: f64) -> Option<Report> {
        let new = self.ring.back()?;
        let old = self.ring.iter().rev().skip(1).min_by(|a, b| {
            ((new.t - a.t) - window)
                .abs()
                .partial_cmp(&((new.t - b.t) - window).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        Some(diff(old, new))
    }
}

fn top(mut v: Vec<Delta>, up: bool) -> Vec<Delta> {
    v.sort_by(|a, b| {
        let (da, db) = (a.to - a.from, b.to - b.from);
        let o = if up {
            db.partial_cmp(&da)
        } else {
            da.partial_cmp(&db)
        };
        o.unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pid.cmp(&b.pid))
    });
    v.truncate(TOP);
    v
}

fn diff(old: &Entry, new: &Entry) -> Report {
    let mut started: Vec<Appeared> = new
        .procs
        .iter()
        .filter(|(id, _)| !old.procs.contains_key(id))
        .map(|(id, p)| Appeared {
            pid: id.pid,
            name: p.name.clone(),
            rss: p.rss,
        })
        .collect();
    let mut exited: Vec<Appeared> = old
        .procs
        .iter()
        .filter(|(id, _)| !new.procs.contains_key(id))
        .map(|(id, p)| Appeared {
            pid: id.pid,
            name: p.name.clone(),
            rss: p.rss,
        })
        .collect();
    // biggest first, then pid, so output is deterministic
    let key = |a: &Appeared| (std::cmp::Reverse(a.rss.unwrap_or(0)), a.pid);
    started.sort_by_key(key);
    exited.sort_by_key(key);
    let (started_total, exited_total) = (started.len(), exited.len());
    started.truncate(TOP);
    exited.truncate(TOP);

    let (mut cpu, mut mem) = (Vec::new(), Vec::new());
    for (id, n) in &new.procs {
        let Some(o) = old.procs.get(id) else { continue };
        // Only compare readings that exist on both sides (unknown is not 0).
        if let (Some(a), Some(b)) = (o.cpu, n.cpu) {
            if (b - a).abs() >= MIN_CPU_DELTA {
                cpu.push(Delta {
                    pid: id.pid,
                    name: n.name.clone(),
                    from: f64::from(a),
                    to: f64::from(b),
                });
            }
        }
        if let (Some(a), Some(b)) = (o.rss, n.rss) {
            if a.abs_diff(b) >= MIN_MEM_DELTA {
                mem.push(Delta {
                    pid: id.pid,
                    name: n.name.clone(),
                    from: a as f64,
                    to: b as f64,
                });
            }
        }
    }
    Report {
        span_secs: new.t - old.t,
        started,
        exited,
        cpu_up: top(
            cpu.iter().filter(|d| d.to > d.from).cloned().collect(),
            true,
        ),
        cpu_down: top(
            cpu.iter().filter(|d| d.to < d.from).cloned().collect(),
            false,
        ),
        mem_up: top(
            mem.iter().filter(|d| d.to > d.from).cloned().collect(),
            true,
        ),
        mem_down: top(
            mem.iter().filter(|d| d.to < d.from).cloned().collect(),
            false,
        ),
        started_total,
        exited_total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use std::sync::Arc;

    fn p(pid: u32, name: &str, cpu: Option<f32>, rss: Option<u64>) -> ProcessSample {
        ProcessSample {
            id: ProcessId {
                pid,
                started_at: Some(1000 + pid as u64),
            },
            parent: None,
            name: name.into(),
            cpu,
            rss,
            restricted: rss.is_none(),
            status: None,
            io: None,
            info: Arc::new(ProcessStatic {
                exe: None,
                cmd: CmdLine::NotRequested,
            }),
        }
    }

    const MB: u64 = 1 << 20;

    #[test]
    fn detects_started_exited_and_changes() {
        let mut log = ChangeLog::default();
        log.push(
            0.0,
            &[
                p(1, "a", Some(5.0), Some(100 * MB)),
                p(2, "gone", Some(1.0), Some(50 * MB)),
                p(3, "grows", Some(2.0), Some(100 * MB)),
            ],
        );
        log.push(
            30.0,
            &[
                p(1, "a", Some(60.0), Some(101 * MB)),
                p(3, "grows", Some(2.2), Some(400 * MB)),
                p(4, "new", Some(9.0), Some(200 * MB)),
            ],
        );
        let r = log.compare(30.0).unwrap();
        assert_eq!(r.span_secs, 30.0);
        assert_eq!(
            r.started
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
        assert_eq!(
            r.exited.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["gone"]
        );
        assert_eq!(r.cpu_up.len(), 1);
        assert_eq!(r.cpu_up[0].name, "a");
        assert_eq!((r.cpu_up[0].from, r.cpu_up[0].to), (5.0, 60.0));
        assert_eq!(r.mem_up.len(), 1);
        assert_eq!(r.mem_up[0].name, "grows");
        assert!(r.cpu_down.is_empty() && r.mem_down.is_empty());
    }

    #[test]
    fn small_changes_are_noise_and_ignored() {
        let mut log = ChangeLog::default();
        log.push(0.0, &[p(1, "a", Some(5.0), Some(100 * MB))]);
        log.push(10.0, &[p(1, "a", Some(5.4), Some(105 * MB))]);
        let r = log.compare(5.0).unwrap();
        assert!(
            r.cpu_up.is_empty()
                && r.cpu_down.is_empty()
                && r.mem_up.is_empty()
                && r.mem_down.is_empty()
        );
    }

    #[test]
    fn unknown_readings_are_not_compared_as_zero() {
        let mut log = ChangeLog::default();
        log.push(
            0.0,
            &[p(1, "a", None, None), p(2, "b", Some(50.0), Some(500 * MB))],
        );
        log.push(
            10.0,
            &[p(1, "a", Some(80.0), Some(900 * MB)), p(2, "b", None, None)],
        );
        let r = log.compare(10.0).unwrap();
        assert!(
            r.cpu_up.is_empty() && r.cpu_down.is_empty(),
            "no from-0 or to-0 deltas: {r:?}"
        );
        assert!(r.mem_up.is_empty() && r.mem_down.is_empty());
        assert!(r.started.is_empty() && r.exited.is_empty());
    }

    #[test]
    fn pid_reuse_is_a_different_process() {
        let mut log = ChangeLog::default();
        log.push(0.0, &[p(7, "old", Some(1.0), Some(10 * MB))]);
        let mut reused = p(7, "new-owner", Some(1.0), Some(10 * MB));
        reused.id.started_at = Some(999_999);
        log.push(10.0, &[reused]);
        let r = log.compare(10.0).unwrap();
        assert_eq!(r.started.len(), 1);
        assert_eq!(r.exited.len(), 1);
        assert_eq!(r.started[0].name, "new-owner");
        assert_eq!(r.exited[0].name, "old");
    }

    #[test]
    fn push_is_rate_limited_and_the_ring_is_bounded() {
        let mut log = ChangeLog::default();
        for i in 0..1000 {
            log.push(i as f64, &[p(1, "a", Some(1.0), Some(MB))]);
        }
        assert_eq!(log.len(), CAPACITY);
        let mut l2 = ChangeLog::default();
        for i in 0..4 {
            l2.push(i as f64, &[]);
        }
        assert_eq!(l2.len(), 1, "entries closer than MIN_GAP are skipped");
    }

    #[test]
    fn picks_the_entry_closest_to_the_requested_window_and_reports_the_real_span() {
        let mut log = ChangeLog::default();
        for t in [0.0, 10.0, 20.0, 60.0, 100.0] {
            log.push(t, &[p(1, "a", Some(1.0), Some(MB))]);
        }
        let r = log.compare(30.0).unwrap(); // newest is t=100: closest older is 60 (age 40) vs 20 (age 80)
        assert_eq!(r.span_secs, 40.0);
        let r = log.compare(120.0).unwrap();
        assert_eq!(r.span_secs, 100.0, "uses the oldest available; says so");
    }

    #[test]
    fn needs_two_entries() {
        let mut log = ChangeLog::default();
        assert!(log.compare(30.0).is_none());
        log.push(0.0, &[]);
        assert!(log.compare(30.0).is_none());
        assert_eq!(log.newest_age(7.5), Some(7.5));
    }

    #[test]
    fn totals_count_everything_but_lists_are_capped() {
        let mut log = ChangeLog::default();
        log.push(0.0, &[]);
        let many: Vec<_> = (0..40)
            .map(|i| p(i, &format!("n{i}"), Some(1.0), Some((i as u64 + 1) * MB)))
            .collect();
        log.push(10.0, &many);
        let r = log.compare(10.0).unwrap();
        assert_eq!(r.started_total, 40);
        assert_eq!(r.started.len(), 5);
        assert_eq!(r.started[0].name, "n39", "largest first");
    }

    #[test]
    fn ordering_is_by_magnitude_of_change() {
        let mut log = ChangeLog::default();
        log.push(
            0.0,
            &[
                p(1, "a", Some(10.0), None),
                p(2, "b", Some(10.0), None),
                p(3, "c", Some(90.0), None),
            ],
        );
        log.push(
            10.0,
            &[
                p(1, "a", Some(20.0), None),
                p(2, "b", Some(70.0), None),
                p(3, "c", Some(10.0), None),
            ],
        );
        let r = log.compare(10.0).unwrap();
        assert_eq!(
            r.cpu_up.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(
            r.cpu_down
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>(),
            ["c"]
        );
    }
}

//! Collector worker thread. Collection never runs on the UI thread.
//!
//! The worker publishes into a single "latest" slot (last-value-wins), so a
//! slow UI can never build an unbounded backlog.

pub mod system;

use crate::domain::{Needs, ProcessId, Snapshot};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Minimum spacing between samples (CPU deltas need a real interval).
const MIN_SAMPLE_GAP: Duration = Duration::from_millis(400);
/// The second sample comes early so CPU % is available quickly after launch.
const WARMUP_GAP: Duration = Duration::from_millis(500);

pub enum Cmd {
    RefreshNow,
    Watch(Option<ProcessId>),
    /// What the visible view needs (skip refreshing the rest to save CPU).
    Needs(Needs),
    Shutdown,
}

pub struct Collector {
    tx: Sender<Cmd>,
    latest: Arc<Mutex<Option<Arc<Snapshot>>>>,
    join: Option<JoinHandle<()>>,
}

impl Collector {
    pub fn spawn(interval: Duration) -> Collector {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let latest: Arc<Mutex<Option<Arc<Snapshot>>>> = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let join = std::thread::Builder::new()
            .name("systui-collector".into())
            .spawn(move || {
                let publish = |s: Snapshot| {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(Arc::new(s));
                    }
                };
                let mut collector = match system::SystemCollector::new() {
                    Ok(c) => c,
                    Err(msg) => {
                        publish(system::failed_snapshot(msg));
                        // Stay alive (idle) until shutdown so the UI keeps the error.
                        while let Ok(c) = rx.recv() {
                            if matches!(c, Cmd::Shutdown) {
                                break;
                            }
                        }
                        return;
                    }
                };
                publish(collector.sample());
                let mut last = Instant::now();
                let mut next = last + WARMUP_GAP.min(interval);
                loop {
                    let wait = next.saturating_duration_since(Instant::now());
                    let due = match rx.recv_timeout(wait) {
                        Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                        Ok(Cmd::Watch(w)) => {
                            collector.watch(w);
                            w.is_some() && last.elapsed() >= MIN_SAMPLE_GAP
                        }
                        Ok(Cmd::Needs(n)) => {
                            if collector.set_needs(n) {
                                // A subsystem was switched back on: sample now (fresh
                                // baseline) and again after the warm-up gap.
                                publish(collector.sample());
                                last = Instant::now();
                                next = last + WARMUP_GAP.min(interval);
                            }
                            false
                        }
                        Ok(Cmd::RefreshNow) => last.elapsed() >= MIN_SAMPLE_GAP,
                        Err(RecvTimeoutError::Timeout) => true,
                    };
                    if due {
                        publish(collector.sample());
                        last = Instant::now();
                        next = last + interval;
                    }
                }
            })
            .ok();
        Collector { tx, latest, join }
    }

    /// Take the newest unseen snapshot, if any.
    pub fn take_latest(&self) -> Option<Arc<Snapshot>> {
        self.latest.lock().ok().and_then(|mut g| g.take())
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

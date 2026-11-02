//! Health-check worker. Unlike the Dev-tab modules it runs continuously once
//! enabled (alerts depend on it) and only ever executes the checks the user
//! configured. Checks run one at a time, each bounded by its own timeout.

use crate::config::CheckKind;
use crate::health::{run_check, CheckSpec, Outcome, Status};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub struct CheckState {
    pub name: String,
    pub kind: CheckKind,
    /// Display-safe target (no credentials, no query string).
    pub target: String,
    pub enabled: bool,
    pub status: Status,
    pub latency: Option<Duration>,
    pub message: Option<String>,
    pub last_checked: Option<SystemTime>,
    pub last_success: Option<SystemTime>,
    pub next_due: Option<SystemTime>,
    pub consecutive_failures: u32,
    pub interval: Duration,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HealthSnapshot {
    pub seq: u64,
    pub checks: Vec<CheckState>,
}

impl CheckState {
    pub fn new(spec: &CheckSpec) -> Self {
        Self {
            name: spec.name.clone(),
            kind: spec.kind,
            target: spec.describe(),
            enabled: spec.enabled,
            status: Status::Unknown,
            latency: None,
            message: (!spec.enabled).then(|| "disabled in config".to_string()),
            last_checked: None,
            last_success: None,
            next_due: None,
            consecutive_failures: 0,
            interval: spec.interval,
            timeout: spec.timeout,
        }
    }

    /// Fold a check result into the state.
    pub fn apply(&mut self, o: Outcome, now: SystemTime) {
        self.last_checked = Some(now);
        self.next_due = Some(now + self.interval);
        if o.status.is_failure() {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.consecutive_failures = 0;
            self.last_success = Some(now);
        }
        self.status = o.status;
        self.latency = o.latency;
        self.message = o.message;
    }

    /// No result for several intervals: whatever `status` says is out of date.
    pub fn is_stale(&self, now: SystemTime) -> bool {
        self.enabled
            && self.last_checked.is_some_and(|t| {
                now.duration_since(t).unwrap_or_default() > self.interval * 3 + self.timeout
            })
    }
}

pub enum HealthCmd {
    CheckNow,
    Shutdown,
}

pub struct HealthHandle {
    tx: Sender<HealthCmd>,
    latest: Arc<Mutex<Option<Arc<HealthSnapshot>>>>,
    join: Option<JoinHandle<()>>,
}

impl HealthHandle {
    pub fn spawn(specs: Vec<CheckSpec>) -> Self {
        let (tx, rx) = mpsc::channel();
        let latest: Arc<Mutex<Option<Arc<HealthSnapshot>>>> = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let join = std::thread::Builder::new()
            .name("systui-health".into())
            .spawn(move || {
                let mut states: Vec<CheckState> = specs.iter().map(CheckState::new).collect();
                let mut seq = 0u64;
                let mut publish = |states: &[CheckState]| {
                    seq += 1;
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(Arc::new(HealthSnapshot {
                            seq,
                            checks: states.to_vec(),
                        }));
                    }
                };
                publish(&states);
                loop {
                    let now = SystemTime::now();
                    for (i, spec) in specs.iter().enumerate() {
                        let due = spec.enabled && states[i].next_due.is_none_or(|t| t <= now);
                        if !due {
                            continue;
                        }
                        let outcome = run_check(spec);
                        states[i].apply(outcome, SystemTime::now());
                        publish(&states);
                        // Stay responsive to shutdown between (possibly slow) checks.
                        if let Ok(HealthCmd::Shutdown) = rx.try_recv() {
                            return;
                        }
                    }
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(HealthCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(HealthCmd::CheckNow) => {
                            for s in states.iter_mut().filter(|s| s.enabled) {
                                s.next_due = None;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .ok();
        Self { tx, latest, join }
    }

    pub fn check_now(&self) {
        let _ = self.tx.send(HealthCmd::CheckNow);
    }

    pub fn take_latest(&self) -> Option<Arc<HealthSnapshot>> {
        self.latest.lock().ok().and_then(|mut g| g.take())
    }
}

impl Drop for HealthHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(HealthCmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CheckConfig;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    fn spec(kind: CheckKind, f: impl FnOnce(&mut CheckConfig)) -> CheckSpec {
        let mut c = CheckConfig {
            name: "t".into(),
            kind,
            interval_seconds: 5,
            timeout_ms: 500,
            ..Default::default()
        };
        f(&mut c);
        CheckSpec::from_config(&c, false).unwrap()
    }

    fn out(status: Status) -> Outcome {
        Outcome {
            status,
            latency: None,
            message: None,
        }
    }

    #[test]
    fn failure_counter_resets_on_success_and_port_open_is_not_a_failure() {
        let s = spec(CheckKind::Tcp, |c| {
            c.host = Some("127.0.0.1".into());
            c.port = Some(1);
        });
        let mut st = CheckState::new(&s);
        let t0 = SystemTime::now();
        st.apply(out(Status::NotReachable), t0);
        st.apply(out(Status::HttpResponded(500)), t0);
        assert_eq!(st.consecutive_failures, 2);
        assert!(st.last_success.is_none());
        st.apply(out(Status::PortOpen), t0);
        assert_eq!(st.consecutive_failures, 0);
        assert!(st.last_success.is_some());
        assert_eq!(st.next_due, Some(t0 + Duration::from_secs(5)));
    }

    #[test]
    fn staleness_needs_a_check_and_ignores_disabled() {
        let s = spec(CheckKind::Process, |c| c.process = Some("x".into()));
        let mut st = CheckState::new(&s);
        let t0 = SystemTime::now();
        assert!(
            !st.is_stale(t0 + Duration::from_secs(999)),
            "never checked => unknown, not stale"
        );
        st.apply(out(Status::Healthy), t0);
        assert!(!st.is_stale(t0 + Duration::from_secs(10)));
        assert!(
            st.is_stale(t0 + Duration::from_secs(20)),
            "> 3 intervals + timeout"
        );
        let d = spec(CheckKind::Process, |c| {
            c.process = Some("x".into());
            c.enabled = false;
        });
        let ds = CheckState::new(&d);
        assert_eq!(ds.message.as_deref(), Some("disabled in config"));
        assert!(!ds.is_stale(t0 + Duration::from_secs(999)));
    }

    #[test]
    fn disabled_checks_are_never_run() {
        // Port that would be reported "port open" if (wrongly) checked.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = HealthHandle::spawn(vec![spec(CheckKind::Tcp, |c| {
            c.host = Some("127.0.0.1".into());
            c.port = Some(port);
            c.enabled = false;
        })]);
        std::thread::sleep(Duration::from_millis(800));
        let snap = h.take_latest().unwrap();
        assert_eq!(snap.checks[0].status, Status::Unknown);
        assert!(snap.checks[0].last_checked.is_none());
        l.set_nonblocking(true).unwrap();
        assert!(
            l.accept().is_err(),
            "nobody connected to the disabled target"
        );
    }

    #[test]
    fn worker_runs_real_checks_against_a_disposable_local_server() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut b = [0u8; 512];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let dead = {
            let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
            l2.local_addr().unwrap().port()
        };
        let h = HealthHandle::spawn(vec![
            spec(CheckKind::Http, |c| {
                c.url = Some(format!("http://127.0.0.1:{port}/health"))
            }),
            spec(CheckKind::Tcp, |c| {
                c.name = "dead".into();
                c.host = Some("127.0.0.1".into());
                c.port = Some(dead);
            }),
        ]);
        let end = Instant::now() + Duration::from_secs(10);
        let mut last = None;
        while Instant::now() < end {
            if let Some(s) = h.take_latest() {
                if s.checks.iter().all(|c| c.last_checked.is_some()) {
                    last = Some(s);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let s = last.expect("both checks ran");
        assert_eq!(s.checks[0].status, Status::Healthy);
        assert!(s.checks[0].latency.is_some() && s.checks[0].last_success.is_some());
        assert_eq!(s.checks[1].status, Status::NotReachable);
        assert_eq!(s.checks[1].consecutive_failures, 1);
        assert!(s.checks[1].next_due.is_some());
        drop(h); // joins promptly
    }

    #[test]
    fn check_now_triggers_an_immediate_recheck() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = HealthHandle::spawn(vec![spec(CheckKind::Tcp, |c| {
            c.host = Some("127.0.0.1".into());
            c.port = Some(port);
            c.interval_seconds = 3600;
        })]);
        let wait = |n: u64| {
            let end = Instant::now() + Duration::from_secs(5);
            while Instant::now() < end {
                if let Some(s) = h.take_latest() {
                    if s.checks[0].last_checked.is_some() && s.seq > n {
                        return Some(s);
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        };
        let first = wait(0).expect("first check");
        h.check_now();
        let second = wait(first.seq).expect("rechecked despite a 1 h interval");
        assert!(second.checks[0].last_checked > first.checks[0].last_checked);
    }
}

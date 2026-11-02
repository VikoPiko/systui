//! SSH worker. It contacts a host only when ALL of these hold: the module is enabled,
//! the host is in the config allowlist, the user consented for this session, the host
//! entry is enabled, and the Services tab is visible. Consent is never persisted.

use crate::ssh::{poll, Failure, HostSpec, RemoteStatus};
use crate::util::exec::{ExecError, Output};
use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshStatus {
    NeedsConsent,
    Checking,
    Ok,
    Unreachable(String),
    Error(String),
    /// `enabled = false` in the config.
    Disabled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostState {
    pub name: String,
    pub dest: String,
    pub status: SshStatus,
    /// Last good data; may be older than `status` (then it is stale).
    pub data: Option<RemoteStatus>,
    pub last_checked: Option<SystemTime>,
    pub last_ok: Option<SystemTime>,
    pub next_due: Option<SystemTime>,
    pub interval: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshSnapshot {
    pub seq: u64,
    pub hosts: Vec<HostState>,
}

impl HostState {
    pub fn new(spec: &HostSpec) -> Self {
        Self {
            name: spec.name.clone(),
            dest: spec.dest(),
            status: if spec.enabled {
                SshStatus::NeedsConsent
            } else {
                SshStatus::Disabled
            },
            data: None,
            last_checked: None,
            last_ok: None,
            next_due: None,
            interval: spec.interval,
        }
    }

    pub fn apply(&mut self, r: Result<RemoteStatus, Failure>, now: SystemTime) {
        self.last_checked = Some(now);
        self.next_due = Some(now + self.interval);
        match r {
            Ok(d) => {
                self.status = SshStatus::Ok;
                self.data = Some(d);
                self.last_ok = Some(now);
            }
            Err(Failure::Unreachable(m)) => self.status = SshStatus::Unreachable(m),
            Err(Failure::Error(m)) => self.status = SshStatus::Error(m),
        }
    }

    pub fn is_stale(&self, now: SystemTime) -> bool {
        self.status == SshStatus::Ok
            && self.last_ok.is_some_and(|t| {
                now.duration_since(t).unwrap_or_default()
                    > self.interval * 3 + Duration::from_secs(20)
            })
    }
}

pub enum SshCmd {
    Consent(String),
    CheckNow,
    Visible(bool),
    Shutdown,
}

pub type Runner = Arc<dyn Fn(&[String], Duration) -> Result<Output, ExecError> + Send + Sync>;

pub struct SshHandle {
    tx: Sender<SshCmd>,
    latest: Arc<Mutex<Option<Arc<SshSnapshot>>>>,
    join: Option<JoinHandle<()>>,
}

impl SshHandle {
    pub fn spawn(specs: Vec<HostSpec>, runner: Runner) -> Self {
        let (tx, rx) = mpsc::channel();
        let latest: Arc<Mutex<Option<Arc<SshSnapshot>>>> = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let join = std::thread::Builder::new()
            .name("systui-ssh".into())
            .spawn(move || {
                let mut states: Vec<HostState> = specs.iter().map(HostState::new).collect();
                let mut consented: HashSet<String> = HashSet::new();
                let mut visible = false;
                let mut seq = 0u64;
                let mut publish = |s: &[HostState]| {
                    seq += 1;
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(Arc::new(SshSnapshot {
                            seq,
                            hosts: s.to_vec(),
                        }));
                    }
                };
                publish(&states);
                loop {
                    if visible {
                        let now = SystemTime::now();
                        for (i, spec) in specs.iter().enumerate() {
                            let due = spec.enabled
                                && consented.contains(&spec.name.to_lowercase())
                                && states[i].next_due.is_none_or(|t| t <= now);
                            if !due {
                                continue;
                            }
                            let r = poll(spec, |a, t| runner(a, t));
                            states[i].apply(r, SystemTime::now());
                            publish(&states);
                            if let Ok(SshCmd::Shutdown) = rx.try_recv() {
                                return;
                            }
                        }
                    }
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(SshCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(SshCmd::Visible(v)) => visible = v,
                        Ok(SshCmd::CheckNow) => {
                            for (i, s) in specs.iter().enumerate() {
                                if consented.contains(&s.name.to_lowercase()) {
                                    states[i].next_due = None;
                                }
                            }
                        }
                        Ok(SshCmd::Consent(name)) => {
                            let key = name.to_lowercase();
                            // Only names from the allowlist can ever be consented to.
                            if let Some(i) = specs
                                .iter()
                                .position(|s| s.enabled && s.name.to_lowercase() == key)
                            {
                                consented.insert(key);
                                states[i].status = SshStatus::Checking;
                                states[i].next_due = None;
                                publish(&states);
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .ok();
        Self { tx, latest, join }
    }

    pub fn send(&self, c: SshCmd) {
        let _ = self.tx.send(c);
    }

    pub fn take_latest(&self) -> Option<Arc<SshSnapshot>> {
        self.latest.lock().ok().and_then(|mut g| g.take())
    }
}

impl Drop for SshHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(SshCmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshHostConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn spec(name: &str, enabled: bool) -> HostSpec {
        HostSpec::from_config(&SshHostConfig {
            name: name.into(),
            host: "10.0.0.5".into(),
            enabled,
            ..Default::default()
        })
        .unwrap()
    }

    const OK_OUT: &str = "0.1 0.2 0.3 1/1 1\n#SYSTUI#\nMemTotal: 2048 kB\nMemAvailable: 1024 kB\n#SYSTUI#\n#SYSTUI#\n10.0\n";

    fn counting_runner(counter: Arc<AtomicUsize>) -> Runner {
        Arc::new(move |_args, _t| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Output {
                stdout: OK_OUT.as_bytes().to_vec(),
                stderr: vec![],
                truncated: false,
            })
        })
    }

    fn wait(
        h: &SshHandle,
        secs: u64,
        pred: &dyn Fn(&SshSnapshot) -> bool,
    ) -> Option<Arc<SshSnapshot>> {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Some(s) = h.take_latest() {
                if pred(&s) {
                    return Some(s);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn nothing_is_contacted_without_consent_even_when_visible() {
        let calls = Arc::new(AtomicUsize::new(0));
        let h = SshHandle::spawn(vec![spec("lab", true)], counting_runner(calls.clone()));
        h.send(SshCmd::Visible(true));
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no consent => no connection"
        );
        let s = wait(&h, 2, &|_| true).unwrap();
        assert_eq!(s.hosts[0].status, SshStatus::NeedsConsent);
    }

    #[test]
    fn consent_alone_does_not_connect_while_the_tab_is_hidden() {
        let calls = Arc::new(AtomicUsize::new(0));
        let h = SshHandle::spawn(vec![spec("lab", true)], counting_runner(calls.clone()));
        h.send(SshCmd::Consent("lab".into()));
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "hidden => no polling");
        h.send(SshCmd::Visible(true));
        let s = wait(&h, 5, &|s| s.hosts[0].status == SshStatus::Ok).expect("polled once visible");
        assert!(s.hosts[0].data.as_ref().unwrap().load.is_some());
        assert!(calls.load(Ordering::SeqCst) >= 1);
        h.send(SshCmd::Visible(false));
        std::thread::sleep(Duration::from_millis(400));
        let frozen = calls.load(Ordering::SeqCst);
        h.send(SshCmd::CheckNow);
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            frozen,
            "hiding the tab stops contact"
        );
    }

    #[test]
    fn consent_is_per_host_and_unknown_names_are_ignored() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let (c2, s2) = (calls.clone(), seen.clone());
        let runner: Runner = Arc::new(move |args, _| {
            c2.fetch_add(1, Ordering::SeqCst);
            s2.lock().unwrap().push(args.join(" "));
            Ok(Output {
                stdout: OK_OUT.as_bytes().to_vec(),
                stderr: vec![],
                truncated: false,
            })
        });
        let mut b = spec("other", true);
        b.host = "10.0.0.9".into();
        let h = SshHandle::spawn(vec![spec("lab", true), b], runner);
        h.send(SshCmd::Visible(true));
        h.send(SshCmd::Consent("not-in-the-allowlist".into()));
        h.send(SshCmd::Consent("LAB".into())); // case-insensitive match of an allowlisted name
        let s = wait(&h, 5, &|s| s.hosts[0].status == SshStatus::Ok).expect("lab polled");
        assert_eq!(
            s.hosts[1].status,
            SshStatus::NeedsConsent,
            "the other host stays untouched"
        );
        std::thread::sleep(Duration::from_millis(600));
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter()
                .all(|a| a.contains("10.0.0.5") && !a.contains("10.0.0.9")),
            "{seen:?}"
        );
    }

    #[test]
    fn disabled_hosts_cannot_be_consented() {
        let calls = Arc::new(AtomicUsize::new(0));
        let h = SshHandle::spawn(vec![spec("off", false)], counting_runner(calls.clone()));
        h.send(SshCmd::Visible(true));
        h.send(SshCmd::Consent("off".into()));
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            wait(&h, 2, &|_| true).unwrap().hosts[0].status,
            SshStatus::Disabled
        );
    }

    #[test]
    fn failures_keep_the_last_good_data_and_report_the_reason() {
        let mut st = HostState::new(&spec("lab", true));
        let t0 = SystemTime::now();
        st.apply(Ok(crate::ssh::parse_output(OK_OUT, &[]).unwrap()), t0);
        assert_eq!(st.status, SshStatus::Ok);
        st.apply(
            Err(Failure::Unreachable("connection refused".into())),
            t0 + Duration::from_secs(60),
        );
        assert_eq!(
            st.status,
            SshStatus::Unreachable("connection refused".into())
        );
        assert!(st.data.is_some(), "last good data retained");
        assert_eq!(st.last_ok, Some(t0));
        assert_eq!(st.next_due, Some(t0 + Duration::from_secs(120)));
    }

    #[test]
    fn staleness() {
        let mut st = HostState::new(&spec("lab", true));
        let t0 = SystemTime::now();
        assert!(
            !st.is_stale(t0 + Duration::from_secs(9999)),
            "never succeeded => not 'stale'"
        );
        st.apply(Ok(crate::ssh::parse_output(OK_OUT, &[]).unwrap()), t0);
        assert!(!st.is_stale(t0 + Duration::from_secs(100)));
        assert!(
            st.is_stale(t0 + Duration::from_secs(300)),
            "> 3 intervals + 20 s"
        );
    }

    #[test]
    fn drop_joins_promptly() {
        let h = SshHandle::spawn(
            vec![spec("lab", true)],
            counting_runner(Arc::new(AtomicUsize::new(0))),
        );
        let t = Instant::now();
        drop(h);
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}

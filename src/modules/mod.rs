//! Optional integrations (Docker, WSL, GPU). Each runs on its own worker thread
//! with its own cadence, starts lazily the first time the Dev tab is opened, polls
//! only while that tab is visible, and can fail without affecting core monitoring.

pub mod docker;
pub mod gpu;
pub mod health;
pub mod ssh;
pub mod wsl;

use crate::config::Config;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModStatus {
    /// Not enabled in the config: nothing is probed or polled.
    Disabled,
    /// Enabled but not polled yet.
    Pending,
    Ok,
    /// The integration cannot work here (missing CLI/driver, engine not running, wrong OS).
    Unavailable(String),
    /// It should work but the last poll failed (timeout, bad output).
    Error(String),
}

#[derive(Debug)]
pub struct ModuleSnapshot<T> {
    pub seq: u64,
    pub status: ModStatus,
    /// Last good data. May be older than `status` indicates (then it is stale).
    pub data: Option<T>,
    pub observed_at: SystemTime,
}

pub enum ModCmd {
    /// Poll now (regular data).
    Refresh,
    /// Poll now including expensive extras (e.g. `docker system df`).
    Extra,
    Visible(bool),
    Shutdown,
}

pub struct ModuleHandle<T> {
    tx: Sender<ModCmd>,
    latest: Arc<Mutex<Option<Arc<ModuleSnapshot<T>>>>>,
    join: Option<JoinHandle<()>>,
}

impl<T: Clone + Send + Sync + 'static> ModuleHandle<T> {
    /// `poll(extra)` returns fresh data or the reason it could not.
    pub fn spawn<F>(name: &str, cadence: Duration, mut poll: F) -> Self
    where
        F: FnMut(bool) -> Result<T, ModStatus> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<ModCmd>();
        let latest: Arc<Mutex<Option<Arc<ModuleSnapshot<T>>>>> = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let join = std::thread::Builder::new()
            .name(format!("systui-{name}"))
            .spawn(move || {
                let mut visible = false;
                let mut last_data: Option<T> = None;
                let mut seq = 0u64;
                let mut do_poll = |extra: bool| {
                    let (status, data) = match poll(extra) {
                        Ok(d) => {
                            last_data = Some(d.clone());
                            (ModStatus::Ok, Some(d))
                        }
                        Err(s) => (s, last_data.clone()),
                    };
                    seq += 1;
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(Arc::new(ModuleSnapshot {
                            seq,
                            status,
                            data,
                            observed_at: SystemTime::now(),
                        }));
                    }
                };
                loop {
                    // Hidden: sleep until told otherwise (no polling at all).
                    let wait = if visible {
                        cadence
                    } else {
                        Duration::from_secs(3600)
                    };
                    match rx.recv_timeout(wait) {
                        Ok(ModCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                        Ok(ModCmd::Visible(v)) => {
                            let became_visible = v && !visible;
                            visible = v;
                            if became_visible {
                                do_poll(false);
                            }
                        }
                        Ok(ModCmd::Refresh) => {
                            if visible {
                                do_poll(false)
                            }
                        }
                        Ok(ModCmd::Extra) => {
                            if visible {
                                do_poll(true)
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if visible {
                                do_poll(false)
                            }
                        }
                    }
                }
            })
            .ok();
        Self { tx, latest, join }
    }

    pub fn send(&self, c: ModCmd) {
        let _ = self.tx.send(c);
    }

    pub fn take_latest(&self) -> Option<Arc<ModuleSnapshot<T>>> {
        self.latest.lock().ok().and_then(|mut g| g.take())
    }
}

impl<T> Drop for ModuleHandle<T> {
    fn drop(&mut self) {
        let _ = self.tx.send(ModCmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Docker,
    Wsl,
    Gpu,
}

#[derive(Default)]
pub struct Updates {
    pub docker: Option<Arc<ModuleSnapshot<docker::DockerData>>>,
    pub wsl: Option<Arc<ModuleSnapshot<wsl::WslData>>>,
    pub gpu: Option<Arc<ModuleSnapshot<gpu::GpuData>>>,
}

/// Owns the module workers. Creates NOTHING until the Dev tab is first shown.
pub struct ModuleSet {
    cfg: Config,
    docker: Option<ModuleHandle<docker::DockerData>>,
    wsl: Option<ModuleHandle<wsl::WslData>>,
    gpu: Option<ModuleHandle<gpu::GpuData>>,
    started: bool,
}

impl ModuleSet {
    pub fn new(cfg: &Config) -> Self {
        Self {
            cfg: cfg.clone(),
            docker: None,
            wsl: None,
            gpu: None,
            started: false,
        }
    }

    fn start(&mut self) {
        self.started = true;
        let m = &self.cfg.modules;
        if m.docker {
            self.docker = Some(docker::spawn());
        }
        if m.wsl {
            self.wsl = Some(wsl::spawn(self.cfg.wsl.tracked_vhdx_paths.clone()));
        }
        if m.gpu {
            self.gpu = Some(gpu::spawn());
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if visible && !self.started {
            self.start();
        }
        if let Some(h) = &self.docker {
            h.send(ModCmd::Visible(visible));
        }
        if let Some(h) = &self.wsl {
            h.send(ModCmd::Visible(visible));
        }
        if let Some(h) = &self.gpu {
            h.send(ModCmd::Visible(visible));
        }
    }

    pub fn refresh(&self, kind: ModuleKind, extra: bool) {
        let c = if extra {
            ModCmd::Extra
        } else {
            ModCmd::Refresh
        };
        match kind {
            ModuleKind::Docker => self.docker.as_ref().map(|h| h.send(c)),
            ModuleKind::Wsl => self.wsl.as_ref().map(|h| h.send(c)),
            ModuleKind::Gpu => self.gpu.as_ref().map(|h| h.send(c)),
        };
    }

    pub fn take(&self) -> Updates {
        Updates {
            docker: self.docker.as_ref().and_then(|h| h.take_latest()),
            wsl: self.wsl.as_ref().and_then(|h| h.take_latest()),
            gpu: self.gpu.as_ref().and_then(|h| h.take_latest()),
        }
    }

    pub fn started(&self) -> bool {
        self.started
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn wait_for<T: Clone + Send + Sync + 'static>(
        h: &ModuleHandle<T>,
        n: usize,
    ) -> Vec<Arc<ModuleSnapshot<T>>> {
        let mut v = Vec::new();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while v.len() < n && std::time::Instant::now() < end {
            if let Some(s) = h.take_latest() {
                v.push(s);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        v
    }

    #[test]
    fn does_not_poll_while_hidden_and_polls_when_visible() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let h = ModuleHandle::spawn("t", Duration::from_millis(50), move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
            Ok(1u32)
        });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "hidden => zero polls");
        h.send(ModCmd::Visible(true));
        let got = wait_for(&h, 1);
        assert_eq!(got[0].status, ModStatus::Ok);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            calls.load(Ordering::SeqCst) >= 3,
            "keeps polling at its cadence"
        );
        h.send(ModCmd::Visible(false));
        std::thread::sleep(Duration::from_millis(100));
        let frozen = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            frozen,
            "hidden again => stops"
        );
    }

    #[test]
    fn failure_keeps_last_good_data_and_reports_status() {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let h = ModuleHandle::spawn("t", Duration::from_secs(100), move |_| {
            if n2.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok("good".to_string())
            } else {
                Err(ModStatus::Error("timeout".into()))
            }
        });
        h.send(ModCmd::Visible(true));
        let a = wait_for(&h, 1);
        assert_eq!(a[0].data.as_deref(), Some("good"));
        h.send(ModCmd::Refresh);
        let b = wait_for(&h, 1);
        assert_eq!(b[0].status, ModStatus::Error("timeout".into()));
        assert_eq!(
            b[0].data.as_deref(),
            Some("good"),
            "stale data retained, flagged by status"
        );
    }

    #[test]
    fn extra_flag_is_passed_through_and_drop_joins() {
        let saw_extra = Arc::new(AtomicUsize::new(0));
        let s2 = saw_extra.clone();
        let h = ModuleHandle::spawn("t", Duration::from_secs(100), move |extra| {
            if extra {
                s2.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        });
        h.send(ModCmd::Visible(true));
        wait_for(&h, 1);
        h.send(ModCmd::Extra);
        wait_for(&h, 1);
        assert_eq!(saw_extra.load(Ordering::SeqCst), 1);
        drop(h); // must not hang
    }

    #[test]
    fn module_set_creates_nothing_until_visible_and_nothing_when_disabled() {
        let mut cfg = Config::default();
        cfg.modules.gpu = false;
        let mut set = ModuleSet::new(&cfg);
        assert!(!set.started());
        set.set_visible(false);
        assert!(!set.started(), "hiding must not start anything");
        set.set_visible(true);
        assert!(set.started());
        assert!(
            set.docker.is_none() && set.wsl.is_none() && set.gpu.is_none(),
            "all disabled"
        );
        assert!(set.take().docker.is_none());
    }
}

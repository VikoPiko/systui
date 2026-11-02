//! Platform-independent data model. Snapshots are immutable once published.
//!
//! Metric contract: unavailable data is `None` (or an explicit state), never 0.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Stable process identity: PID alone is not enough because PIDs are reused.
/// `started_at` is seconds since the Unix epoch; `None` means the OS did not
/// report it, which reduces (does not remove) identity safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessId {
    pub pid: u32,
    pub started_at: Option<u64>,
}

/// Command line availability (collected lazily, only for the inspected process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdLine {
    NotRequested,
    Unavailable,
    Available(String),
}

/// Rarely-changing per-process metadata, cached and shared between snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessStatic {
    pub exe: Option<String>,
    pub cmd: CmdLine,
}

#[derive(Debug, Clone)]
pub struct ProcessSample {
    pub id: ProcessId,
    pub parent: Option<u32>,
    pub name: String,
    /// Percent of ONE logical CPU (can exceed 100). `None` while warming up.
    pub cpu: Option<f32>,
    /// Resident set (Linux) / working set (Windows), in bytes. `None` if restricted.
    pub rss: Option<u64>,
    /// The OS denied access to this process's counters (rss/cpu are unknown, not 0).
    pub restricted: bool,
    /// `None` when the OS API does not provide a real state (e.g. Windows).
    pub status: Option<String>,
    /// (read, write) bytes/second. `None` unless the I/O columns are enabled, still
    /// warming up, or the OS denied access. On Windows this counts ALL I/O (files, pipes, network).
    pub io: Option<(f64, f64)>,
    pub info: Arc<ProcessStatic>,
}

#[derive(Debug, Clone, Default)]
pub struct CpuMetrics {
    /// Aggregate busy %, 0-100. `None` until the second sample.
    pub usage: Option<f32>,
    pub per_core: Vec<f32>,
    pub logical: usize,
    pub brand: String,
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryMetrics {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub arch: &'static str,
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct NetIface {
    pub name: String,
    pub state: String,
    pub loopback: bool,
    /// Cumulative bytes since boot/driver start (counter, not a rate).
    pub rx_total: u64,
    pub tx_total: u64,
    /// Bytes/second between the last two samples; `None` if not computable.
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
    pub read_only: bool,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    pub fn used_pct(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used() as f64 * 100.0 / self.total as f64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorState {
    Ok,
    WarmingUp,
    Unsupported,
    Error,
}

#[derive(Debug, Clone)]
pub struct CollectorStatus {
    pub name: &'static str,
    pub state: CollectorState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub seq: u64,
    pub sampled_at: SystemTime,
    /// Monotonic time since the previous sample (basis of all rates).
    pub elapsed: Option<Duration>,
    pub cpu: CpuMetrics,
    pub memory: Option<MemoryMetrics>,
    pub host: HostInfo,
    pub processes: Vec<ProcessSample>,
    /// False when this sample did not refresh processes (the current view does not
    /// need them); `processes` then holds the last known list and must not be shown as live.
    pub processes_fresh: bool,
    pub network: Vec<NetIface>,
    /// Same contract as `processes_fresh`, for `network` (and its rates).
    pub network_fresh: bool,
    pub volumes: Vec<Volume>,
    /// Battery / temperature / throttling; empty where the platform exposes nothing.
    pub sensors: crate::sensors::Sensors,
    pub diagnostics: Vec<CollectorStatus>,
}

impl Snapshot {
    /// Aggregate rates over non-loopback interfaces. `None` if no interface has
    /// a computable rate yet. May include virtual adapters (documented).
    pub fn net_totals(&self) -> Option<(f64, f64)> {
        let mut any = false;
        let (mut rx, mut tx) = (0.0, 0.0);
        for i in self.network.iter().filter(|i| !i.loopback) {
            if let (Some(r), Some(t)) = (i.rx_rate, i.tx_rate) {
                any = true;
                rx += r;
                tx += t;
            }
        }
        any.then_some((rx, tx))
    }

    pub fn errors(&self) -> impl Iterator<Item = &CollectorStatus> {
        self.diagnostics
            .iter()
            .filter(|d| d.state == CollectorState::Error || d.state == CollectorState::Unsupported)
    }
}

/// How often the network counters are needed by the current view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetNeed {
    /// Not shown anywhere: do not refresh at all.
    Off,
    /// Only a headline figure: refresh at most every 2 s.
    Slow,
    /// Interface table / graphs: every sample.
    Fast,
}

/// What the collector must refresh, driven by the visible view. CPU, memory and
/// volumes are cheap and always sampled (alerts depend on them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    pub processes: bool,
    /// Per-process I/O rates (the `i` toggle in the Processes tab).
    pub process_io: bool,
    pub network: NetNeed,
}

impl Needs {
    pub const fn all() -> Self {
        Self {
            processes: true,
            process_io: false,
            network: NetNeed::Fast,
        }
    }
}

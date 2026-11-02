//! Application state and input reducer. Owns no OS handles: collection runs on
//! a worker thread and arrives via `ingest`; side effects leave via `effects`.

use crate::actions::docker::DockerAction;
use crate::actions::process::{classify, OsActions, ProcessActions, Risk};
use crate::alerts::{ActiveAlert, AlertEngine, AlertEvent, Inputs};
use crate::changes::ChangeLog;
use crate::config::{Config, CpuMode, StartView};
use crate::dev::{self, Families, FamilyStats};
use crate::domain::{Needs, NetNeed, ProcessId, Snapshot};
use crate::export::{build as build_doc, proc_doc, Redactor};
use crate::modules::docker::{valid_container_id, DockerData, LogPreview};
use crate::modules::gpu::GpuData;
use crate::modules::health::HealthSnapshot;
use crate::modules::ssh::{SshSnapshot, SshStatus};
use crate::modules::wsl::WslData;
use crate::modules::{ModuleKind, ModuleSnapshot, Updates};
use crate::palette::{self, Command, PaletteState};
use crate::processes::{ProcessView, MAX_FILTER_LEN};
use crate::recorder::Recorder;
use crate::ssh::HostSpec;
use crate::util::ring::History;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Processes,
    Network,
    Storage,
    Developer,
    Services,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Overview,
        Tab::Processes,
        Tab::Network,
        Tab::Storage,
        Tab::Developer,
        Tab::Services,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Processes => "Processes",
            Tab::Network => "Network",
            Tab::Storage => "Storage",
            Tab::Developer => "Dev",
            Tab::Services => "Services",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    Secondary,
}

/// Terminate-process confirmation. The target is captured when the dialog
/// opens and never follows the table selection afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillDialog {
    pub target: ProcessId,
    pub name: String,
    pub exe: Option<String>,
    pub risk: Risk,
    pub can_graceful: bool,
    pub force: bool,
    pub typed: String,
    pub require_typing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Help,
    Filter,
    Palette(PaletteState),
    /// "What changed?" overlay; the value indexes `changes::WINDOWS`.
    Changes(usize),
    DockerLogs(Box<LogsView>),
    DockerConfirm(Box<DockerDialog>),
    SshConsent(Box<ConsentDialog>),
    Kill(Box<KillDialog>),
}

/// Container log preview overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsView {
    pub id: String,
    pub name: String,
    pub lines: Vec<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub truncated: bool,
    pub scroll: usize,
}

/// Typed confirmation for stop/restart. The target is fixed when the dialog opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerDialog {
    pub action: DockerAction,
    pub id: String,
    pub name: String,
    pub image: String,
    pub running: bool,
    pub typed: String,
}

/// Per-session consent to run the fixed read-only script on one allowlisted SSH host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentDialog {
    pub name: String,
    pub dest: String,
    pub command: String,
    pub typed: String,
}

/// Result of a short-lived background task started by an effect.
pub enum TaskResult {
    Logs(String, Result<LogPreview, String>),
    Action(Result<String, String>),
}

/// Side effects requested by the reducer; the main loop forwards them to the
/// collector thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    RefreshNow,
    Watch(Option<ProcessId>),
    /// Tell the collector what the visible view needs (saves CPU on other views).
    Needs(Needs),
    /// The Dev tab became visible / hidden (module workers poll only while visible).
    DevVisible(bool),
    /// Refresh one module now; `true` also runs its expensive extras (docker system df).
    ModuleRefresh(ModuleKind, bool),
    /// Run all health checks now (Services tab `R`).
    HealthCheckNow,
    /// The Services tab became visible / hidden (SSH polls only while visible).
    SshVisible(bool),
    /// The user consented to contact this allowlisted host for this session.
    SshConsent(String),
    SshCheckNow,
    /// Collect listening sockets once (Network tab `L`); runs off the UI thread.
    CollectListeners,
    /// Read the last lines of one container's log (off the UI thread).
    DockerLogs {
        id: String,
        lines: usize,
    },
    /// Stop or restart one container the user confirmed.
    DockerAction {
        action: DockerAction,
        id: String,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageSort {
    Used,
    Free,
    Mount,
}

pub struct Histories {
    pub cpu: History,
    pub mem: History,
    pub net_rx: History,
    pub net_tx: History,
    pub per_iface: HashMap<String, (History, History)>,
    cap: usize,
}

impl Histories {
    fn new(cap: usize) -> Self {
        Self {
            cpu: History::new(cap),
            mem: History::new(cap),
            net_rx: History::new(cap),
            net_tx: History::new(cap),
            per_iface: HashMap::new(),
            cap,
        }
    }
}

/// On-demand listening-ports view (Network tab `L`). Never polled.
pub struct PortsView {
    pub open: bool,
    pub loading: bool,
    pub data: Option<Result<crate::ports::Listeners, String>>,
    pub collected_at: Option<std::time::SystemTime>,
    pub table: TableState,
}

pub struct NetView {
    pub selected: Option<String>,
    pub table: TableState,
}

pub struct StorageView {
    pub sort: StorageSort,
    pub table: TableState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevPane {
    Families,
    Docker,
    Wsl,
    Gpu,
}

/// Latest snapshot of an optional module plus when we received it.
pub struct ModView<T> {
    pub snap: Option<Arc<ModuleSnapshot<T>>>,
    pub received_at: Option<Instant>,
}

impl<T> Default for ModView<T> {
    fn default() -> Self {
        Self {
            snap: None,
            received_at: None,
        }
    }
}

pub struct DevState {
    pub families: Arc<Families>,
    pub stats: Vec<FamilyStats>,
    pub family_table: TableState,
    pub pane: DevPane,
    pub docker: ModView<DockerData>,
    pub wsl: ModView<WslData>,
    pub gpu: ModView<GpuData>,
    pub docker_table: TableState,
}

struct PendingKill {
    id: ProcessId,
    name: String,
    seen: u8,
}

pub struct App {
    pub tab: Tab,
    pub focus: Focus,
    pub mode: Mode,
    pub paused: bool,
    pub should_quit: bool,
    /// Transient one-line notice shown in the footer until the next key.
    pub notice: Option<String>,
    pub config: Config,
    pub snapshot: Option<Arc<Snapshot>>,
    pub received_at: Option<Instant>,
    pending: Option<Arc<Snapshot>>,
    pub hist: Histories,
    pub procs: ProcessView,
    pub net: NetView,
    pub ports: PortsView,
    pub storage: StorageView,
    pub dev: DevState,
    dev_visible_sent: bool,
    needs_sent: Option<Needs>,
    /// Per-process I/O columns (`i` in the Processes tab).
    pub proc_io: bool,
    pub alerts: AlertEngine,
    pub changes: ChangeLog,
    pub recorder: Option<Recorder>,
    rec_last: Option<f64>,
    /// Time of the latest sample; key-triggered actions use it so they share the sampling clock.
    last_now: Option<Instant>,
    redactor: Redactor,
    /// Recent alert events, newest last (bounded).
    pub alert_log: VecDeque<String>,
    pub health: Option<Arc<HealthSnapshot>>,
    pub health_received_at: Option<Instant>,
    pub services_table: TableState,
    pub ssh: Option<Arc<SshSnapshot>>,
    pub ssh_table: TableState,
    pub ssh_specs: Vec<HostSpec>,
    ssh_visible_sent: bool,
    live_snapshot: Option<Arc<Snapshot>>,
    t0: Option<Instant>,
    pub overview_by_mem: bool,
    pub overview_top: Vec<usize>,
    effects: Vec<Effect>,
    watched: Option<ProcessId>,
    pending_kill: Option<PendingKill>,
    actions: Box<dyn ProcessActions>,
    /// Where `x` writes process exports (current directory by default).
    pub export_dir: PathBuf,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self::with_actions(config, Box::new(OsActions))
    }

    pub fn with_actions(config: Config, actions: Box<dyn ProcessActions>) -> Self {
        let tab = match config.ui.start_view {
            StartView::Overview => Tab::Overview,
            StartView::Processes => Tab::Processes,
            StartView::Network => Tab::Network,
            StartView::Storage => Tab::Storage,
        };
        let cap = config.ui.history_samples;
        let families = Arc::new(Families::new(
            &config.developer.process_families,
            &config.developer.aliases,
        ));
        Self {
            tab,
            focus: Focus::Primary,
            mode: Mode::Normal,
            paused: false,
            should_quit: false,
            notice: None,
            snapshot: None,
            received_at: None,
            pending: None,
            hist: Histories::new(cap),
            procs: ProcessView::new(config.processes.default_sort, cap),
            net: NetView {
                selected: None,
                table: TableState::default(),
            },
            ports: PortsView {
                open: false,
                loading: false,
                data: None,
                collected_at: None,
                table: TableState::default(),
            },
            storage: StorageView {
                sort: StorageSort::Used,
                table: TableState::default(),
            },
            dev: DevState {
                families: families.clone(),
                stats: Vec::new(),
                family_table: TableState::default(),
                pane: DevPane::Families,
                docker: ModView::default(),
                wsl: ModView::default(),
                gpu: ModView::default(),
                docker_table: TableState::default(),
            },
            dev_visible_sent: false,
            needs_sent: None,
            proc_io: false,
            alerts: AlertEngine::new(config.alerts.clone()),
            changes: ChangeLog::default(),
            recorder: None,
            rec_last: None,
            last_now: None,
            redactor: Redactor::from_env(),
            alert_log: VecDeque::new(),
            health: None,
            health_received_at: None,
            services_table: TableState::default(),
            ssh: None,
            ssh_table: TableState::default(),
            ssh_specs: if config.modules.ssh {
                config
                    .ssh
                    .hosts
                    .iter()
                    .filter_map(|h| HostSpec::from_config(h).ok())
                    .collect()
            } else {
                Vec::new()
            },
            ssh_visible_sent: false,
            live_snapshot: None,
            t0: None,
            overview_by_mem: false,
            overview_top: Vec::new(),
            effects: Vec::new(),
            watched: None,
            pending_kill: None,
            actions,
            export_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            config,
        }
    }

    /// Whether the per-process I/O columns are shown (not in the compact layout).
    pub fn procs_io_visible(&self, compact: bool) -> bool {
        self.proc_io && !compact
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.sync_needs(); // also emits the initial needs on the first call
        std::mem::take(&mut self.effects)
    }

    // ------------------------------------------------------------ data in

    /// Accept a new snapshot. While paused it is held back (sampling continues
    /// on the worker; paused samples are not added to the histories).
    pub fn ingest(&mut self, snap: Arc<Snapshot>, now: Instant) {
        self.live_snapshot = Some(snap.clone());
        self.last_now = Some(now);
        self.observe_alerts(now);
        self.maybe_record(now);
        self.check_pending_kill(&snap);
        if self.paused {
            self.pending = Some(snap);
        } else {
            self.apply(snap, now);
        }
    }

    fn apply(&mut self, snap: Arc<Snapshot>, now: Instant) {
        if let Some(u) = snap.cpu.usage {
            self.hist.cpu.push(u as f64);
        }
        if let Some(m) = &snap.memory {
            self.hist.mem.push(m.used as f64 * 100.0 / m.total as f64);
        }
        if let (true, Some((rx, tx))) = (snap.network_fresh, snap.net_totals()) {
            self.hist.net_rx.push(rx);
            self.hist.net_tx.push(tx);
        }
        let cap = self.hist.cap;
        for i in &snap.network {
            let (h_rx, h_tx) = self
                .hist
                .per_iface
                .entry(i.name.clone())
                .or_insert_with(|| (History::new(cap), History::new(cap)));
            if let (true, Some(r), Some(t)) = (snap.network_fresh, i.rx_rate, i.tx_rate) {
                h_rx.push(r);
                h_tx.push(t);
            }
        }
        self.hist
            .per_iface
            .retain(|k, _| snap.network.iter().any(|i| &i.name == k));

        if snap.processes_fresh {
            self.procs.rebuild(&snap.processes);
            self.procs.record_history(&snap.processes);
            let t = self.engine_time(now);
            self.changes.push(t, &snap.processes);
        }
        self.sync_net_selection(&snap);
        self.snapshot = Some(snap);
        self.received_at = Some(now);
        if self.tab == Tab::Developer && self.snapshot.as_ref().is_some_and(|s| s.processes_fresh) {
            self.refresh_dev_stats();
        }
        self.recompute_overview();
        self.sync_watch();
    }

    fn sync_net_selection(&mut self, snap: &Snapshot) {
        let keep = self
            .net
            .selected
            .as_ref()
            .and_then(|n| snap.network.iter().position(|i| &i.name == n));
        let idx = keep.or_else(|| {
            // Interface vanished or none chosen yet: first non-loopback, else first.
            snap.network
                .iter()
                .position(|i| !i.loopback)
                .or((!snap.network.is_empty()).then_some(0))
        });
        self.net.selected = idx.map(|i| snap.network[i].name.clone());
        self.net.table.select(idx);
    }

    pub fn recompute_overview(&mut self) {
        let Some(s) = &self.snapshot else { return };
        let mut idx: Vec<usize> = (0..s.processes.len()).collect();
        let by_mem = self.overview_by_mem;
        idx.sort_by(|&a, &b| {
            let (pa, pb) = (&s.processes[a], &s.processes[b]);
            let o = if by_mem {
                pb.rss.cmp(&pa.rss)
            } else {
                // Sort-only key: unknown CPU ranks below any real reading.
                pb.cpu
                    .unwrap_or(-1.0)
                    .partial_cmp(&pa.cpu.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            };
            o.then(pa.id.pid.cmp(&pb.id.pid))
        });
        idx.truncate(10);
        self.overview_top = idx;
    }

    fn check_pending_kill(&mut self, snap: &Snapshot) {
        let Some(p) = self.pending_kill.as_mut() else {
            return;
        };
        if !snap.processes_fresh {
            return;
        }
        let still = snap.processes.iter().any(|s| s.id == p.id);
        if !still {
            self.notice = Some(format!("{} (PID {}) has exited.", p.name, p.id.pid));
            self.pending_kill = None;
        } else {
            p.seen += 1;
            if p.seen >= 2 {
                self.notice = Some(format!(
                    "{} (PID {}) is still running after the signal.",
                    p.name, p.id.pid
                ));
                self.pending_kill = None;
            }
        }
    }

    // ------------------------------------------------------------- queries

    pub fn sample_age(&self, now: Instant) -> Option<Duration> {
        self.received_at.map(|t| now.saturating_duration_since(t))
    }

    /// Data is stale when no snapshot arrived for several refresh intervals.
    pub fn is_stale(&self, now: Instant) -> bool {
        let limit = Duration::from_millis(self.config.ui.refresh_ms * 3 + 1000);
        !self.paused && self.sample_age(now).is_some_and(|a| a > limit)
    }

    /// Convert a raw per-core-equivalent process CPU value to the configured mode.
    pub fn cpu_display(&self, raw: f32) -> f32 {
        match self.config.processes.cpu_mode {
            CpuMode::PerCoreEquivalent => raw,
            CpuMode::HostShare => {
                let n = self.snapshot.as_ref().map_or(1, |s| s.cpu.logical.max(1));
                raw / n as f32
            }
        }
    }

    /// What the collector must refresh for the visible view. CPU, memory and volumes
    /// are always sampled; processes and network only where they are shown.
    pub fn desired_needs(&self) -> Needs {
        let (processes, network) = match self.tab {
            Tab::Overview => (true, NetNeed::Slow),
            Tab::Processes | Tab::Developer => (true, NetNeed::Off),
            Tab::Network => (self.ports.open, NetNeed::Fast),
            Tab::Storage | Tab::Services => (false, NetNeed::Off),
        };
        Needs {
            processes,
            process_io: self.proc_io && self.tab == Tab::Processes,
            network,
        }
    }

    fn sync_needs(&mut self) {
        let want = self.desired_needs();
        if self.needs_sent != Some(want) {
            self.needs_sent = Some(want);
            self.effects.push(Effect::Needs(want));
        }
    }

    fn sync_dev_visible(&mut self) {
        let ssh_v = self.tab == Tab::Services && !self.ssh_specs.is_empty();
        if ssh_v != self.ssh_visible_sent {
            self.ssh_visible_sent = ssh_v;
            self.effects.push(Effect::SshVisible(ssh_v));
        }
        let v = self.tab == Tab::Developer;
        if v != self.dev_visible_sent {
            self.dev_visible_sent = v;
            self.effects.push(Effect::DevVisible(v));
        }
    }

    fn sync_watch(&mut self) {
        self.sync_needs();
        self.sync_dev_visible();
        let want = if self.tab == Tab::Processes && self.procs.detail_open {
            self.procs.selected
        } else {
            None
        };
        if want != self.watched {
            self.watched = want;
            self.effects.push(Effect::Watch(want));
        }
    }

    // --------------------------------------------------------------- input

    pub fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.focus = Focus::Primary;
        if tab == Tab::Developer {
            self.refresh_dev_stats();
        }
        self.sync_dev_visible();
        self.sync_needs();
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Primary => Focus::Secondary,
            Focus::Secondary => Focus::Primary,
        };
    }

    fn set_paused(&mut self, paused: bool, now: Instant) {
        self.paused = paused;
        if !paused {
            if let Some(s) = self.pending.take() {
                self.apply(s, now);
            }
        }
    }

    /// Handle a key event. Returns true when the screen needs a redraw.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        self.notice = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return true;
        }
        if ctrl && key.code == KeyCode::Char('k') {
            match self.mode {
                Mode::Normal => self.mode = Mode::Palette(PaletteState::default()),
                Mode::Palette(_) => self.mode = Mode::Normal,
                _ => {}
            }
            return true;
        }
        if ctrl && key.code == KeyCode::Char('r') {
            if self.mode == Mode::Normal {
                self.toggle_recording(self.app_now());
            }
            return true;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Palette(p) => self.palette_key(key, p),
            Mode::Changes(i) => self.changes_key(key, i),
            Mode::DockerLogs(v) => self.logs_key(key, v),
            Mode::DockerConfirm(d) => self.docker_confirm_key(key, d),
            Mode::SshConsent(d) => self.ssh_consent_key(key, d),
            Mode::Help => {
                if !matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.mode = Mode::Help;
                }
            }
            Mode::Filter => self.filter_key(key),
            Mode::Kill(d) => self.kill_key(key, d),
            Mode::Normal => {
                self.normal_key(key);
            }
        }
        self.sync_watch();
        true
    }

    fn filter_key(&mut self, key: KeyEvent) {
        let procs = self.procs_slice();
        match key.code {
            KeyCode::Esc => {
                self.procs.filter.clear();
            }
            KeyCode::Enter => {}
            KeyCode::Backspace => {
                self.procs.filter.pop();
                self.mode = Mode::Filter;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && self.procs.filter.chars().count() < MAX_FILTER_LEN =>
            {
                self.procs.filter.push(c);
                self.mode = Mode::Filter;
            }
            _ => self.mode = Mode::Filter,
        }
        self.rebuild_procs(procs);
    }

    fn procs_slice(&self) -> Option<Arc<Snapshot>> {
        self.snapshot.clone()
    }

    fn rebuild_procs(&mut self, snap: Option<Arc<Snapshot>>) {
        if let Some(s) = snap {
            self.procs.rebuild(&s.processes);
        }
    }

    fn normal_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return; // unbound Ctrl combos must not fall through to plain keys
        }
        let now = Instant::now();
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('w') => self.mode = Mode::Changes(1),
            KeyCode::Char(' ') => self.set_paused(!self.paused, now),
            KeyCode::Char('r') => {
                self.effects.push(Effect::RefreshNow);
                self.notice = Some("refresh requested".into());
            }
            KeyCode::Char(c @ '1'..='6') => self.select_tab(Tab::ALL[c as usize - '1' as usize]),
            KeyCode::Tab | KeyCode::BackTab if self.tab == Tab::Developer => {
                self.cycle_dev_pane(key.code == KeyCode::BackTab)
            }
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            _ => match self.tab {
                Tab::Developer => self.dev_key(key),
                Tab::Services => self.services_key(key),
                Tab::Overview => self.overview_key(key),
                Tab::Processes => self.process_key(key),
                Tab::Network => self.network_key(key),
                Tab::Storage => self.storage_key(key),
            },
        }
    }

    fn overview_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('s') {
            self.overview_by_mem = !self.overview_by_mem;
            self.recompute_overview();
        }
    }

    fn network_key(&mut self, key: KeyEvent) {
        if self.ports_key(key) {
            return;
        }
        let Some(s) = self.snapshot.clone() else {
            return;
        };
        if s.network.is_empty() {
            return;
        }
        let cur = self.net.table.selected().unwrap_or(0);
        let last = s.network.len() - 1;
        let new = match key.code {
            KeyCode::Down | KeyCode::Char('j') => (cur + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => cur.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => last,
            _ => return,
        };
        self.net.table.select(Some(new));
        self.net.selected = Some(s.network[new].name.clone());
    }

    fn storage_key(&mut self, key: KeyEvent) {
        let n = self.snapshot.as_ref().map_or(0, |s| s.volumes.len());
        let cur = self.storage.table.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('s') => {
                self.storage.sort = match self.storage.sort {
                    StorageSort::Used => StorageSort::Free,
                    StorageSort::Free => StorageSort::Mount,
                    StorageSort::Mount => StorageSort::Used,
                }
            }
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.storage.table.select(Some((cur + 1).min(n - 1)))
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.storage.table.select(Some(cur.saturating_sub(1)))
            }
            KeyCode::Home if n > 0 => self.storage.table.select(Some(0)),
            KeyCode::End if n > 0 => self.storage.table.select(Some(n - 1)),
            _ => {}
        }
    }

    fn process_key(&mut self, key: KeyEvent) {
        let Some(s) = self.snapshot.clone() else {
            if key.code == KeyCode::Char('/') {
                self.mode = Mode::Filter;
            }
            return;
        };
        let procs = &s.processes;
        let page = self.procs.page.max(1) as isize;
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.procs.move_by(1, procs),
            KeyCode::Up | KeyCode::Char('k') => self.procs.move_by(-1, procs),
            KeyCode::PageDown => self.procs.move_by(page, procs),
            KeyCode::PageUp => self.procs.move_by(-page, procs),
            KeyCode::Home | KeyCode::Char('g') => self.procs.move_to(0, procs),
            KeyCode::End | KeyCode::Char('G') => self.procs.move_to(usize::MAX, procs),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('s') => self.procs.cycle_sort(procs),
            KeyCode::Char('S') => self.procs.toggle_reverse(procs),
            KeyCode::Enter => {
                if self.procs.selected.is_some() {
                    self.procs.detail_open = !self.procs.detail_open;
                    self.procs.reveal_cmd = false;
                }
            }
            KeyCode::Esc => {
                if self.procs.detail_open {
                    self.procs.detail_open = false;
                    self.procs.reveal_cmd = false;
                } else if !self.procs.filter.is_empty() {
                    self.procs.filter.clear();
                    self.procs.rebuild(procs);
                } else if self.procs.family.is_some() {
                    self.procs.family = None;
                    self.procs.rebuild(procs);
                }
            }
            KeyCode::Char('e') if self.procs.detail_open => {
                self.procs.reveal_cmd = !self.procs.reveal_cmd;
            }
            KeyCode::Char('p') => self.jump_parent(&s),
            KeyCode::Char('c') => self.jump_child(&s),
            KeyCode::Char('t') => {
                self.procs.toggle_tree(procs);
                self.notice = Some(if self.procs.tree {
                    "tree view on (Left/Right collapse/expand; paused while filtering)".into()
                } else {
                    "tree view off".into()
                });
            }
            KeyCode::Left | KeyCode::Char('h') => self.procs.tree_left(procs),
            KeyCode::Right | KeyCode::Char('l') => self.procs.tree_right(procs),
            KeyCode::Char('i') => {
                self.proc_io = !self.proc_io;
                self.notice = Some(if self.proc_io {
                    "per-process I/O on (needs one more sample; Windows counts all I/O incl. network)".into()
                } else {
                    "per-process I/O off".into()
                });
            }
            KeyCode::Char('K') => self.open_kill_dialog(&s),
            KeyCode::Char('x') => self.export_selected(&s),
            _ => {}
        }
        self.procs.record_history(procs);
    }

    fn jump_to_pid(&mut self, s: &Snapshot, pid: u32) -> bool {
        let pos = self
            .procs
            .rows
            .iter()
            .position(|&i| s.processes[i].id.pid == pid);
        match pos {
            Some(p) => {
                self.procs.move_to(p, &s.processes);
                true
            }
            None => false,
        }
    }

    fn jump_parent(&mut self, s: &Snapshot) {
        let Some(sel) = self.procs.selected_sample(&s.processes) else {
            return;
        };
        match sel.parent {
            None => self.notice = Some("no parent reported for this process".into()),
            Some(pp) => {
                if !self.jump_to_pid(s, pp) {
                    self.notice = Some(format!(
                        "parent PID {pp} is not in the list (exited, hidden by filter, or not visible)"
                    ));
                }
            }
        }
    }

    fn jump_child(&mut self, s: &Snapshot) {
        let Some(sel) = self.procs.selected_sample(&s.processes) else {
            return;
        };
        let pid = sel.id.pid;
        let kids: Vec<u32> = s
            .processes
            .iter()
            .filter(|p| p.parent == Some(pid) && p.id.pid != pid)
            .map(|p| p.id.pid)
            .collect();
        match kids.iter().min() {
            None => self.notice = Some("no child processes".into()),
            Some(&k) => {
                if self.jump_to_pid(s, k) {
                    if kids.len() > 1 {
                        self.notice = Some(format!("{} children; jumped to PID {k}", kids.len()));
                    }
                } else {
                    self.notice = Some(format!("child PID {k} is hidden by the filter"));
                }
            }
        }
    }

    fn open_kill_dialog(&mut self, s: &Snapshot) {
        let Some(p) = self.procs.selected_sample(&s.processes) else {
            return;
        };
        if p.id.pid == std::process::id() {
            self.notice = Some("systui will not terminate itself".into());
            return;
        }
        let risk = classify(p.id.pid, &p.name);
        if let Risk::Protected(why) = &risk {
            self.notice = Some(format!("{} is protected: {why}", p.name));
            return;
        }
        let can_graceful = self.actions.can_graceful();
        self.mode = Mode::Kill(Box::new(KillDialog {
            target: p.id,
            name: p.name.clone(),
            exe: p.info.exe.clone(),
            risk,
            can_graceful,
            force: !can_graceful,
            typed: String::new(),
            require_typing: self.config.processes.confirm_terminate,
        }));
    }

    fn kill_key(&mut self, key: KeyEvent, mut d: Box<KillDialog>) {
        match key.code {
            KeyCode::Esc => {
                self.notice = Some("termination cancelled".into());
                return;
            }
            KeyCode::Char('f') if d.can_graceful && !d.force => {
                d.force = true;
                d.typed.clear();
            }
            KeyCode::Backspace => {
                d.typed.pop();
            }
            KeyCode::Char('y') if !d.require_typing => return self.execute_kill(&d),
            KeyCode::Char(c) if d.require_typing && c.is_ascii_digit() && d.typed.len() < 10 => {
                d.typed.push(c)
            }
            KeyCode::Enter if d.require_typing && d.typed == d.target.pid.to_string() => {
                return self.execute_kill(&d);
            }
            _ => {}
        }
        self.mode = Mode::Kill(d);
    }

    fn execute_kill(&mut self, d: &KillDialog) {
        match self.actions.terminate(&d.target, &d.name, d.force) {
            Ok(()) => {
                self.notice = Some(format!(
                    "{} sent to {} (PID {}); confirming exit…",
                    if d.force { "Kill" } else { "Terminate request" },
                    d.name,
                    d.target.pid
                ));
                self.pending_kill = Some(PendingKill {
                    id: d.target,
                    name: d.name.clone(),
                    seen: 0,
                });
                self.effects.push(Effect::RefreshNow);
            }
            Err(e) => self.notice = Some(format!("Not terminated: {e}")),
        }
    }
}

// ------------------------------------------------------- palette & export
impl App {
    fn palette_key(&mut self, key: KeyEvent, mut p: PaletteState) {
        let n = palette::search(&p.query).len();
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                let cmd = palette::search(&p.query).get(p.selected).map(|s| s.cmd);
                if let Some(c) = cmd {
                    self.run_command(c);
                }
                return;
            }
            KeyCode::Down => p.selected = (p.selected + 1).min(n.saturating_sub(1)),
            KeyCode::Up => p.selected = p.selected.saturating_sub(1),
            KeyCode::Backspace => {
                p.query.pop();
                p.selected = 0;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && p.query.chars().count() < 40 =>
            {
                p.query.push(c);
                p.selected = 0;
            }
            _ => {}
        }
        self.mode = Mode::Palette(p);
    }

    /// Execute a palette command. Only the fixed `Command` set exists.
    pub fn run_command(&mut self, c: Command) {
        let snap = self.snapshot.clone();
        let now = self.app_now();
        match c {
            Command::GoOverview => self.select_tab(Tab::Overview),
            Command::GoProcesses => self.select_tab(Tab::Processes),
            Command::GoNetwork => self.select_tab(Tab::Network),
            Command::GoStorage => self.select_tab(Tab::Storage),
            Command::GoDeveloper => self.select_tab(Tab::Developer),
            Command::GoServices => self.select_tab(Tab::Services),
            Command::TogglePause => self.set_paused(!self.paused, now),
            Command::RefreshNow => {
                self.effects.push(Effect::RefreshNow);
                self.notice = Some("refresh requested".into());
            }
            Command::Help => self.mode = Mode::Help,
            Command::WhatChanged => self.mode = Mode::Changes(1),
            Command::ToggleRecording => self.toggle_recording(now),
            Command::Quit => self.should_quit = true,
            Command::ToggleTopBy => {
                self.select_tab(Tab::Overview);
                self.overview_by_mem = !self.overview_by_mem;
                self.recompute_overview();
            }
            Command::FilterProcesses => {
                self.select_tab(Tab::Processes);
                self.mode = Mode::Filter;
            }
            Command::CycleSort
            | Command::ReverseSort
            | Command::TerminateSelected
            | Command::ExportProcess => {
                self.select_tab(Tab::Processes);
                let Some(s) = snap else {
                    self.notice = Some("no data yet".into());
                    return;
                };
                match c {
                    Command::CycleSort => self.procs.cycle_sort(&s.processes),
                    Command::ReverseSort => self.procs.toggle_reverse(&s.processes),
                    Command::TerminateSelected => self.open_kill_dialog(&s),
                    _ => self.export_selected(&s),
                }
            }
        }
    }

    /// Write sanitized details (no command line, redacted paths) of the selected
    /// process. Never overwrites an existing file.
    fn export_selected(&mut self, s: &Snapshot) {
        let Some(p) = self.procs.selected_sample(&s.processes) else {
            self.notice = Some("no process selected".into());
            return;
        };
        let doc = proc_doc(p, &Redactor::from_env());
        let unix = s
            .sampled_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = self
            .export_dir
            .join(format!("systui-process-{}-{unix}.json", p.id.pid));
        let text = match serde_json::to_string_pretty(&doc) {
            Ok(t) => t + "\n",
            Err(e) => {
                self.notice = Some(format!("export failed: {e}"));
                return;
            }
        };
        self.notice = Some(match crate::commands::write_output(&path, &text, false) {
            Ok(()) => format!("exported (sanitized) to {}", path.display()),
            Err(e) => format!("export failed: {e}"),
        });
    }
}

// ------------------------------------------------------------- Dev tab
impl App {
    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    pub fn refresh_dev_stats(&mut self) {
        let Some(s) = self.snapshot.clone() else {
            return;
        };
        self.dev.stats = dev::summarize(&self.dev.families, &s.processes, Self::now_unix());
        let n = self.dev.stats.len();
        let cur = self.dev.family_table.selected();
        self.dev.family_table.select(if n == 0 {
            None
        } else {
            Some(cur.unwrap_or(0).min(n - 1))
        });
    }

    pub fn ingest_modules(&mut self, u: Updates, now: Instant) {
        if let Some(s) = u.docker {
            let n = s.data.as_ref().map_or(0, |d| d.containers.len());
            let cur = self.dev.docker_table.selected().unwrap_or(0);
            self.dev
                .docker_table
                .select(if n == 0 { None } else { Some(cur.min(n - 1)) });
            self.dev.docker = ModView {
                snap: Some(s),
                received_at: Some(now),
            };
        }
        if let Some(s) = u.wsl {
            self.dev.wsl = ModView {
                snap: Some(s),
                received_at: Some(now),
            };
        }
        if let Some(s) = u.gpu {
            self.dev.gpu = ModView {
                snap: Some(s),
                received_at: Some(now),
            };
        }
    }

    fn cycle_dev_pane(&mut self, back: bool) {
        use DevPane::*;
        let order = [Families, Docker, Wsl, Gpu];
        let i = order.iter().position(|p| *p == self.dev.pane).unwrap_or(0);
        let n = order.len();
        self.dev.pane = order[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    fn dev_key(&mut self, key: KeyEvent) {
        let (list_len, table) = match self.dev.pane {
            DevPane::Families => (self.dev.stats.len(), &mut self.dev.family_table),
            DevPane::Docker => (
                self.dev
                    .docker
                    .snap
                    .as_ref()
                    .and_then(|s| s.data.as_ref())
                    .map_or(0, |d| d.containers.len()),
                &mut self.dev.docker_table,
            ),
            _ => (0, &mut self.dev.family_table),
        };
        if list_len > 0 && matches!(self.dev.pane, DevPane::Families | DevPane::Docker) {
            let cur = table.selected().unwrap_or(0);
            let new = match key.code {
                KeyCode::Down | KeyCode::Char('j') => Some((cur + 1).min(list_len - 1)),
                KeyCode::Up | KeyCode::Char('k') => Some(cur.saturating_sub(1)),
                KeyCode::Home | KeyCode::Char('g') => Some(0),
                KeyCode::End | KeyCode::Char('G') => Some(list_len - 1),
                _ => None,
            };
            if let Some(n) = new {
                table.select(Some(n));
                return;
            }
        }
        match key.code {
            KeyCode::Enter if self.dev.pane == DevPane::Families => self.drill_into_family(),
            KeyCode::Char('l') if self.dev.pane == DevPane::Docker => self.open_docker_logs(),
            KeyCode::Char('S') if self.dev.pane == DevPane::Docker => {
                self.open_docker_dialog(DockerAction::Stop)
            }
            KeyCode::Char('X') if self.dev.pane == DevPane::Docker => {
                self.open_docker_dialog(DockerAction::Restart)
            }
            KeyCode::Char('R') => {
                let (kind, extra, enabled) = match self.dev.pane {
                    DevPane::Docker => (ModuleKind::Docker, true, self.config.modules.docker),
                    DevPane::Wsl => (ModuleKind::Wsl, false, self.config.modules.wsl),
                    DevPane::Gpu => (ModuleKind::Gpu, false, self.config.modules.gpu),
                    DevPane::Families => {
                        self.notice = Some(
                            "R refreshes the focused Docker/WSL/GPU panel (Tab to focus one)"
                                .into(),
                        );
                        return;
                    }
                };
                if enabled {
                    self.effects.push(Effect::ModuleRefresh(kind, extra));
                    self.notice = Some(if extra {
                        "refreshing Docker incl. `docker system df`…".into()
                    } else {
                        "refresh requested".into()
                    });
                } else {
                    self.notice = Some("that module is disabled in the config".into());
                }
            }
            _ => {}
        }
    }

    fn drill_into_family(&mut self) {
        let Some(sel) = self.dev.family_table.selected() else {
            return;
        };
        let Some(st) = self.dev.stats.get(sel) else {
            return;
        };
        if st.count == 0 {
            self.notice = Some(format!("no running `{}` processes", st.name));
            return;
        }
        let (idx, name) = (st.index, st.name.clone());
        self.procs.filter.clear();
        self.procs.family = Some((self.dev.families.clone(), idx));
        self.select_tab(Tab::Processes);
        if let Some(s) = self.snapshot.clone() {
            self.procs.rebuild(&s.processes);
            self.procs.record_history(&s.processes);
        }
        self.notice = Some(format!("showing family `{name}` (Esc clears)"));
    }
}

// ------------------------------------------------- Services tab & alerts
impl App {
    fn engine_time(&mut self, now: Instant) -> f64 {
        let t0 = *self.t0.get_or_insert(now);
        now.saturating_duration_since(t0).as_secs_f64()
    }

    /// Evaluate alert rules against the newest data (display pause does not pause alerts).
    pub fn observe_alerts(&mut self, now: Instant) {
        if !self.alerts.enabled() {
            return;
        }
        let t = self.engine_time(now);
        let (cpu, mem, vols): (Option<f64>, Option<f64>, Vec<(String, f64)>) =
            match &self.live_snapshot {
                Some(s) => (
                    s.cpu.usage.map(f64::from),
                    s.memory.map(|m| m.used as f64 * 100.0 / m.total as f64),
                    s.volumes
                        .iter()
                        .filter(|v| v.total > 0)
                        .map(|v| (v.mount.clone(), v.available as f64 * 100.0 / v.total as f64))
                        .collect(),
                ),
                None => (None, None, Vec::new()),
            };
        let services: Vec<(String, u32)> = self
            .health
            .as_ref()
            .map(|h| {
                h.checks
                    .iter()
                    .filter(|c| c.enabled && c.last_checked.is_some())
                    .map(|c| (c.name.clone(), c.consecutive_failures))
                    .collect()
            })
            .unwrap_or_default();
        let events = self.alerts.observe(
            t,
            &Inputs {
                cpu_percent: cpu,
                memory_percent: mem,
                volumes: &vols,
                services: &services,
            },
        );
        for e in events {
            let stamp = crate::export::utc_string(Self::now_unix())[11..19].to_string();
            match e {
                AlertEvent::Raised(a) => {
                    let line = format!("{stamp} RAISED  {} - {}", a.key.label(), a.detail);
                    self.notice = Some(format!("ALERT: {} - {}", a.key.label(), a.detail));
                    self.push_alert_log(line);
                }
                AlertEvent::Cleared(k) => {
                    self.push_alert_log(format!("{stamp} cleared {}", k.label()));
                }
            }
        }
    }

    fn push_alert_log(&mut self, line: String) {
        if self.alert_log.len() >= 50 {
            self.alert_log.pop_front();
        }
        self.alert_log.push_back(line);
    }

    pub fn active_alerts(&self) -> Vec<&ActiveAlert> {
        self.alerts.active()
    }

    pub fn ingest_health(&mut self, h: Arc<HealthSnapshot>, now: Instant) {
        let n = h.checks.len();
        let cur = self.services_table.selected().unwrap_or(0);
        self.services_table
            .select(if n == 0 { None } else { Some(cur.min(n - 1)) });
        self.health = Some(h);
        self.health_received_at = Some(now);
        self.observe_alerts(now);
    }

    fn services_key(&mut self, key: KeyEvent) {
        let on_ssh = self.focus == Focus::Secondary && !self.ssh_specs.is_empty();
        let n = if on_ssh {
            self.ssh_specs.len()
        } else {
            self.health.as_ref().map_or(0, |h| h.checks.len())
        };
        let table = if on_ssh {
            &mut self.ssh_table
        } else {
            &mut self.services_table
        };
        let cur = table.selected().unwrap_or(0);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => table.select(Some((cur + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') if n > 0 => table.select(Some(cur.saturating_sub(1))),
            KeyCode::Home | KeyCode::Char('g') if n > 0 => table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') if n > 0 => table.select(Some(n - 1)),
            KeyCode::Char('C') => self.open_ssh_consent(),
            KeyCode::Char('R') => {
                let mut did = Vec::new();
                if self.config.modules.health && !self.config.health.checks.is_empty() {
                    self.effects.push(Effect::HealthCheckNow);
                    did.push("health checks");
                }
                if !self.ssh_specs.is_empty() {
                    self.effects.push(Effect::SshCheckNow);
                    did.push("consented SSH hosts");
                }
                self.notice = Some(if did.is_empty() {
                    "nothing to refresh: health checks and SSH hosts are not enabled in the config"
                        .into()
                } else {
                    format!("re-checking {}", did.join(" and "))
                });
            }
            _ => {}
        }
    }
}

impl App {
    fn changes_key(&mut self, key: KeyEvent, i: usize) {
        let n = crate::changes::WINDOWS.len();
        let next = match key.code {
            KeyCode::Esc | KeyCode::Char('w') | KeyCode::Char('q') => return,
            KeyCode::Right | KeyCode::Char(']') | KeyCode::Char('l') => (i + 1).min(n - 1),
            KeyCode::Left | KeyCode::Char('[') | KeyCode::Char('h') => i.saturating_sub(1),
            _ => i,
        };
        self.mode = Mode::Changes(next);
    }

    /// Seconds on the engine clock (for the "What changed?" age line).
    pub fn changes_age(&mut self, now: Instant) -> Option<f64> {
        let t = self.engine_time(now);
        self.changes.newest_age(t)
    }
}

// ------------------------------------------------------- listening ports
impl App {
    /// Handles keys for the ports view. Returns true if the key was consumed.
    fn ports_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('L') {
            self.ports.open = !self.ports.open;
            if self.ports.open {
                self.request_listeners();
            }
            return true;
        }
        if !self.ports.open {
            return false;
        }
        let n = match &self.ports.data {
            Some(Ok(l)) => l.items.len(),
            _ => 0,
        };
        let cur = self.ports.table.selected().unwrap_or(0);
        match key.code {
            KeyCode::Esc => self.ports.open = false,
            KeyCode::Char('R') | KeyCode::Char('r') => self.request_listeners(),
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.ports.table.select(Some((cur + 1).min(n - 1)))
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.ports.table.select(Some(cur.saturating_sub(1)))
            }
            KeyCode::Home | KeyCode::Char('g') if n > 0 => self.ports.table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') if n > 0 => self.ports.table.select(Some(n - 1)),
            _ => {}
        }
        true
    }

    fn request_listeners(&mut self) {
        if !self.ports.loading {
            self.ports.loading = true;
            self.effects.push(Effect::CollectListeners);
        }
    }

    pub fn ingest_ports(&mut self, r: Result<crate::ports::Listeners, String>) {
        self.ports.loading = false;
        self.ports.collected_at = Some(std::time::SystemTime::now());
        let n = r.as_ref().map_or(0, |l| l.items.len());
        let cur = self.ports.table.selected().unwrap_or(0);
        self.ports
            .table
            .select(if n == 0 { None } else { Some(cur.min(n - 1)) });
        self.ports.data = Some(r);
    }
}

// ------------------------------------------------------ session recording
impl App {
    fn app_now(&self) -> Instant {
        self.last_now.unwrap_or_else(Instant::now)
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Start or stop a recording session. Never starts on its own.
    pub fn toggle_recording(&mut self, now: Instant) {
        if let Some(r) = self.recorder.take() {
            let (lines, bytes, dropped) = r.stop();
            self.rec_last = None;
            self.notice = Some(format!(
                "recording stopped: {lines} line(s), {}{}",
                crate::util::format::bytes(bytes),
                if dropped > 0 {
                    format!(", {dropped} dropped")
                } else {
                    String::new()
                }
            ));
            return;
        }
        let cfg = self.config.recording.clone();
        if !cfg.enabled {
            self.notice =
                Some("recording is disabled: set [recording] enabled = true in the config".into());
            return;
        }
        let Some(dir) = cfg.resolved_dir() else {
            self.notice = Some("recording: cannot determine a recordings directory".into());
            return;
        };
        match Recorder::start(
            &dir,
            cfg.limits(),
            &crate::export::utc_string(Self::now_unix()),
        ) {
            Ok(r) => {
                let path = r
                    .current_file()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                self.recorder = Some(r);
                self.rec_last = None;
                self.maybe_record(now);
                self.notice = Some(format!(
                    "recording every {} s to {path} (Ctrl+R stops; kept: {} files / {} MB / {} days)",
                    cfg.interval_seconds, cfg.max_files, cfg.max_total_mb, cfg.max_age_days
                ));
            }
            Err(e) => {
                self.notice = Some(format!("cannot start recording in {}: {e}", dir.display()))
            }
        }
    }

    /// Called for every incoming sample, including while the display is paused.
    fn maybe_record(&mut self, now: Instant) {
        if self.recorder.is_none() {
            return;
        }
        if let Some(err) = self.recorder.as_ref().and_then(|r| r.error()) {
            self.recorder = None;
            self.notice = Some(format!("recording stopped: {err}"));
            return;
        }
        let t = self.engine_time(now);
        let every = self.config.recording.interval_seconds as f64;
        if self.rec_last.is_some_and(|l| t - l < every) {
            return;
        }
        let (Some(snap), Some(rec)) = (self.live_snapshot.clone(), self.recorder.as_ref()) else {
            return;
        };
        let doc = build_doc(&snap, self.config.recording.top_processes, &self.redactor);
        if let Ok(line) = serde_json::to_string(&doc) {
            rec.record(line);
            self.rec_last = Some(t);
        }
    }
}

// ------------------------------------------------ Docker logs & actions
impl App {
    fn selected_container(&self) -> Option<crate::modules::docker::Container> {
        let i = self.dev.docker_table.selected()?;
        self.dev
            .docker
            .snap
            .as_ref()?
            .data
            .as_ref()?
            .containers
            .get(i)
            .cloned()
    }

    fn open_docker_logs(&mut self) {
        if !self.config.modules.docker {
            self.notice = Some("Docker is disabled in the config".into());
            return;
        }
        let Some(c) = self.selected_container() else {
            self.notice = Some("no container selected".into());
            return;
        };
        if !valid_container_id(&c.id) {
            self.notice = Some("unexpected container id from Docker; refusing".into());
            return;
        }
        self.effects.push(Effect::DockerLogs {
            id: c.id.clone(),
            lines: self.config.docker.log_lines,
        });
        self.mode = Mode::DockerLogs(Box::new(LogsView {
            id: c.id,
            name: c.name,
            lines: Vec::new(),
            loading: true,
            error: None,
            truncated: false,
            scroll: usize::MAX, // start at the newest line
        }));
    }

    fn logs_key(&mut self, key: KeyEvent, mut v: Box<LogsView>) {
        let page = 15usize;
        let last = v.lines.len().saturating_sub(1);
        let cur = v.scroll.min(last);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('l') => return,
            KeyCode::Down | KeyCode::Char('j') => v.scroll = (cur + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => v.scroll = cur.saturating_sub(1),
            KeyCode::PageDown => v.scroll = (cur + page).min(last),
            KeyCode::PageUp => v.scroll = cur.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => v.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => v.scroll = usize::MAX,
            KeyCode::Char('r') if !v.loading => {
                v.loading = true;
                self.effects.push(Effect::DockerLogs {
                    id: v.id.clone(),
                    lines: self.config.docker.log_lines,
                });
            }
            _ => {}
        }
        self.mode = Mode::DockerLogs(v);
    }

    pub fn ingest_task(&mut self, r: TaskResult) {
        match r {
            TaskResult::Logs(id, res) => {
                if let Mode::DockerLogs(v) = &mut self.mode {
                    if v.id == id {
                        v.loading = false;
                        match res {
                            Ok(p) => {
                                v.lines = p.lines;
                                v.truncated = p.truncated;
                                v.error = None;
                                v.scroll = usize::MAX;
                            }
                            Err(e) => v.error = Some(e),
                        }
                    }
                }
            }
            TaskResult::Action(res) => {
                self.notice = Some(match res {
                    Ok(m) => format!("Docker: {m}"),
                    Err(e) => format!("Docker action failed: {e}"),
                });
                self.effects
                    .push(Effect::ModuleRefresh(ModuleKind::Docker, false));
            }
        }
    }

    fn open_docker_dialog(&mut self, action: DockerAction) {
        if !self.config.modules.docker {
            self.notice = Some("Docker is disabled in the config".into());
            return;
        }
        if !self.config.docker.allow_actions {
            self.notice = Some("container actions are disabled: set [docker] allow_actions = true (each still asks for confirmation)".into());
            return;
        }
        let Some(c) = self.selected_container() else {
            self.notice = Some("no container selected".into());
            return;
        };
        if !valid_container_id(&c.id) {
            self.notice = Some("unexpected container id from Docker; refusing".into());
            return;
        }
        let running = c.state == "running";
        if action == DockerAction::Stop && !running {
            self.notice = Some(format!("{} is not running (state: {})", c.name, c.state));
            return;
        }
        self.mode = Mode::DockerConfirm(Box::new(DockerDialog {
            action,
            id: c.id,
            name: c.name,
            image: c.image,
            running,
            typed: String::new(),
        }));
    }

    fn docker_confirm_key(&mut self, key: KeyEvent, mut d: Box<DockerDialog>) {
        match key.code {
            KeyCode::Esc => {
                self.notice = Some("container action cancelled".into());
                return;
            }
            KeyCode::Backspace => {
                d.typed.pop();
            }
            KeyCode::Enter if d.typed == d.name => {
                self.notice = Some(format!(
                    "{}ing {}…",
                    d.action.verb().trim_end_matches('e'),
                    d.name
                ));
                self.effects.push(Effect::DockerAction {
                    action: d.action,
                    id: d.id.clone(),
                    name: d.name.clone(),
                });
                return;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && d.typed.chars().count() < 80 =>
            {
                d.typed.push(c)
            }
            _ => {}
        }
        self.mode = Mode::DockerConfirm(d);
    }
}

// -------------------------------------------------------- SSH consent
impl App {
    pub fn ingest_ssh(&mut self, s: Arc<SshSnapshot>) {
        let n = s.hosts.len();
        let cur = self.ssh_table.selected().unwrap_or(0);
        self.ssh_table
            .select(if n == 0 { None } else { Some(cur.min(n - 1)) });
        self.ssh = Some(s);
    }

    fn open_ssh_consent(&mut self) {
        if !self.config.modules.ssh || self.ssh_specs.is_empty() {
            self.notice = Some(
                "SSH checks are not enabled: set [modules] ssh = true and add [[ssh.hosts]]".into(),
            );
            return;
        }
        if self.focus != Focus::Secondary {
            self.notice = Some("press Tab to focus the SSH hosts table first".into());
            return;
        }
        let i = self
            .ssh_table
            .selected()
            .unwrap_or(0)
            .min(self.ssh_specs.len() - 1);
        let spec = &self.ssh_specs[i];
        if !spec.enabled {
            self.notice = Some(format!("{} is disabled in the config", spec.name));
            return;
        }
        let status = self
            .ssh
            .as_ref()
            .and_then(|s| s.hosts.get(i))
            .map(|h| h.status.clone());
        if status.is_some_and(|s| s != SshStatus::NeedsConsent) {
            self.notice = Some(format!(
                "already consented to {} for this session",
                spec.name
            ));
            return;
        }
        self.mode = Mode::SshConsent(Box::new(ConsentDialog {
            name: spec.name.clone(),
            dest: spec.dest(),
            command: spec.remote_command(),
            typed: String::new(),
        }));
    }

    fn ssh_consent_key(&mut self, key: KeyEvent, mut d: Box<ConsentDialog>) {
        match key.code {
            KeyCode::Esc => {
                self.notice = Some("no connection was made".into());
                return;
            }
            KeyCode::Backspace => {
                d.typed.pop();
            }
            KeyCode::Enter if d.typed == d.name => {
                self.effects.push(Effect::SshConsent(d.name.clone()));
                self.notice = Some(format!(
                    "consent given for {} until you quit; connecting…",
                    d.name
                ));
                return;
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !c.is_control()
                    && d.typed.chars().count() < 60 =>
            {
                d.typed.push(c)
            }
            _ => {}
        }
        self.mode = Mode::SshConsent(d);
    }
}

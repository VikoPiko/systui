//! Persistent `sysinfo` collector: CPU, memory, processes, network, volumes.
//!
//! One long-lived `System`/`Networks`/`Disks` is reused for every sample;
//! only the needed refresh kinds are requested.

use crate::domain::*;
use crate::util::rate::RateTracker;
use crate::util::text::sanitize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use sysinfo::{
    DiskRefreshKind, Disks, MemoryRefreshKind, Networks, Pid, ProcessRefreshKind,
    ProcessesToUpdate, System, ThreadKind, UpdateKind,
};

/// Volumes can block on dead network mounts, so they refresh far less often.
const DISK_REFRESH_EVERY: Duration = Duration::from_secs(10);
/// Battery/temperature files are cheap but change slowly.
const SENSORS_EVERY: Duration = Duration::from_secs(10);
/// Headline-only network figures (Overview) are refreshed at most this often.
const SLOW_NET_EVERY: Duration = Duration::from_secs(2);

pub struct SystemCollector {
    sys: System,
    networks: Option<Networks>,
    disks: Option<Disks>,
    net_rates: HashMap<String, (RateTracker, RateTracker)>,
    statics: HashMap<ProcessId, Arc<ProcessStatic>>,
    long_names: HashMap<ProcessId, String>,
    seen: HashSet<ProcessId>,
    volumes: Vec<Volume>,
    volume_note: Option<String>,
    last_disk_refresh: Option<Instant>,
    sensors: crate::sensors::Sensors,
    last_sensors: Option<Instant>,
    last_sample: Option<Instant>,
    seq: u64,
    watch: Option<ProcessId>,
    needs: Needs,
    last_processes: Vec<ProcessSample>,
    last_network: Vec<NetIface>,
    last_proc_refresh: Option<Instant>,
    last_net_refresh: Option<Instant>,
    io_warm: bool,
    hostname: Option<String>,
    os: Option<String>,
    init_errors: Vec<CollectorStatus>,
}

fn proc_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet)
}

pub fn process_id(p: &sysinfo::Process) -> ProcessId {
    let st = p.start_time();
    ProcessId {
        pid: p.pid().as_u32(),
        started_at: (st != 0).then_some(st),
    }
}

impl SystemCollector {
    pub fn new() -> Result<Self, String> {
        let mut sys =
            System::new().map_err(|e| format!("cannot initialise system collector: {e}"))?;
        sys.refresh_cpu_usage(); // primes the CPU baseline and the core list
        let mut init_errors = Vec::new();
        let networks = match Networks::new_with_refreshed_list() {
            Ok(n) => Some(n),
            Err(e) => {
                init_errors.push(CollectorStatus {
                    name: "network",
                    state: CollectorState::Error,
                    detail: Some(e.to_string()),
                });
                None
            }
        };
        let disks = match Disks::new_with_refreshed_list_specifics(
            DiskRefreshKind::nothing().with_storage(),
        ) {
            Ok(d) => Some(d),
            Err(e) => {
                init_errors.push(CollectorStatus {
                    name: "storage",
                    state: CollectorState::Error,
                    detail: Some(e.to_string()),
                });
                None
            }
        };
        Ok(Self {
            sys,
            networks,
            disks,
            net_rates: HashMap::new(),
            statics: HashMap::new(),
            long_names: HashMap::new(),
            seen: HashSet::new(),
            volumes: Vec::new(),
            volume_note: None,
            last_disk_refresh: None,
            sensors: Default::default(),
            last_sensors: None,
            last_sample: None,
            seq: 0,
            watch: None,
            needs: Needs::all(),
            last_processes: Vec::new(),
            last_network: Vec::new(),
            last_proc_refresh: None,
            last_net_refresh: None,
            io_warm: false,
            hostname: System::host_name().ok().map(|s| sanitize(&s)),
            os: System::long_os_version()
                .or_else(|_| System::name())
                .ok()
                .map(|s| sanitize(&s)),
            init_errors,
        })
    }

    /// Request that the command line of this process be collected (lazy).
    pub fn watch(&mut self, id: Option<ProcessId>) {
        self.watch = id;
    }

    /// The process name to show. On Linux the kernel truncates `comm` to 15 characters;
    /// when that may have happened, recover the real name from the executable or argv
    /// (cached per process identity).
    fn display_name(
        cache: &mut HashMap<ProcessId, String>,
        id: ProcessId,
        p: &sysinfo::Process,
    ) -> String {
        let comm = p.name().to_string_lossy().to_string();
        if !cfg!(target_os = "linux") || comm.chars().count() < COMM_MAX {
            return sanitize(&comm);
        }
        if let Some(n) = cache.get(&id) {
            return n.clone();
        }
        let full = sanitize(&resolve_long_name(id.pid, &comm, p.exe()));
        cache.insert(id, full.clone());
        full
    }

    fn refresh_processes(&mut self, first: bool, now: Instant) -> Vec<ProcessSample> {
        let mut kind = proc_kind();
        if self.needs.process_io {
            kind = kind.with_disk_usage();
        }
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        if let Some(w) = self.watch {
            let pid = Pid::from_u32(w.pid);
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                proc_kind().with_cmd(UpdateKind::OnlyIfNotSet),
            );
        }
        // I/O deltas cover the time since the previous PROCESS refresh.
        let io_secs = self
            .last_proc_refresh
            .map(|t| now.saturating_duration_since(t).as_secs_f64())
            .filter(|s| *s > 0.0);
        self.last_proc_refresh = Some(now);
        let io_valid = self.needs.process_io && self.io_warm && io_secs.is_some();

        let mut processes = Vec::with_capacity(self.sys.processes().len());
        let mut live: HashSet<ProcessId> = HashSet::with_capacity(self.sys.processes().len());
        for p in self.sys.processes().values() {
            // Linux lists every thread of a process as its own entry; those are not processes
            // (they would double-count memory and inflate the process count).
            if p.thread_kind() == Some(ThreadKind::Userland) {
                continue;
            }
            let id = process_id(p);
            live.insert(id);
            let mut info = self.statics.get(&id).cloned().unwrap_or_else(|| {
                Arc::new(ProcessStatic {
                    exe: p.exe().map(|e| sanitize(&e.to_string_lossy())),
                    cmd: CmdLine::NotRequested,
                })
            });
            if self.watch == Some(id) && info.cmd == CmdLine::NotRequested {
                let parts: Vec<String> = p
                    .cmd()
                    .iter()
                    .map(|a| sanitize(&a.to_string_lossy()))
                    .collect();
                info = Arc::new(ProcessStatic {
                    exe: info.exe.clone(),
                    cmd: if parts.is_empty() {
                        CmdLine::Unavailable
                    } else {
                        CmdLine::Available(parts.join(" "))
                    },
                });
            }
            self.statics.insert(id, info.clone());
            // A process first seen in this sample has no CPU baseline yet.
            let warm = !first && self.seen.contains(&id);
            // Windows reports a 0 working set when OpenProcess is denied; no real
            // user-mode process has 0, so treat it as "unknown", not "0 B".
            let restricted = cfg!(windows) && p.memory() == 0;
            let io = (io_valid && warm && !restricted).then(|| {
                let d = p.disk_usage();
                let s = io_secs.unwrap_or(1.0);
                (d.read_bytes as f64 / s, d.written_bytes as f64 / s)
            });
            processes.push(ProcessSample {
                id,
                parent: p.parent().map(|p| p.as_u32()),
                name: Self::display_name(&mut self.long_names, id, p),
                cpu: (warm && !restricted).then(|| p.cpu_usage()),
                rss: (!restricted).then(|| p.memory()),
                restricted,
                status: status_of(p),
                io,
                info,
            });
        }
        if self.needs.process_io {
            self.io_warm = true;
        }
        // prune caches for processes that no longer exist
        self.statics.retain(|k, _| live.contains(k));
        self.long_names.retain(|k, _| live.contains(k));
        self.seen = live;
        self.last_processes = processes.clone();
        processes
    }

    /// Apply what the visible view needs. Returns true when a subsystem was turned
    /// ON, i.e. its next readings have no valid baseline and a quick re-sample is wanted.
    pub fn set_needs(&mut self, n: Needs) -> bool {
        let old = self.needs;
        let mut turned_on = false;
        if old.processes && !n.processes {
            self.seen.clear(); // CPU baselines are meaningless after a gap
            self.last_proc_refresh = None;
        }
        if !old.processes && n.processes {
            turned_on = true;
        }
        if !n.processes || (n.process_io && !old.process_io) {
            self.io_warm = false;
        }
        if old.process_io != n.process_io && n.processes {
            turned_on |= n.process_io;
        }
        if old.network != NetNeed::Off && n.network == NetNeed::Off {
            self.net_rates.clear();
            self.last_net_refresh = None;
        }
        if old.network == NetNeed::Off && n.network != NetNeed::Off {
            turned_on = true;
        }
        self.needs = n;
        turned_on
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = self.last_sample.map(|t| now.saturating_duration_since(t));
        let first = self.seq == 0;
        let mut diagnostics = std::mem::take(&mut self.init_errors);

        // ---- CPU + memory
        self.sys.refresh_cpu_usage();
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
        let cpus = self.sys.cpus();
        let cpu = CpuMetrics {
            usage: (!first && !cpus.is_empty()).then(|| self.sys.global_cpu_usage()),
            per_core: if first {
                Vec::new()
            } else {
                cpus.iter().map(|c| c.usage()).collect()
            },
            logical: cpus.len(),
            brand: cpus
                .first()
                .map(|c| sanitize(c.brand()))
                .unwrap_or_default(),
        };
        let total = self.sys.total_memory();
        let memory = (total > 0).then(|| MemoryMetrics {
            total,
            used: self.sys.used_memory(),
            available: self.sys.available_memory(),
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
        });
        diagnostics.push(CollectorStatus {
            name: "cpu",
            state: if cpus.is_empty() {
                CollectorState::Unsupported
            } else if first {
                CollectorState::WarmingUp
            } else {
                CollectorState::Ok
            },
            detail: cpus.is_empty().then(|| "no CPU information from OS".into()),
        });
        diagnostics.push(CollectorStatus {
            name: "memory",
            state: if memory.is_some() {
                CollectorState::Ok
            } else {
                CollectorState::Error
            },
            detail: memory
                .is_none()
                .then(|| "OS reported 0 bytes of RAM".into()),
        });

        // ---- processes (only when the visible view needs them)
        let processes_fresh = self.needs.processes;
        let processes = if processes_fresh {
            self.refresh_processes(first, now)
        } else {
            self.last_processes.clone()
        };
        diagnostics.push(CollectorStatus {
            name: "processes",
            state: if processes.is_empty() {
                CollectorState::Error
            } else {
                CollectorState::Ok
            },
            detail: processes
                .is_empty()
                .then(|| "OS returned no processes".into()),
        });

        // ---- network
        let net_due = match self.needs.network {
            NetNeed::Off => false,
            NetNeed::Fast => true,
            NetNeed::Slow => self
                .last_net_refresh
                .is_none_or(|t| now.saturating_duration_since(t) >= SLOW_NET_EVERY),
        };
        let network_fresh = net_due;
        let mut network = Vec::new();
        if !net_due {
            network = self.last_network.clone();
        } else if let Some(nets) = self.networks.as_mut() {
            nets.refresh(true);
            self.last_net_refresh = Some(now);
            let mut names = HashSet::new();
            for (name, data) in nets.iter() {
                let name = sanitize(&name.to_string_lossy());
                let (rx, tx) = (data.total_received(), data.total_transmitted());
                let (rt, tt) = self.net_rates.entry(name.clone()).or_default();
                let rx_rate = rt.update(rx, now);
                let tx_rate = tt.update(tx, now);
                let lower = name.to_ascii_lowercase();
                network.push(NetIface {
                    loopback: lower == "lo" || lower.contains("loopback"),
                    state: data.operational_state().to_string().to_ascii_lowercase(),
                    name: name.clone(),
                    rx_total: rx,
                    tx_total: tx,
                    rx_rate,
                    tx_rate,
                });
                names.insert(name);
            }
            self.net_rates.retain(|k, _| names.contains(k));
            network.sort_by(|a, b| a.name.cmp(&b.name));
            self.last_network = network.clone();
            diagnostics.push(CollectorStatus {
                name: "network",
                state: if network.is_empty() {
                    CollectorState::Unsupported
                } else if first {
                    CollectorState::WarmingUp
                } else {
                    CollectorState::Ok
                },
                detail: network
                    .is_empty()
                    .then(|| "no network interfaces reported".into()),
            });
        }

        // ---- volumes (slow cadence)
        if let Some(disks) = self.disks.as_mut() {
            let due = self
                .last_disk_refresh
                .is_none_or(|t| now.saturating_duration_since(t) >= DISK_REFRESH_EVERY);
            if due {
                disks.refresh_specifics(true, DiskRefreshKind::nothing().with_storage());
                self.last_disk_refresh = Some(now);
                let mut seen = HashSet::new();
                let mut hidden = 0usize;
                let mut vols = Vec::new();
                for d in disks.iter() {
                    let fs = sanitize(&d.file_system().to_string_lossy());
                    let mount = sanitize(&d.mount_point().to_string_lossy());
                    // Single-file bind mounts (e.g. /mnt/wslg/versions.txt) are not volumes.
                    // (Unix only: a stat on a dead Windows network drive could block.)
                    let file_mount = cfg!(unix) && !d.mount_point().is_dir();
                    if d.total_space() == 0 || fs == "squashfs" || file_mount {
                        hidden += 1;
                        continue;
                    }
                    if !seen.insert((mount.clone(), fs.clone())) {
                        continue;
                    }
                    vols.push(Volume {
                        name: sanitize(&d.name().to_string_lossy()),
                        mount,
                        fs,
                        total: d.total_space(),
                        available: d.available_space(),
                        removable: d.is_removable(),
                        read_only: d.is_read_only(),
                    });
                }
                self.volumes = vols;
                self.volume_note = (hidden > 0)
                    .then(|| format!("{hidden} volume(s) hidden (no capacity/squashfs)"));
            }
            diagnostics.push(CollectorStatus {
                name: "storage",
                state: if self.volumes.is_empty() {
                    CollectorState::Unsupported
                } else {
                    CollectorState::Ok
                },
                detail: if self.volumes.is_empty() {
                    Some("no mounted volumes reported".into())
                } else {
                    self.volume_note.clone()
                },
            });
        }

        if self
            .last_sensors
            .is_none_or(|t| now.saturating_duration_since(t) >= SENSORS_EVERY)
        {
            self.sensors = crate::sensors::read();
            self.last_sensors = Some(now);
        }
        self.last_sample = Some(now);
        let seq = self.seq;
        self.seq += 1;
        Snapshot {
            seq,
            sampled_at: SystemTime::now(),
            elapsed,
            cpu,
            memory,
            host: HostInfo {
                hostname: self.hostname.clone(),
                os: self.os.clone(),
                arch: std::env::consts::ARCH,
                uptime_secs: System::uptime().ok(),
            },
            processes,
            processes_fresh,
            network,
            network_fresh,
            volumes: self.volumes.clone(),
            sensors: self.sensors.clone(),
            diagnostics,
        }
    }
}

/// sysinfo hard-codes `Run` on Windows, which would be an invented reading.
fn status_of(p: &sysinfo::Process) -> Option<String> {
    if cfg!(windows) {
        None
    } else {
        Some(p.status().to_string().to_ascii_lowercase())
    }
}

/// Linux keeps only the first 15 characters of a process name (`comm`).
pub const COMM_MAX: usize = 15;

/// Pick the candidate that `comm` is a truncation of (it must be longer than 15 characters and
/// start with exactly `comm`); otherwise keep `comm`. Pure, so it is testable on any OS.
pub fn pick_long_name(comm: &str, candidates: &[String]) -> String {
    if comm.chars().count() >= COMM_MAX {
        for c in candidates {
            if c.chars().count() > COMM_MAX && c.chars().take(COMM_MAX).collect::<String>() == comm
            {
                return c.clone();
            }
        }
    }
    comm.to_string()
}

/// Names a process might really be called, from its executable and first two argv entries.
#[cfg(target_os = "linux")]
fn name_candidates(pid: u32, exe: Option<&std::path::Path>) -> Vec<String> {
    use std::io::Read;
    let mut v = Vec::new();
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    if let Some(n) = exe.and_then(|e| e.file_name()) {
        v.push(n.to_string_lossy().to_string());
    }
    if let Ok(f) = std::fs::File::open(format!("/proc/{pid}/cmdline")) {
        let mut buf = Vec::new();
        let _ = f.take(4096).read_to_end(&mut buf);
        for arg in buf.split(|b| *b == 0).take(2) {
            if let Ok(s) = std::str::from_utf8(arg) {
                if !s.is_empty() {
                    v.push(base(s));
                }
            }
        }
    }
    v
}

/// Best-effort full name for a process whose `comm` may be truncated (Linux only).
pub fn resolve_long_name(pid: u32, comm: &str, exe: Option<&std::path::Path>) -> String {
    #[cfg(target_os = "linux")]
    {
        pick_long_name(comm, &name_candidates(pid, exe))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, exe);
        comm.to_string()
    }
}

/// A snapshot describing a collector that could not start (UI shows the error).
pub fn failed_snapshot(message: String) -> Snapshot {
    Snapshot {
        seq: 0,
        sampled_at: SystemTime::now(),
        elapsed: None,
        cpu: CpuMetrics::default(),
        memory: None,
        host: HostInfo {
            hostname: None,
            os: None,
            arch: std::env::consts::ARCH,
            uptime_secs: None,
        },
        processes: Vec::new(),
        processes_fresh: true,
        network: Vec::new(),
        network_fresh: true,
        volumes: Vec::new(),
        sensors: Default::default(),
        diagnostics: vec![CollectorStatus {
            name: "system",
            state: CollectorState::Error,
            detail: Some(message),
        }],
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn long_names_are_recovered_only_when_they_are_a_truncation_of_the_comm() {
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick_long_name("elastic-otel-co", &c(&["elastic-otel-collector"])),
            "elastic-otel-collector"
        );
        // a candidate that merely shares a prefix of different length is not accepted
        assert_eq!(
            pick_long_name("elastic-otel-co", &c(&["elastic-agent-xyz-long"])),
            "elastic-otel-co"
        );
        // short names are never "improved"
        assert_eq!(
            pick_long_name("bash", &c(&["bash-completion-helper-long"])),
            "bash"
        );
        // exactly 15 chars with a 15-char candidate: nothing to recover
        assert_eq!(
            pick_long_name("abcdefghijklmno", &c(&["abcdefghijklmno"])),
            "abcdefghijklmno"
        );
        assert_eq!(pick_long_name("abcdefghijklmno", &[]), "abcdefghijklmno");
        // first matching candidate wins (exe before argv)
        assert_eq!(
            pick_long_name(
                "abcdefghijklmno",
                &c(&["abcdefghijklmnopqrs", "abcdefghijklmnoZZZ"])
            ),
            "abcdefghijklmnopqrs"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_linux_process_with_a_long_name_is_recovered() {
        // Copy `sleep` to a >15 char name, run it, and make sure we see the whole name.
        let dir = std::env::temp_dir().join(format!("systui-long-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("a-rather-long-process-name");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        let mut child = std::process::Command::new(&exe).arg("30").spawn().unwrap();
        let mut c = SystemCollector::new().unwrap();
        c.sample();
        std::thread::sleep(std::time::Duration::from_millis(600));
        let s = c.sample();
        let name = s
            .processes
            .iter()
            .find(|p| p.id.pid == child.id())
            .map(|p| p.name.clone());
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(name.as_deref(), Some("a-rather-long-process-name"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn threads_are_not_listed_as_processes() {
        // Spawn a few threads in this very process; none of them may appear as a process.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let s = stop.clone();
                std::thread::spawn(move || {
                    while !s.load(std::sync::atomic::Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                })
            })
            .collect();
        let mut c = SystemCollector::new().unwrap();
        let snap = c.sample();
        let me = std::process::id();
        let mine = snap.processes.iter().filter(|p| p.id.pid == me).count();
        let tids: Vec<u32> = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().parse().ok())
            .filter(|t| *t != me)
            .collect();
        let leaked = snap
            .processes
            .iter()
            .filter(|p| tids.contains(&p.id.pid))
            .count();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in hs {
            let _ = h.join();
        }
        assert_eq!(mine, 1);
        assert_eq!(
            leaked, 0,
            "{leaked} of this process's threads were listed as processes"
        );
    }
}

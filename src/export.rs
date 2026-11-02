//! Portable, sanitized exports (`snapshot --json`, process export).
//!
//! Omitted by default: hostname, usernames (home directories in paths are
//! redacted), command lines, environment, network addresses.

use crate::domain::{CollectorState, ProcessSample, Snapshot};
use serde::Serialize;
use std::time::UNIX_EPOCH;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct SnapshotDoc {
    pub schema_version: u32,
    pub tool: ToolDoc,
    pub collected_at_unix: u64,
    pub collected_at_utc: String,
    pub host: HostDoc,
    pub cpu: CpuDoc,
    pub memory: Option<MemDoc>,
    pub network: Vec<NetDoc>,
    pub volumes: Vec<VolDoc>,
    pub sensors: SensorsDoc,
    /// False when this sample did not refresh the process list (the visible view did not need it);
    /// `process_count` and `top_processes` are then withheld rather than shown stale.
    pub processes_fresh: bool,
    /// Same for network rates (they are null when false).
    pub network_fresh: bool,
    pub process_count: Option<usize>,
    pub top_processes: Vec<ProcDoc>,
    pub collectors: Vec<CollectorDoc>,
    pub omitted: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct ToolDoc {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct HostDoc {
    pub os: Option<String>,
    pub arch: &'static str,
    pub uptime_seconds: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CpuDoc {
    pub state: &'static str,
    pub logical_cores: usize,
    pub usage_percent: Option<f32>,
    pub per_core_percent: Vec<f32>,
}

#[derive(Debug, Serialize)]
pub struct MemDoc {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct NetDoc {
    pub name: String,
    pub state: String,
    pub loopback: bool,
    pub rx_total_bytes: u64,
    pub tx_total_bytes: u64,
    pub rx_bytes_per_sec: Option<f64>,
    pub tx_bytes_per_sec: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct VolDoc {
    pub mount: String,
    pub fs: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct SensorsDoc {
    /// null = no battery present or readable.
    pub battery: Option<BatteryDoc>,
    pub temperatures: Vec<TempDoc>,
    /// Why temperatures are empty when that is a platform limitation.
    pub temperatures_note: Option<String>,
    pub thermal_throttle_events: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BatteryDoc {
    pub percent: Option<u8>,
    pub state: &'static str,
    pub ac_online: Option<bool>,
    pub minutes_remaining: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct TempDoc {
    pub label: String,
    pub celsius: f32,
}

#[derive(Debug, Serialize)]
pub struct ProcDoc {
    pub pid: u32,
    pub name: String,
    pub parent_pid: Option<u32>,
    pub status: Option<String>,
    /// Percent of one logical CPU (may exceed 100). null = unknown.
    pub cpu_percent_of_one_core: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub access_restricted: bool,
    pub started_at_unix: Option<u64>,
    pub executable: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CollectorDoc {
    pub name: &'static str,
    pub state: &'static str,
    pub detail: Option<String>,
}

/// Path redaction: home directory and user-profile segments become `~`/`<user>`.
pub struct Redactor {
    home: Option<String>,
}

impl Redactor {
    pub fn from_env() -> Self {
        Self {
            home: directories::BaseDirs::new().map(|d| d.home_dir().to_string_lossy().to_string()),
        }
    }

    pub fn with_home(home: Option<&str>) -> Self {
        Self {
            home: home.map(str::to_string),
        }
    }

    pub fn path(&self, p: &str) -> String {
        let mut s = p.to_string();
        if let Some(h) = &self.home {
            if let (false, Some(head), Some(rest)) =
                (h.is_empty(), s.get(..h.len()), s.get(h.len()..))
            {
                if head.eq_ignore_ascii_case(h) {
                    s = format!("~{rest}");
                }
            }
        }
        redact_profile_segment(&s)
    }
}

/// `C:\Users\alice\x` -> `C:\Users\<user>\x`, `/home/alice/x` -> `/home/<user>/x`
/// (covers other users' profiles that the home-dir rule does not).
fn redact_profile_segment(s: &str) -> String {
    for marker in ["\\Users\\", "/Users/", "/home/"] {
        let lower = s.to_ascii_lowercase();
        if let Some(i) = lower.find(&marker.to_ascii_lowercase()) {
            let start = i + marker.len();
            let sep = if marker.contains('\\') { '\\' } else { '/' };
            let end = s[start..].find(sep).map_or(s.len(), |e| start + e);
            if &s[start..end] != "<user>" && end > start {
                return format!("{}<user>{}", &s[..start], &s[end..]);
            }
        }
    }
    s.to_string()
}

fn state_str(s: &CollectorState) -> &'static str {
    match s {
        CollectorState::Ok => "ok",
        CollectorState::WarmingUp => "warming_up",
        CollectorState::Unsupported => "unsupported",
        CollectorState::Error => "error",
    }
}

pub fn proc_doc(p: &ProcessSample, r: &Redactor) -> ProcDoc {
    ProcDoc {
        pid: p.id.pid,
        name: p.name.clone(),
        parent_pid: p.parent,
        status: p.status.clone(),
        cpu_percent_of_one_core: p.cpu,
        memory_bytes: p.rss,
        access_restricted: p.restricted,
        started_at_unix: p.id.started_at,
        executable: p.info.exe.as_deref().map(|e| r.path(e)),
    }
}

/// Build the document. Top processes: highest CPU first (unknown last), ties by memory.
pub fn build(s: &Snapshot, top: usize, r: &Redactor) -> SnapshotDoc {
    let mut idx: Vec<usize> = (0..s.processes.len()).collect();
    idx.sort_by(|&a, &b| {
        let (pa, pb) = (&s.processes[a], &s.processes[b]);
        // Sort-only keys: unknown ranks below any real reading.
        pb.cpu
            .unwrap_or(-1.0)
            .partial_cmp(&pa.cpu.unwrap_or(-1.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(pb.rss.cmp(&pa.rss))
            .then(pa.id.pid.cmp(&pb.id.pid))
    });
    let unix = s
        .sampled_at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    SnapshotDoc {
        schema_version: SCHEMA_VERSION,
        tool: ToolDoc {
            name: "systui",
            version: env!("CARGO_PKG_VERSION"),
        },
        collected_at_unix: unix,
        collected_at_utc: utc_string(unix),
        host: HostDoc {
            os: s.host.os.clone(),
            arch: s.host.arch,
            uptime_seconds: s.host.uptime_secs,
        },
        cpu: CpuDoc {
            state: if s.cpu.usage.is_some() {
                "ok"
            } else {
                "warming_up"
            },
            logical_cores: s.cpu.logical,
            usage_percent: s.cpu.usage,
            per_core_percent: s.cpu.per_core.clone(),
        },
        memory: s.memory.map(|m| MemDoc {
            total_bytes: m.total,
            used_bytes: m.used,
            available_bytes: m.available,
            swap_total_bytes: m.swap_total,
            swap_used_bytes: m.swap_used,
        }),
        network: s
            .network
            .iter()
            .map(|n| NetDoc {
                name: n.name.clone(),
                state: n.state.clone(),
                loopback: n.loopback,
                rx_total_bytes: n.rx_total,
                tx_total_bytes: n.tx_total,
                rx_bytes_per_sec: n.rx_rate.filter(|_| s.network_fresh),
                tx_bytes_per_sec: n.tx_rate.filter(|_| s.network_fresh),
            })
            .collect(),
        volumes: s
            .volumes
            .iter()
            .map(|v| VolDoc {
                mount: v.mount.clone(),
                fs: v.fs.clone(),
                total_bytes: v.total,
                available_bytes: v.available,
            })
            .collect(),
        sensors: SensorsDoc {
            battery: s.sensors.battery.as_ref().map(|b| BatteryDoc {
                percent: b.percent,
                state: b.state.label(),
                ac_online: b.ac_online,
                minutes_remaining: b.minutes_remaining,
            }),
            temperatures: s
                .sensors
                .temps
                .iter()
                .map(|t| TempDoc {
                    label: t.label.clone(),
                    celsius: t.celsius,
                })
                .collect(),
            temperatures_note: s.sensors.temps_note.clone(),
            thermal_throttle_events: s.sensors.throttle_events,
        },
        processes_fresh: s.processes_fresh,
        network_fresh: s.network_fresh,
        process_count: s.processes_fresh.then_some(s.processes.len()),
        top_processes: if s.processes_fresh {
            idx.into_iter()
                .take(top)
                .map(|i| proc_doc(&s.processes[i], r))
                .collect()
        } else {
            Vec::new()
        },
        collectors: s
            .diagnostics
            .iter()
            .map(|d| CollectorDoc {
                name: d.name,
                state: state_str(&d.state),
                detail: d.detail.clone(),
            })
            .collect(),
        omitted: vec![
            "hostname",
            "usernames",
            "command_lines",
            "environment",
            "network_addresses",
        ],
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` from Unix seconds (civil-from-days, no deps).
pub fn utc_string(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let rem = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_known_values() {
        assert_eq!(utc_string(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_string(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_string(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn redacts_home_and_profiles() {
        let r = Redactor::with_home(Some("C:\\Users\\alice"));
        assert_eq!(r.path("C:\\Users\\alice\\bin\\x.exe"), "~\\bin\\x.exe");
        assert_eq!(r.path("c:\\users\\ALICE\\bin\\x.exe"), "~\\bin\\x.exe");
        assert_eq!(
            r.path("C:\\Users\\bob\\AppData\\y.exe"),
            "C:\\Users\\<user>\\AppData\\y.exe"
        );
        assert_eq!(
            r.path("C:\\Windows\\System32\\cmd.exe"),
            "C:\\Windows\\System32\\cmd.exe"
        );
        let l = Redactor::with_home(Some("/home/alice"));
        assert_eq!(l.path("/home/alice/.cargo/bin/x"), "~/.cargo/bin/x");
        assert_eq!(l.path("/home/bob/x"), "/home/<user>/x");
        assert_eq!(l.path("/usr/bin/x"), "/usr/bin/x");
        assert_eq!(
            Redactor::with_home(None).path("/Users/carol/a"),
            "/Users/<user>/a"
        );
    }
}

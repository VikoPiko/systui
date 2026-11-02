//! Battery / temperature / thermal-throttling readings, only where the platform
//! exposes them without privileges. Absent hardware is simply absent (`None` /
//! empty): nothing is invented and no "0 °C" or "0 %" is ever shown.
//!
//! * Linux: `/sys/class/power_supply`, `/sys/class/hwmon`, `/sys/class/thermal`.
//! * Windows: `GetSystemPowerStatus` for the battery. Windows has no unprivileged
//!   CPU temperature API, so temperatures are reported as unavailable with the reason.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    /// Plugged in but not charging.
    NotCharging,
    Unknown,
}

impl BatteryState {
    pub fn label(self) -> &'static str {
        match self {
            BatteryState::Charging => "charging",
            BatteryState::Discharging => "discharging",
            BatteryState::Full => "full",
            BatteryState::NotCharging => "plugged in, not charging",
            BatteryState::Unknown => "state unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// 0-100, or `None` if the OS does not know.
    pub percent: Option<u8>,
    pub state: BatteryState,
    pub ac_online: Option<bool>,
    /// OS estimate of time to empty while discharging.
    pub minutes_remaining: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TempReading {
    pub label: String,
    pub celsius: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sensors {
    /// `None` = no battery present (desktop) or not readable.
    pub battery: Option<Battery>,
    pub temps: Vec<TempReading>,
    /// Why `temps` is empty, when that is a platform limitation rather than "no sensors".
    pub temps_note: Option<String>,
    /// Total thermal-throttle events reported by the CPU (Linux, where available).
    pub throttle_events: Option<u64>,
}

impl Sensors {
    pub fn hottest(&self) -> Option<&TempReading> {
        self.temps.iter().max_by(|a, b| {
            a.celsius
                .partial_cmp(&b.celsius)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn is_empty(&self) -> bool {
        self.battery.is_none() && self.temps.is_empty() && self.throttle_events.is_none()
    }
}

/// Plausible air/silicon temperature; anything else is a broken sensor, not a reading.
fn plausible(c: f32) -> bool {
    c.is_finite() && (-30.0..=150.0).contains(&c)
}

// ----------------------------------------------------------------- Windows

/// Interpret the fields of `SYSTEM_POWER_STATUS` (pure, so it is testable everywhere).
pub fn battery_from_power_status(ac: u8, flag: u8, percent: u8, life_secs: u32) -> Option<Battery> {
    if flag != 255 && flag & 128 != 0 {
        return None; // "no system battery"
    }
    let ac_online = match ac {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    let pct = (percent <= 100).then_some(percent);
    let charging = flag != 255 && flag & 8 != 0;
    let state = if flag == 255 {
        BatteryState::Unknown
    } else if charging {
        BatteryState::Charging
    } else {
        match (ac_online, pct) {
            (Some(true), Some(100)) => BatteryState::Full,
            (Some(true), _) => BatteryState::NotCharging,
            (Some(false), _) => BatteryState::Discharging,
            (None, _) => BatteryState::Unknown,
        }
    };
    let minutes_remaining =
        (state == BatteryState::Discharging && life_secs != u32::MAX).then_some(life_secs / 60);
    Some(Battery {
        percent: pct,
        state,
        ac_online,
        minutes_remaining,
    })
}

#[cfg(windows)]
fn read_windows() -> Sensors {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    // SAFETY: `st` is a valid, writable SYSTEM_POWER_STATUS for the duration of the call.
    let ok = unsafe { GetSystemPowerStatus(&mut st) }.is_ok();
    Sensors {
        battery: ok
            .then(|| {
                battery_from_power_status(
                    st.ACLineStatus,
                    st.BatteryFlag,
                    st.BatteryLifePercent,
                    st.BatteryLifeTime,
                )
            })
            .flatten(),
        temps: Vec::new(),
        temps_note: Some(
            "Windows has no unprivileged CPU temperature API (GPU temperature is in the Dev tab)"
                .into(),
        ),
        throttle_events: None,
    }
}

// ------------------------------------------------------------------- Linux

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn subdirs(p: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
        })
        .collect();
    v.sort();
    v
}

/// Read sensors from a sysfs tree rooted at `sys` (normally `/sys`).
pub fn read_linux(sys: &Path) -> Sensors {
    // power supplies
    let (mut battery, mut ac_online) = (None, None::<bool>);
    for d in subdirs(&sys.join("class/power_supply"), "") {
        let ty = read_trim(&d.join("type")).unwrap_or_default();
        match ty.as_str() {
            "Battery" => {
                // peripherals (mouse, headset) report scope=Device; only the system battery counts
                if read_trim(&d.join("scope")).as_deref() == Some("Device") || battery.is_some() {
                    continue;
                }
                let percent = read_trim(&d.join("capacity"))
                    .and_then(|c| c.parse::<u8>().ok())
                    .filter(|p| *p <= 100);
                let state = match read_trim(&d.join("status")).as_deref() {
                    Some("Charging") => BatteryState::Charging,
                    Some("Discharging") => BatteryState::Discharging,
                    Some("Full") => BatteryState::Full,
                    Some("Not charging") => BatteryState::NotCharging,
                    _ => BatteryState::Unknown,
                };
                battery = Some(Battery {
                    percent,
                    state,
                    ac_online: None,
                    minutes_remaining: None,
                });
            }
            "Mains" => {
                if let Some(v) = read_trim(&d.join("online")) {
                    ac_online = Some(ac_online.unwrap_or(false) || v == "1");
                }
            }
            _ => {}
        }
    }
    if let Some(b) = battery.as_mut() {
        b.ac_online = ac_online;
    }

    // temperatures
    let mut temps = Vec::new();
    for h in subdirs(&sys.join("class/hwmon"), "hwmon") {
        let chip = read_trim(&h.join("name")).unwrap_or_else(|| "hwmon".into());
        let mut inputs: Vec<PathBuf> = std::fs::read_dir(&h)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy())
                    .is_some_and(|n| n.starts_with("temp") && n.ends_with("_input"))
            })
            .collect();
        inputs.sort();
        for p in inputs {
            let Some(fname) = p.file_name() else { continue };
            let stem = fname.to_string_lossy().replace("_input", "");
            let Some(milli) = read_trim(&p).and_then(|v| v.parse::<i64>().ok()) else {
                continue;
            };
            let c = milli as f32 / 1000.0;
            if !plausible(c) {
                continue;
            }
            let label = read_trim(&h.join(format!("{stem}_label")))
                .map(|l| format!("{chip} {l}"))
                .unwrap_or_else(|| format!("{chip} {stem}"));
            temps.push(TempReading {
                label: crate::util::text::sanitize(&label),
                celsius: c,
            });
        }
    }
    if temps.is_empty() {
        for z in subdirs(&sys.join("class/thermal"), "thermal_zone") {
            let Some(milli) = read_trim(&z.join("temp")).and_then(|v| v.parse::<i64>().ok()) else {
                continue;
            };
            let c = milli as f32 / 1000.0;
            if plausible(c) {
                let ty = read_trim(&z.join("type")).unwrap_or_else(|| "zone".into());
                temps.push(TempReading {
                    label: crate::util::text::sanitize(&ty),
                    celsius: c,
                });
            }
        }
    }
    temps.truncate(16);

    // thermal throttling (Intel exposes per-core counters)
    let mut total = None::<u64>;
    for c in subdirs(&sys.join("devices/system/cpu"), "cpu") {
        if let Some(n) = read_trim(&c.join("thermal_throttle/core_throttle_count"))
            .and_then(|v| v.parse::<u64>().ok())
        {
            total = Some(total.unwrap_or(0) + n);
        }
    }

    Sensors {
        battery,
        temps_note: temps
            .is_empty()
            .then(|| "no temperature sensors exposed under /sys".to_string()),
        temps,
        throttle_events: total,
    }
}

/// Read whatever this platform offers. Cheap enough to call every few seconds.
pub fn read() -> Sensors {
    #[cfg(windows)]
    {
        read_windows()
    }
    #[cfg(target_os = "linux")]
    {
        read_linux(Path::new("/sys"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Sensors {
            temps_note: Some("battery/temperature sensors are not implemented for this OS".into()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_status_no_battery_is_none_not_zero_percent() {
        assert_eq!(battery_from_power_status(1, 128, 255, u32::MAX), None);
    }

    #[test]
    fn windows_status_cases() {
        let b = battery_from_power_status(0, 1, 63, 7_800).unwrap();
        assert_eq!(
            (b.percent, b.state, b.ac_online, b.minutes_remaining),
            (Some(63), BatteryState::Discharging, Some(false), Some(130))
        );
        let b = battery_from_power_status(1, 9, 40, u32::MAX).unwrap(); // charging flag 8
        assert_eq!(
            (b.state, b.minutes_remaining),
            (BatteryState::Charging, None)
        );
        let b = battery_from_power_status(1, 1, 100, u32::MAX).unwrap();
        assert_eq!(b.state, BatteryState::Full);
        let b = battery_from_power_status(1, 1, 80, u32::MAX).unwrap();
        assert_eq!(b.state, BatteryState::NotCharging);
        let b = battery_from_power_status(255, 255, 255, u32::MAX).unwrap();
        assert_eq!(
            (b.percent, b.state, b.ac_online),
            (None, BatteryState::Unknown, None)
        );
        let b = battery_from_power_status(0, 1, 50, u32::MAX).unwrap();
        assert_eq!(
            b.minutes_remaining, None,
            "unknown estimate is not 0 minutes"
        );
    }

    fn fake_sys(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("systui-sys-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, v: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, v).unwrap();
        };
        w("class/power_supply/BAT0/type", "Battery\n");
        w("class/power_supply/BAT0/capacity", "57\n");
        w("class/power_supply/BAT0/status", "Discharging\n");
        w("class/power_supply/hidpp_battery_0/type", "Battery\n"); // a mouse
        w("class/power_supply/hidpp_battery_0/scope", "Device\n");
        w("class/power_supply/hidpp_battery_0/capacity", "11\n");
        w("class/power_supply/AC/type", "Mains\n");
        w("class/power_supply/AC/online", "0\n");
        w("class/hwmon/hwmon0/name", "coretemp\n");
        w("class/hwmon/hwmon0/temp1_input", "61000\n");
        w("class/hwmon/hwmon0/temp1_label", "Package id 0\n");
        w("class/hwmon/hwmon0/temp2_input", "58500\n");
        w("class/hwmon/hwmon1/name", "broken\n");
        w("class/hwmon/hwmon1/temp1_input", "-127000\n"); // implausible: ignored
        w("class/hwmon/hwmon2/name", "nvme\n");
        w("class/hwmon/hwmon2/temp1_input", "abc\n"); // unparsable: ignored
        w(
            "devices/system/cpu/cpu0/thermal_throttle/core_throttle_count",
            "3\n",
        );
        w(
            "devices/system/cpu/cpu1/thermal_throttle/core_throttle_count",
            "4\n",
        );
        root
    }

    #[test]
    fn linux_sysfs_is_parsed_and_junk_is_ignored() {
        let root = fake_sys("parse");
        let s = read_linux(&root);
        let b = s.battery.clone().unwrap();
        assert_eq!(
            (b.percent, b.state, b.ac_online),
            (Some(57), BatteryState::Discharging, Some(false))
        );
        assert_eq!(s.temps.len(), 2, "{:?}", s.temps);
        assert_eq!(
            s.temps[0],
            TempReading {
                label: "coretemp Package id 0".into(),
                celsius: 61.0
            }
        );
        assert_eq!(s.temps[1].label, "coretemp temp2");
        assert_eq!(s.hottest().unwrap().celsius, 61.0);
        assert_eq!(s.throttle_events, Some(7));
        assert!(s.temps_note.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn peripheral_batteries_are_not_the_system_battery() {
        let root = fake_sys("peripheral");
        std::fs::remove_dir_all(root.join("class/power_supply/BAT0")).unwrap();
        assert!(
            read_linux(&root).battery.is_none(),
            "a mouse battery must not appear as the laptop battery"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn thermal_zone_fallback_and_empty_tree() {
        let root =
            std::env::temp_dir().join(format!("systui-sys-{}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("class/thermal/thermal_zone0")).unwrap();
        std::fs::write(root.join("class/thermal/thermal_zone0/temp"), "45000").unwrap();
        std::fs::write(
            root.join("class/thermal/thermal_zone0/type"),
            "x86_pkg_temp",
        )
        .unwrap();
        let s = read_linux(&root);
        assert_eq!(
            s.temps,
            vec![TempReading {
                label: "x86_pkg_temp".into(),
                celsius: 45.0
            }]
        );
        let _ = std::fs::remove_dir_all(&root);

        let empty = std::env::temp_dir().join("systui-definitely-missing-sys-tree");
        let s = read_linux(&empty);
        assert!(s.is_empty() && s.battery.is_none() && s.throttle_events.is_none());
        assert!(s
            .temps_note
            .as_deref()
            .unwrap()
            .contains("no temperature sensors"));
    }

    #[test]
    fn real_platform_read_never_panics_and_is_sane() {
        let s = read();
        if let Some(b) = &s.battery {
            assert!(b.percent.is_none_or(|p| p <= 100));
        }
        for t in &s.temps {
            assert!(plausible(t.celsius));
        }
        #[cfg(windows)]
        assert!(
            s.temps.is_empty() && s.temps_note.is_some(),
            "Windows temperatures are reported unavailable, with a reason"
        );
    }
}

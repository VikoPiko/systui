mod common;
use common::*;
use std::sync::Arc;
use systui::commands::{validate_output_path, write_output};
use systui::domain::{CollectorState, CollectorStatus};
use systui::export::{build, Redactor, SCHEMA_VERSION};

fn doc_json(top: usize) -> serde_json::Value {
    let mut s = snapshot(40);
    s.cpu.usage = None; // warming up must serialize as null, not 0
    s.processes[3].restricted = true;
    s.processes[3].rss = None;
    s.processes[3].cpu = None;
    s.processes[0].info = Arc::new(systui::domain::ProcessStatic {
        exe: Some("C:\\Users\\alice\\secret-project\\tool.exe".into()),
        cmd: systui::domain::CmdLine::Available("tool.exe --token=SUPERSECRET".into()),
    });
    s.diagnostics.push(CollectorStatus {
        name: "storage",
        state: CollectorState::Unsupported,
        detail: Some("none".into()),
    });
    serde_json::to_value(build(
        &s,
        top,
        &Redactor::with_home(Some("C:\\Users\\alice")),
    ))
    .unwrap()
}

fn keys(v: &serde_json::Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

#[test]
fn top_level_schema_is_stable() {
    let v = doc_json(5);
    assert_eq!(
        keys(&v),
        [
            "collected_at_unix",
            "collected_at_utc",
            "collectors",
            "cpu",
            "host",
            "memory",
            "network",
            "network_fresh",
            "omitted",
            "process_count",
            "processes_fresh",
            "schema_version",
            "sensors",
            "tool",
            "top_processes",
            "volumes"
        ]
    );
    assert_eq!(v["schema_version"], SCHEMA_VERSION);
    assert_eq!(v["tool"]["name"], "systui");
    assert_eq!(
        keys(&v["cpu"]),
        [
            "logical_cores",
            "per_core_percent",
            "state",
            "usage_percent"
        ]
    );
    assert_eq!(
        keys(&v["top_processes"][0]),
        [
            "access_restricted",
            "cpu_percent_of_one_core",
            "executable",
            "memory_bytes",
            "name",
            "parent_pid",
            "pid",
            "started_at_unix",
            "status"
        ]
    );
    assert_eq!(
        keys(&v["network"][0]),
        [
            "loopback",
            "name",
            "rx_bytes_per_sec",
            "rx_total_bytes",
            "state",
            "tx_bytes_per_sec",
            "tx_total_bytes"
        ]
    );
    assert_eq!(
        keys(&v["volumes"][0]),
        ["available_bytes", "fs", "mount", "total_bytes"]
    );
    assert_eq!(
        keys(&v["memory"]),
        [
            "available_bytes",
            "swap_total_bytes",
            "swap_used_bytes",
            "total_bytes",
            "used_bytes"
        ]
    );
}

#[test]
fn unavailable_is_null_never_zero() {
    let v = doc_json(40);
    assert!(v["cpu"]["usage_percent"].is_null());
    assert_eq!(v["cpu"]["state"], "warming_up");
    let restricted: Vec<_> = v["top_processes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["access_restricted"] == true)
        .collect();
    assert_eq!(restricted.len(), 1);
    assert!(restricted[0]["memory_bytes"].is_null());
    assert!(restricted[0]["cpu_percent_of_one_core"].is_null());
    let c = v["collectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "storage")
        .unwrap();
    assert_eq!(c["state"], "unsupported");
}

#[test]
fn sensitive_data_is_absent_by_default() {
    let text = serde_json::to_string(&doc_json(40)).unwrap();
    for needle in ["SUPERSECRET", "--token", "alice", "testhost"] {
        assert!(!text.contains(needle), "leaked `{needle}`");
    }
    assert!(text.contains("~"), "home directory redacted to ~");
    let v = doc_json(40);
    assert!(v["omitted"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o == "command_lines"));
}

#[test]
fn top_limits_and_orders_by_cpu() {
    let v = doc_json(7);
    let list = v["top_processes"].as_array().unwrap();
    assert_eq!(list.len(), 7);
    let cpus: Vec<f64> = list
        .iter()
        .filter_map(|p| p["cpu_percent_of_one_core"].as_f64())
        .collect();
    assert!(cpus.windows(2).all(|w| w[0] >= w[1]));
    assert_eq!(v["process_count"], 40);
    assert_eq!(doc_json(0)["top_processes"].as_array().unwrap().len(), 0);
}

#[test]
fn output_never_silently_overwrites() {
    let dir = std::env::temp_dir().join(format!("systui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("out.json");
    write_output(&f, "one", false).unwrap();
    let e = write_output(&f, "two", false).unwrap_err();
    assert!(e.contains("--force"), "{e}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "one");
    write_output(&f, "three", true).unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "three");
    assert!(validate_output_path(&dir)
        .unwrap_err()
        .contains("directory"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remote_looking_output_paths_are_rejected() {
    for p in [
        "\\\\server\\share\\x.json",
        "//server/x.json",
        "https://example.com/x.json",
    ] {
        assert!(
            validate_output_path(std::path::Path::new(p)).is_err(),
            "{p}"
        );
    }
    assert!(validate_output_path(std::path::Path::new("local.json")).is_ok());
}

#[test]
fn sensors_section_is_additive_null_when_absent_and_populated_when_present() {
    use systui::sensors::*;
    let mut s = snapshot(3);
    let v = serde_json::to_value(build(&s, 3, &Redactor::with_home(None))).unwrap();
    assert!(
        v["sensors"]["battery"].is_null(),
        "no battery => null, not 0%"
    );
    assert_eq!(v["sensors"]["temperatures"].as_array().unwrap().len(), 0);
    assert!(v["sensors"]["thermal_throttle_events"].is_null());
    assert_eq!(
        keys(&v["sensors"]),
        [
            "battery",
            "temperatures",
            "temperatures_note",
            "thermal_throttle_events"
        ]
    );
    assert_eq!(
        v["schema_version"], 1,
        "additive change keeps the schema version"
    );

    s.sensors = Sensors {
        battery: Some(Battery {
            percent: Some(57),
            state: BatteryState::Discharging,
            ac_online: Some(false),
            minutes_remaining: None,
        }),
        temps: vec![TempReading {
            label: "coretemp Package id 0".into(),
            celsius: 61.0,
        }],
        temps_note: None,
        throttle_events: Some(7),
    };
    let v = serde_json::to_value(build(&s, 3, &Redactor::with_home(None))).unwrap();
    assert_eq!(
        keys(&v["sensors"]["battery"]),
        ["ac_online", "minutes_remaining", "percent", "state"]
    );
    assert_eq!(v["sensors"]["battery"]["percent"], 57);
    assert_eq!(v["sensors"]["battery"]["state"], "discharging");
    assert!(v["sensors"]["battery"]["minutes_remaining"].is_null());
    assert_eq!(v["sensors"]["temperatures"][0]["celsius"], 61.0);
    assert_eq!(v["sensors"]["thermal_throttle_events"], 7);
}

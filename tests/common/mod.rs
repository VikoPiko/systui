#![allow(dead_code)]
//! Synthetic data builders shared by integration tests.

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use systui::domain::*;

pub fn pid(n: u32) -> ProcessId {
    ProcessId {
        pid: n,
        started_at: Some(1_700_000_000 + n as u64),
    }
}

pub fn proc(n: u32, name: &str, cpu: Option<f32>, rss: u64) -> ProcessSample {
    ProcessSample {
        id: pid(n),
        parent: if n > 1 { Some(1) } else { None },
        name: name.to_string(),
        cpu,
        rss: Some(rss),
        restricted: false,
        io: None,
        status: Some("run".into()),
        info: Arc::new(ProcessStatic {
            exe: Some(format!("/bin/{name}")),
            cmd: CmdLine::Available(format!("/bin/{name} --flag")),
        }),
    }
}

/// `n` processes with distinct, deterministic CPU/RSS values.
pub fn many_procs(n: u32) -> Vec<ProcessSample> {
    (1..=n)
        .map(|i| {
            proc(
                i,
                &format!("proc{i:04}"),
                Some(((i * 37) % 400) as f32 / 4.0),
                (i as u64 * 7919 % 10_000) * 1024 * 1024 / 10,
            )
        })
        .collect()
}

pub fn snapshot_with(seq: u64, processes: Vec<ProcessSample>) -> Snapshot {
    Snapshot {
        seq,
        sampled_at: SystemTime::now(),
        elapsed: Some(Duration::from_secs(1)),
        cpu: CpuMetrics {
            usage: Some(37.0),
            per_core: vec![10.0, 20.0, 95.0, 50.0],
            logical: 4,
            brand: "Test CPU".into(),
        },
        memory: Some(MemoryMetrics {
            total: 16 << 30,
            used: 9 << 30,
            available: 7 << 30,
            swap_total: 4 << 30,
            swap_used: 1 << 30,
        }),
        host: HostInfo {
            hostname: Some("testhost".into()),
            os: Some("TestOS 1".into()),
            arch: "x86_64",
            uptime_secs: Some(90_000),
        },
        processes,
        processes_fresh: true,
        network_fresh: true,
        network: vec![
            NetIface {
                name: "lo".into(),
                state: "up".into(),
                loopback: true,
                rx_total: 10,
                tx_total: 10,
                rx_rate: Some(1.0),
                tx_rate: Some(1.0),
            },
            NetIface {
                name: "eth0".into(),
                state: "up".into(),
                loopback: false,
                rx_total: 5 << 30,
                tx_total: 1 << 30,
                rx_rate: Some(2048.0),
                tx_rate: Some(512.0),
            },
        ],
        volumes: vec![
            Volume {
                name: "sda1".into(),
                mount: "/".into(),
                fs: "ext4".into(),
                total: 100 << 30,
                available: 30 << 30,
                removable: false,
                read_only: false,
            },
            Volume {
                name: "sdb1".into(),
                mount: "/data".into(),
                fs: "xfs".into(),
                total: 500 << 30,
                available: 10 << 30,
                removable: false,
                read_only: false,
            },
        ],
        sensors: Default::default(),
        diagnostics: vec![],
    }
}

pub fn snapshot(n: u32) -> Snapshot {
    snapshot_with(1, many_procs(n))
}

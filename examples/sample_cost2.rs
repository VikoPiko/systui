//! Diagnostic: which sysinfo process-refresh strategies are cheaper?
use std::time::{Duration, Instant};
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

fn avg(name: &str, n: u32, mut f: impl FnMut()) {
    f();
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    println!(
        "{name:<52} {:>7.2} ms/call",
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );
}

fn main() {
    let mut sys = System::new().unwrap();
    let kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    std::thread::sleep(Duration::from_millis(500));
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    let total = sys.processes().len();
    // processes that actually have readable counters (memory != 0)
    let ok: Vec<Pid> = sys
        .processes()
        .values()
        .filter(|p| p.memory() > 0)
        .map(|p| p.pid())
        .collect();
    println!("processes: {total}, readable: {}", ok.len());
    avg("All  cpu+mem+exe", 30, || {
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    });
    avg("All  mem only", 30, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
    });
    avg("All  disk_usage only", 30, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_disk_usage(),
        );
    });
    avg("All  cpu+mem+disk_usage", 30, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_disk_usage(),
        );
    });
    avg("Some(readable only) cpu+mem", 30, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&ok),
            false,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
    });
    let top40: Vec<Pid> = ok.iter().take(40).copied().collect();
    avg("Some(40 pids) cpu+mem", 30, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&top40),
            false,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
    });
    let mut nets = Networks::new_with_refreshed_list().unwrap();
    avg("Networks::refresh(true)", 30, || nets.refresh(true));
    avg("Networks::refresh(false)", 30, || nets.refresh(false));
}

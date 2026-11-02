//! Time the individual refresh calls the collector makes (diagnostic).
use std::time::{Duration, Instant};
use sysinfo::{
    DiskRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    System, UpdateKind,
};

fn avg(name: &str, n: u32, mut f: impl FnMut()) {
    f();
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    println!(
        "{name:<44} {:>8.2} ms/call",
        t.elapsed().as_secs_f64() * 1e3 / n as f64
    );
}

fn main() {
    let mut sys = System::new().unwrap();
    sys.refresh_cpu_usage();
    std::thread::sleep(Duration::from_millis(300));
    avg("cpu usage", 20, || sys.refresh_cpu_usage());
    avg("memory", 20, || {
        sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap())
    });
    let all = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet);
    avg("procs cpu+mem+exe(OnlyIfNotSet)", 20, || {
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, all);
    });
    avg("procs cpu+mem", 20, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
    });
    avg("procs memory only", 20, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
    });
    avg("procs cpu only", 20, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
    });
    avg("procs nothing (list only)", 20, || {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
    });
    let mut nets = Networks::new_with_refreshed_list().unwrap();
    avg("networks refresh", 20, || nets.refresh(true));
    let mut disks = Disks::new_with_refreshed_list().unwrap();
    avg("disks refresh (storage only)", 5, || {
        disks.refresh_specifics(true, DiskRefreshKind::nothing().with_storage())
    });
    println!("processes: {}", sys.processes().len());
}

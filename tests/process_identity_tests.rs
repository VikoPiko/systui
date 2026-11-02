//! Tests against the REAL OS: collector output and the process action layer.
//! Termination is only ever attempted on a disposable child spawned here.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use systui::actions::process::{ActionError, OsActions, ProcessActions};
use systui::collectors::system::SystemCollector;
use systui::domain::{CmdLine, CollectorState, ProcessId, Snapshot};

struct Disposable(Child);
impl Drop for Disposable {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_sleeper() -> Disposable {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("ping");
        c.args(["-n", "120", "127.0.0.1"]);
        c
    } else {
        let mut c = Command::new("sleep");
        c.arg("120");
        c
    };
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn disposable child");
    Disposable(child)
}

fn find(s: &Snapshot, pid: u32) -> Option<(ProcessId, String)> {
    s.processes
        .iter()
        .find(|p| p.id.pid == pid)
        .map(|p| (p.id, p.name.clone()))
}

fn sample_until_found(c: &mut SystemCollector, pid: u32) -> (ProcessId, String) {
    for _ in 0..20 {
        let s = c.sample();
        if let Some(f) = find(&s, pid) {
            return f;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("collector never saw pid {pid}");
}

#[test]
fn collector_first_sample_is_warming_up_then_real_values() {
    let mut c = SystemCollector::new().expect("collector");
    let first = c.sample();
    assert!(
        first.cpu.usage.is_none(),
        "first CPU sample must not be a fake number"
    );
    assert!(first.processes.iter().all(|p| p.cpu.is_none()));
    assert!(first
        .diagnostics
        .iter()
        .any(|d| d.name == "cpu" && d.state == CollectorState::WarmingUp));
    assert!(first.network.iter().all(|n| n.rx_rate.is_none()));

    std::thread::sleep(Duration::from_millis(600));
    let second = c.sample();
    let u = second.cpu.usage.expect("CPU usage after warm-up");
    assert!((0.0..=100.0).contains(&u), "{u}");
    assert_eq!(second.cpu.per_core.len(), second.cpu.logical);
    assert!(second.cpu.logical > 0);
    assert!(second.elapsed.unwrap() >= Duration::from_millis(500));

    let m = second.memory.expect("memory");
    assert!(m.total > 0 && m.used <= m.total && m.available <= m.total);
    assert!(second.host.uptime_secs.is_some());
    assert!(!second.processes.is_empty());
    assert!(
        second.processes.len() > 20,
        "a desktop OS has many processes"
    );
}

#[test]
fn collector_sees_this_test_process_with_memory_and_identity() {
    let mut c = SystemCollector::new().unwrap();
    c.sample();
    std::thread::sleep(Duration::from_millis(600));
    let s = c.sample();
    let me = s
        .processes
        .iter()
        .find(|p| p.id.pid == std::process::id())
        .expect("own process listed");
    assert!(me.rss.unwrap() > 0);
    assert!(
        me.id.started_at.is_some(),
        "start time available => identity safe"
    );
    assert!(me.cpu.is_some(), "has a baseline after two samples");
    assert!(me.info.exe.is_some());
    assert_eq!(me.info.cmd, CmdLine::NotRequested, "cmd line is lazy");
    // No control characters anywhere in collected strings.
    assert!(s
        .processes
        .iter()
        .all(|p| !p.name.chars().any(char::is_control)));
}

#[test]
fn commandline_is_collected_only_for_watched_process() {
    let child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, _) = sample_until_found(&mut c, pid);
    c.watch(Some(id));
    let s = c.sample();
    let p = s.processes.iter().find(|p| p.id == id).unwrap();
    assert!(
        matches!(p.info.cmd, CmdLine::Available(_) | CmdLine::Unavailable),
        "requested now"
    );
    if let CmdLine::Available(cmd) = &p.info.cmd {
        assert!(cmd.contains("120"), "{cmd}");
    }
    let other = s
        .processes
        .iter()
        .find(|q| q.id != id && q.id.pid != std::process::id());
    assert!(other.unwrap().info.cmd == CmdLine::NotRequested);
}

#[test]
fn new_process_has_no_cpu_baseline_on_first_sighting() {
    let mut c = SystemCollector::new().unwrap();
    c.sample();
    std::thread::sleep(Duration::from_millis(600));
    c.sample();
    let child = spawn_sleeper();
    let pid = child.0.id();
    std::thread::sleep(Duration::from_millis(300));
    let s = c.sample();
    let p = s
        .processes
        .iter()
        .find(|p| p.id.pid == pid)
        .expect("new child listed");
    assert!(
        p.cpu.is_none(),
        "first sighting must be 'warming up', not 0%"
    );
    std::thread::sleep(Duration::from_millis(600));
    let s = c.sample();
    assert!(s
        .processes
        .iter()
        .find(|p| p.id.pid == pid)
        .unwrap()
        .cpu
        .is_some());
}

#[test]
fn exited_process_disappears_from_the_next_snapshot() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    sample_until_found(&mut c, pid);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(find(&c.sample(), pid).is_none(), "no stale row");
}

#[test]
fn network_rates_appear_on_second_sample_and_are_sane() {
    let mut c = SystemCollector::new().unwrap();
    let first = c.sample();
    std::thread::sleep(Duration::from_millis(600));
    let second = c.sample();
    if second.network.is_empty() {
        return; // headless CI box without interfaces: nothing to assert
    }
    assert!(first.network.iter().all(|n| n.rx_rate.is_none()));
    let any = second.network.iter().any(|n| n.rx_rate.is_some());
    assert!(any, "at least one interface should have a computed rate");
    assert!(second
        .network
        .iter()
        .filter_map(|n| n.rx_rate)
        .all(|r| r.is_finite() && r >= 0.0));
}

#[test]
fn volumes_have_consistent_capacity() {
    let mut c = SystemCollector::new().unwrap();
    let s = c.sample();
    for v in &s.volumes {
        assert!(v.total > 0 && v.available <= v.total, "{v:?}");
        assert!(v.used_pct() >= 0.0 && v.used_pct() <= 100.0);
    }
}

// --------------------------------------------------------- action layer

#[test]
fn pid_reuse_is_rejected_and_target_is_left_alone() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, name) = sample_until_found(&mut c, pid);
    let Some(real_start) = id.started_at else {
        eprintln!("start time unavailable on this platform; skipping");
        return;
    };
    // Same PID, but a different (earlier) start time: i.e. what a stale
    // selection looks like after the PID was recycled.
    let stale = ProcessId {
        pid,
        started_at: Some(real_start - 100),
    };
    let r = OsActions.terminate(&stale, &name, true);
    assert_eq!(r, Err(ActionError::IdentityMismatch(pid)));
    assert!(
        child.0.try_wait().unwrap().is_none(),
        "child must still be alive"
    );
}

#[test]
fn name_mismatch_is_rejected_even_with_matching_pid_and_start() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, _) = sample_until_found(&mut c, pid);
    let r = OsActions.terminate(&id, "definitely-not-this.exe", true);
    assert_eq!(r, Err(ActionError::IdentityMismatch(pid)));
    assert!(child.0.try_wait().unwrap().is_none());
}

#[test]
fn missing_process_is_not_found() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, name) = sample_until_found(&mut c, pid);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        OsActions.terminate(&id, &name, true),
        Err(ActionError::NotFound)
    );
}

#[test]
fn disposable_child_is_actually_terminated() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, name) = sample_until_found(&mut c, pid);
    OsActions
        .terminate(&id, &name, true)
        .expect("force-terminate disposable child");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "child still alive after kill");
        std::thread::sleep(Duration::from_millis(50));
    }
    // and the next sample no longer lists it
    std::thread::sleep(Duration::from_millis(200));
    assert!(find(&c.sample(), pid).is_none());
}

#[cfg(unix)]
#[test]
fn graceful_sigterm_terminates_disposable_child() {
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, name) = sample_until_found(&mut c, pid);
    assert!(OsActions.can_graceful());
    OsActions.terminate(&id, &name, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
#[test]
fn windows_has_no_graceful_option_and_says_so() {
    assert!(!OsActions.can_graceful());
    let mut child = spawn_sleeper();
    let pid = child.0.id();
    let mut c = SystemCollector::new().unwrap();
    let (id, name) = sample_until_found(&mut c, pid);
    assert_eq!(
        OsActions.terminate(&id, &name, false),
        Err(ActionError::GracefulUnsupported)
    );
    assert!(child.0.try_wait().unwrap().is_none());
}

#[test]
fn collector_worker_publishes_latest_and_shuts_down_cleanly() {
    use systui::collectors::{Cmd, Collector};
    let c = Collector::spawn(Duration::from_millis(500));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seqs = Vec::new();
    while seqs.len() < 3 && Instant::now() < deadline {
        if let Some(s) = c.take_latest() {
            seqs.push(s.seq);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seqs.len() >= 3, "worker should keep sampling: {seqs:?}");
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    c.send(Cmd::RefreshNow);
    drop(c); // joins the thread; must not hang
}

#[cfg(windows)]
#[test]
fn restricted_processes_are_unknown_not_zero_on_windows() {
    let mut c = SystemCollector::new().unwrap();
    c.sample();
    std::thread::sleep(Duration::from_millis(600));
    let s = c.sample();
    for p in &s.processes {
        if p.restricted {
            assert!(
                p.rss.is_none() && p.cpu.is_none(),
                "{} must be unknown",
                p.name
            );
        } else {
            assert!(
                p.rss.is_some_and(|r| r > 0),
                "{} has a real reading",
                p.name
            );
        }
    }
}

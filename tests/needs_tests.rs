mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::{Duration, Instant};
use systui::app::{App, Effect, Tab};
use systui::collectors::system::SystemCollector;
use systui::collectors::{Cmd, Collector};
use systui::config::Config;
use systui::domain::{Needs, NetNeed};
use systui::ui::{self, theme::Theme, RenderOpts};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn needs_effects(a: &mut App) -> Vec<Needs> {
    a.take_effects()
        .into_iter()
        .filter_map(|e| {
            if let Effect::Needs(n) = e {
                Some(n)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn needs_follow_the_visible_tab() {
    let mut a = App::new(Config::default());
    let n = |t| {
        let mut a = App::new(Config::default());
        a.select_tab(t);
        a.desired_needs()
    };
    assert_eq!(
        n(Tab::Overview),
        Needs {
            processes: true,
            process_io: false,
            network: NetNeed::Slow
        }
    );
    assert_eq!(
        n(Tab::Processes),
        Needs {
            processes: true,
            process_io: false,
            network: NetNeed::Off
        }
    );
    assert_eq!(
        n(Tab::Network),
        Needs {
            processes: false,
            process_io: false,
            network: NetNeed::Fast
        }
    );
    assert_eq!(
        n(Tab::Storage),
        Needs {
            processes: false,
            process_io: false,
            network: NetNeed::Off
        }
    );
    assert_eq!(
        n(Tab::Developer),
        Needs {
            processes: true,
            process_io: false,
            network: NetNeed::Off
        }
    );
    // initial needs are emitted once, then only on change
    assert_eq!(needs_effects(&mut a).len(), 1);
    assert!(needs_effects(&mut a).is_empty());
    a.handle_key(k(KeyCode::Char('4')));
    assert_eq!(needs_effects(&mut a), vec![n(Tab::Storage)]);
    a.handle_key(k(KeyCode::Char('j'))); // same needs: no effect
    assert!(needs_effects(&mut a).is_empty());
}

#[test]
fn io_toggle_only_applies_on_the_processes_tab() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot(10)), Instant::now());
    a.select_tab(Tab::Processes);
    needs_effects(&mut a);
    a.handle_key(k(KeyCode::Char('i')));
    assert!(a.proc_io);
    let n = needs_effects(&mut a);
    assert_eq!(n.len(), 1);
    assert!(n[0].process_io);
    a.handle_key(k(KeyCode::Char('1')));
    assert!(
        !needs_effects(&mut a)[0].process_io,
        "I/O collection stops off the Processes tab"
    );
    a.handle_key(k(KeyCode::Char('2')));
    assert!(
        needs_effects(&mut a)[0].process_io,
        "...and resumes when you come back"
    );
}

// ------------------------------------------------------------ real collector

fn warm(c: &mut SystemCollector) {
    c.sample();
    std::thread::sleep(Duration::from_millis(600));
    c.sample();
}

#[test]
fn skipped_subsystems_are_flagged_stale_and_reuse_last_data() {
    let mut c = SystemCollector::new().unwrap();
    warm(&mut c);
    let before = c.sample();
    assert!(before.processes_fresh && before.network_fresh);

    let turned_on = c.set_needs(Needs {
        processes: false,
        process_io: false,
        network: NetNeed::Off,
    });
    assert!(!turned_on, "turning things OFF needs no re-baseline");
    let s = c.sample();
    assert!(!s.processes_fresh && !s.network_fresh);
    assert_eq!(
        s.processes.len(),
        before.processes.len(),
        "last known list is carried"
    );
    assert!(s.cpu.usage.is_some(), "CPU/RAM are always sampled");
    assert!(s.memory.is_some());
}

#[test]
fn resuming_processes_restarts_cpu_warmup_instead_of_showing_a_long_average() {
    let mut c = SystemCollector::new().unwrap();
    warm(&mut c);
    c.set_needs(Needs {
        processes: false,
        process_io: false,
        network: NetNeed::Off,
    });
    c.sample();
    std::thread::sleep(Duration::from_millis(1200)); // a "long gap"
    assert!(
        c.set_needs(Needs::all()),
        "turning ON asks for a quick re-sample"
    );
    let first = c.sample();
    assert!(first.processes_fresh);
    assert!(
        first.processes.iter().all(|p| p.cpu.is_none()),
        "no CPU % across the gap: must say warming up"
    );
    assert!(
        first.network.iter().all(|n| n.rx_rate.is_none()),
        "no rate across the gap either"
    );
    std::thread::sleep(Duration::from_millis(600));
    let second = c.sample();
    assert!(second.processes.iter().any(|p| p.cpu.is_some()));
}

#[test]
fn slow_network_refreshes_at_most_every_two_seconds() {
    let mut c = SystemCollector::new().unwrap();
    c.set_needs(Needs {
        processes: false,
        process_io: false,
        network: NetNeed::Slow,
    });
    let a = c.sample();
    assert!(a.network_fresh, "first Slow sample refreshes");
    std::thread::sleep(Duration::from_millis(300));
    let b = c.sample();
    assert!(!b.network_fresh, "within 2 s: reuse, flagged not fresh");
    assert_eq!(a.network.len(), b.network.len());
    std::thread::sleep(Duration::from_millis(2000));
    assert!(c.sample().network_fresh, "after 2 s it refreshes again");
}

#[test]
fn process_io_needs_a_baseline_then_reports_finite_non_negative_rates() {
    let mut c = SystemCollector::new().unwrap();
    let mut n = Needs::all();
    n.process_io = true;
    c.set_needs(n);
    warm(&mut c); // io_warm becomes true during these samples
    std::thread::sleep(Duration::from_millis(600));
    let s = c.sample();
    let with_io: Vec<_> = s.processes.iter().filter_map(|p| p.io).collect();
    assert!(!with_io.is_empty(), "some readable processes report I/O");
    assert!(with_io
        .iter()
        .all(|(r, w)| r.is_finite() && *r >= 0.0 && w.is_finite() && *w >= 0.0));
    assert!(
        s.processes
            .iter()
            .filter(|p| p.restricted)
            .all(|p| p.io.is_none()),
        "denied processes: unknown, not 0"
    );

    // Turning I/O off drops it again; turning it on re-baselines (no stale delta).
    c.set_needs(Needs::all());
    assert!(c.sample().processes.iter().all(|p| p.io.is_none()));
    n.process_io = true;
    c.set_needs(n);
    assert!(
        c.sample().processes.iter().all(|p| p.io.is_none()),
        "first sample after enabling"
    );
}

#[test]
fn worker_applies_needs_and_republishes_quickly_on_resume() {
    let c = Collector::spawn(Duration::from_millis(500));
    let wait = |pred: &dyn Fn(&systui::domain::Snapshot) -> bool, secs: u64| {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Some(s) = c.take_latest() {
                if pred(&s) {
                    return Some(s);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    };
    assert!(wait(&|s| s.processes_fresh, 10).is_some());
    c.send(Cmd::Needs(Needs {
        processes: false,
        process_io: false,
        network: NetNeed::Off,
    }));
    let stale = wait(&|s| !s.processes_fresh && !s.network_fresh, 10).expect("stale snapshot");
    assert!(!stale.processes.is_empty());
    c.send(Cmd::Needs(Needs::all()));
    // resume => immediate fresh sample (does not wait for the next interval)
    let t = Instant::now();
    let back = wait(&|s| s.processes_fresh, 3).expect("fresh again");
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
    assert!(back.processes.iter().all(|p| p.cpu.is_none()));
}

// ------------------------------------------------------------------- UI side

fn draw(a: &mut App, w: u16, h: u16) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(f, a, &opts)).unwrap();
    let b = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn stale_data_is_never_rendered_as_live() {
    let mut a = App::new(Config::default());
    let mut s = snapshot(30);
    s.processes_fresh = false;
    s.network_fresh = false;
    a.ingest(Arc::new(s), Instant::now());
    for (tab, want) in [
        (Tab::Processes, "Refreshing the process list"),
        (Tab::Network, "Refreshing network counters"),
        (Tab::Developer, "Refreshing the process list"),
    ] {
        a.select_tab(tab);
        let out = draw(&mut a, 120, 40);
        assert!(out.contains(want), "{tab:?}:\n{out}");
        assert!(!out.contains("proc0001"), "no stale rows on {tab:?}");
    }
    a.select_tab(Tab::Overview);
    assert!(
        draw(&mut a, 120, 40).contains("Refreshing the process list"),
        "overview top list"
    );
}

#[test]
fn stale_samples_do_not_pollute_history_or_selection() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot(30)), Instant::now());
    a.select_tab(Tab::Processes);
    let (net_len, hist_len) = (a.hist.net_rx.len(), a.procs.hist_rss.len());
    let mut s = snapshot_with(2, many_procs(30));
    s.processes_fresh = false;
    s.network_fresh = false;
    for _ in 0..5 {
        a.ingest(Arc::new(s.clone()), Instant::now());
    }
    assert_eq!(
        a.hist.net_rx.len(),
        net_len,
        "network history unchanged by non-fresh samples"
    );
    assert_eq!(
        a.procs.hist_rss.len(),
        hist_len,
        "process history unchanged"
    );
}

#[test]
fn kill_confirmation_ignores_non_fresh_process_lists() {
    // A stale list that still contains the process must not be read as "still running"
    // (and an absent one must not be read as "exited").
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot(30)), Instant::now());
    a.select_tab(Tab::Processes);
    let mut s = snapshot_with(2, many_procs(30));
    s.processes_fresh = false;
    for _ in 0..3 {
        a.ingest(Arc::new(s.clone()), Instant::now());
    }
    assert!(a.notice.is_none());
}

#[test]
fn io_columns_render_with_honest_placeholders() {
    let mut a = App::new(Config::default());
    let mut s = snapshot(10);
    s.processes[0].io = Some((2048.0, 1024.0));
    s.processes[1].io = None; // warming up
    s.processes[2].io = None;
    s.processes[2].restricted = true;
    s.processes[2].rss = None;
    a.ingest(Arc::new(s), Instant::now());
    a.select_tab(Tab::Processes);
    assert!(!draw(&mut a, 120, 40).contains("READ/s"), "off by default");
    a.handle_key(k(KeyCode::Char('i')));
    let out = draw(&mut a, 120, 40);
    assert!(
        out.contains("READ/s") && out.contains("WRITE/s") && out.contains("I/O on"),
        "{out}"
    );
    assert!(out.contains("2.0 KiB/s") && out.contains("1.0 KiB/s"));
    assert!(out.contains("…"), "warming up placeholder");
    assert!(out.contains("denied"));
}

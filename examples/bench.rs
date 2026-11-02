//! Headless benchmark of the real pipeline: collector thread -> App -> render
//! (ratatui TestBackend, so terminal I/O cost is NOT included).
//!
//!   cargo run --release --example bench -- [seconds=60] [refresh_ms=1000]
//!
//! Reports startup, steady-state CPU/RSS of this process, render and input
//! latency percentiles. See PERFORMANCE.md for how to interpret it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use systui::app::{App, Effect, Tab};
use systui::collectors::{Cmd, Collector};
use systui::config::Config;
use systui::modules::ModuleSet;
use systui::ui::{self, theme::Theme, RenderOpts};

fn pct(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn report(name: &str, mut v: Vec<Duration>) {
    v.sort();
    println!(
        "{name:<28} n={:<5} p50={:>8.3} ms  p95={:>8.3} ms  max={:>8.3} ms",
        v.len(),
        pct(&v, 0.5).as_secs_f64() * 1e3,
        pct(&v, 0.95).as_secs_f64() * 1e3,
        v.last().map_or(0.0, |d| d.as_secs_f64() * 1e3)
    );
}

fn own_stats(sys: &mut System, pid: Pid) -> (u64, u64) {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    let p = sys.process(pid).expect("own process");
    (p.accumulated_cpu_time(), p.memory())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let secs: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(60);
    let refresh: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1000);

    let t0 = Instant::now();
    let mut cfg = Config::default();
    cfg.ui.refresh_ms = refresh;
    // BENCH_MODULES=1: enable Docker + WSL + GPU and keep the Dev tab open.
    let with_modules = std::env::var_os("BENCH_MODULES").is_some();
    cfg.modules.docker = with_modules;
    cfg.modules.wsl = with_modules;
    cfg.modules.gpu = with_modules;
    let mut app = App::new(cfg);
    let opts = RenderOpts {
        theme: Theme::dark(),
        unicode: true,
    };
    let mut term = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
    let collector = Collector::spawn(Duration::from_millis(refresh));
    let mut modules = ModuleSet::new(&app.config);
    let first = loop {
        if let Some(s) = collector.take_latest() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    app.ingest(first, Instant::now());
    term.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
    println!(
        "time to first sample + first frame: {:.0} ms",
        t0.elapsed().as_secs_f64() * 1e3
    );
    println!(
        "process count on this host: {}",
        app.snapshot.as_ref().map_or(0, |s| s.processes.len())
    );

    // --- latency (let CPU warm up first)
    std::thread::sleep(Duration::from_millis(1200));
    if let Some(s) = collector.take_latest() {
        app.ingest(s, Instant::now());
    }
    for (tab, name) in [
        (Tab::Overview, "render overview 120x40"),
        (Tab::Processes, "render processes 120x40"),
        (Tab::Network, "render network 120x40"),
        (Tab::Storage, "render storage 120x40"),
    ] {
        app.select_tab(tab);
        let mut v = Vec::with_capacity(500);
        for _ in 0..500 {
            let t = Instant::now();
            term.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
            v.push(t.elapsed());
        }
        report(name, v);
    }
    app.select_tab(Tab::Processes);
    let mut v = Vec::with_capacity(500);
    for i in 0..500 {
        let key = KeyEvent::new(
            if i % 2 == 0 {
                KeyCode::Char('j')
            } else {
                KeyCode::Char('k')
            },
            KeyModifiers::NONE,
        );
        let t = Instant::now();
        app.handle_key(key);
        term.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
        v.push(t.elapsed());
    }
    report("keypress + redraw (procs)", v);
    let snap = app.snapshot.clone().unwrap();
    let mut v = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        app.ingest(snap.clone(), Instant::now());
        v.push(t.elapsed());
    }
    report("ingest (sort+histories)", v);

    // --- steady state: real cadence, redraw once per new snapshot (as the UI does)
    let mut sys = System::new().expect("system");
    let me = Pid::from_u32(std::process::id());
    let tab = match std::env::var("BENCH_TAB").as_deref() {
        Ok("processes") => Tab::Processes,
        Ok("network") => Tab::Network,
        Ok("storage") => Tab::Storage,
        Ok("dev") => Tab::Developer,
        Ok("services") => Tab::Services,
        _ if with_modules => Tab::Developer,
        _ => Tab::Overview,
    };
    app.select_tab(tab);
    println!("steady-state tab: {tab:?}");
    let (cpu0, _) = own_stats(&mut sys, me);
    let start = Instant::now();
    let mut peak = 0u64;
    let mut last_probe = Instant::now();
    let mut frames = 0u32;
    let mut module_updates = 0u32;
    while start.elapsed() < Duration::from_secs(secs) {
        for e in app.take_effects() {
            match e {
                Effect::DevVisible(v) => modules.set_visible(v),
                Effect::Needs(n) => collector.send(Cmd::Needs(n)),
                _ => {}
            }
        }
        if modules.started() {
            let u = modules.take();
            if u.docker.is_some() || u.wsl.is_some() || u.gpu.is_some() {
                app.ingest_modules(u, Instant::now());
                module_updates += 1;
                term.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
            }
        }
        if let Some(s) = collector.take_latest() {
            app.ingest(s, Instant::now());
            term.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
            frames += 1;
        }
        std::thread::sleep(Duration::from_millis(100)); // same idle wake as the real loop
        if last_probe.elapsed() >= Duration::from_secs(5) {
            peak = peak.max(own_stats(&mut sys, me).1);
            last_probe = Instant::now();
        }
    }
    let wall = start.elapsed();
    let (cpu1, rss) = own_stats(&mut sys, me);
    peak = peak.max(rss);
    let cpu_ms = cpu1.saturating_sub(cpu0);
    println!();
    println!(
        "steady state: {secs}s at refresh_ms={refresh}, {frames} frames, modules {} ({module_updates} module updates)",
        if with_modules { "ENABLED + Dev tab open" } else { "off" }
    );
    println!(
        "  CPU time used: {cpu_ms} ms over {:.1} s wall = {:.2}% of one logical CPU",
        wall.as_secs_f64(),
        cpu_ms as f64 / 1000.0 / wall.as_secs_f64() * 100.0
    );
    println!(
        "  RSS: end {:.1} MiB, peak {:.1} MiB",
        rss as f64 / 1048576.0,
        peak as f64 / 1048576.0
    );
    println!("  (excludes real terminal I/O; includes collector thread + UI-loop wakeups)");
}

mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use systui::app::{App, Effect, Tab};
use systui::config::{CheckKind, Config};
use systui::health::{specs_from_config, Status};
use systui::modules::health::{CheckState, HealthHandle, HealthSnapshot};
use systui::ui::{self, theme::Theme, RenderOpts};

fn parse(s: &str) -> Result<Config, systui::config::ConfigError> {
    Config::from_toml(s, Path::new("t.toml"))
}

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

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

// ------------------------------------------------------------------- config

#[test]
fn shipped_example_config_is_valid_and_its_check_is_disabled() {
    let t = std::fs::read_to_string("config/config.example.toml").unwrap();
    let c = parse(&t).unwrap();
    assert!(!c.modules.health, "health is off by default");
    assert!(!c.alerts.enabled, "alerts are off by default");
    assert!(
        c.health.checks.iter().all(|c| !c.enabled),
        "shipped example checks are disabled"
    );
    assert!(specs_from_config(&c).is_none());
}

#[test]
fn health_config_parses_all_four_types() {
    let c = parse(
        "[modules]\nhealth = true\n\
         [[health.checks]]\nname = \"api\"\ntype = \"http\"\nurl = \"http://127.0.0.1:7141/health\"\ninterval_seconds = 10\ntimeout_ms = 500\n\
         [[health.checks]]\nname = \"db\"\ntype = \"tcp\"\nhost = \"localhost\"\nport = 5432\n\
         [[health.checks]]\nname = \"worker\"\ntype = \"process\"\nprocess = \"node\"\n\
         [[health.checks]]\nname = \"svc\"\ntype = \"service\"\nservice = \"nginx\"\nenabled = false\n",
    )
    .unwrap();
    let specs = specs_from_config(&c).unwrap();
    assert_eq!(specs.len(), 4);
    assert_eq!(specs[0].kind, CheckKind::Http);
    assert_eq!(specs[0].interval, Duration::from_secs(10));
    assert_eq!(
        specs[1].timeout,
        Duration::from_millis(1000),
        "default timeout 1 s"
    );
    assert_eq!(
        specs[1].interval,
        Duration::from_secs(30),
        "default interval 30 s"
    );
    assert!(!specs[3].enabled);
}

#[test]
fn non_local_or_unsafe_targets_are_rejected_at_load_time() {
    let bad = [
        ("http", "url = \"http://example.com/health\"", "not allowed"),
        ("http", "url = \"https://127.0.0.1/health\"", "TLS"),
        ("http", "url = \"http://user:pw@127.0.0.1/\"", "credentials"),
        ("http", "url = \"http://192.168.1.10/\"", "not local"),
        ("http", "url = \"http://8.8.8.8/\"", "not local"),
        ("tcp", "host = \"example.com\"\nport = 443", "not allowed"),
        ("tcp", "host = \"127.0.0.1\"", "port"),
        ("tcp", "host = \"0.0.0.0\"\nport = 80", "not a destination"),
        ("process", "", "process"),
        ("service", "service = \"a; rm -rf /\"", "service name"),
    ];
    for (kind, body, want) in bad {
        let e = parse(&format!(
            "[[health.checks]]\nname = \"x\"\ntype = \"{kind}\"\n{body}\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(e.contains(want), "{kind} {body}: {e}");
    }
}

#[test]
fn lan_addresses_need_the_explicit_flag() {
    let body =
        "[[health.checks]]\nname = \"nas\"\ntype = \"tcp\"\nhost = \"192.168.1.20\"\nport = 445\n";
    assert!(parse(body).is_err());
    assert!(parse(&format!("[health]\nallow_lan = true\n{body}")).is_ok());
    // public addresses are refused even with allow_lan
    let public = "[health]\nallow_lan = true\n[[health.checks]]\nname = \"p\"\ntype = \"tcp\"\nhost = \"1.1.1.1\"\nport = 53\n";
    assert!(parse(public).is_err());
}

#[test]
fn check_set_and_timing_bounds() {
    let mk = |extra: &str| {
        parse(&format!(
            "[[health.checks]]\nname = \"a\"\ntype = \"tcp\"\nhost = \"127.0.0.1\"\nport = 1\n{extra}\n"
        ))
    };
    assert!(mk("interval_seconds = 1").is_err());
    assert!(mk("timeout_ms = 20").is_err());
    assert!(mk("timeout_ms = 99999").is_err());
    assert!(
        mk("interval_seconds = 5\ntimeout_ms = 6000").is_err(),
        "timeout must be below the interval"
    );
    assert!(mk("interval_seconds = 5\ntimeout_ms = 1000").is_ok());
    let dup = "[[health.checks]]\nname = \"a\"\ntype = \"process\"\nprocess = \"x\"\n\
               [[health.checks]]\nname = \"A\"\ntype = \"process\"\nprocess = \"y\"\n";
    assert!(parse(dup).unwrap_err().to_string().contains("duplicate"));
    assert!(
        parse("[[health.checks]]\nname = \"\"\ntype = \"process\"\nprocess = \"x\"\n").is_err()
    );
}

#[test]
fn alert_config_bounds() {
    for bad in [
        "cpu_percent = 0",
        "cpu_percent = 150",
        "memory_percent = -3",
        "sustain_seconds = 0",
        "service_failures = 0",
        "hysteresis_points = 80",
    ] {
        assert!(parse(&format!("[alerts]\n{bad}\n")).is_err(), "{bad}");
    }
    let c = parse("[alerts]\nenabled = true\ncpu_percent = 80\n").unwrap();
    assert!(c.alerts.enabled);
    assert_eq!(c.alerts.cpu_percent, 80.0);
    assert_eq!(c.alerts.memory_percent, 90.0, "spec default");
    assert_eq!(
        c.alerts.sustain_seconds, 15,
        "spec default: sustained for 15 s"
    );
    assert_eq!(c.alerts.disk_free_percent_below, 10.0);
    assert_eq!(c.alerts.service_failures, 3);
}

// -------------------------------------------------------- alerts via the app

fn alert_cfg() -> Config {
    let mut c = Config::default();
    c.alerts.enabled = true;
    c
}

fn snap_cpu(cpu: Option<f32>, seq: u64) -> Arc<systui::domain::Snapshot> {
    let mut s = snapshot_with(seq, many_procs(5));
    s.cpu.usage = cpu;
    s.volumes[1].available = s.volumes[1].total / 2; // fixture default is nearly full; keep disk quiet here
    Arc::new(s)
}

#[test]
fn sustained_cpu_raises_a_visible_alert_and_recovery_clears_it() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    for i in 0..14 {
        a.ingest(snap_cpu(Some(97.0), i), t0 + Duration::from_secs(i));
    }
    assert!(
        a.active_alerts().is_empty(),
        "not sustained long enough yet"
    );
    a.ingest(snap_cpu(Some(97.0), 15), t0 + Duration::from_secs(15));
    assert_eq!(a.active_alerts().len(), 1);
    assert!(a.notice.as_deref().unwrap().starts_with("ALERT:"));
    let s = draw(&mut a, 120, 40);
    assert!(s.contains("ALERT 1"), "header badge:\n{s}");
    assert!(s.contains("CPU high"), "overview shows it");
    for i in 16..30 {
        a.ingest(snap_cpu(Some(20.0), i), t0 + Duration::from_secs(i));
    }
    assert!(a.active_alerts().is_empty());
    assert!(a.alert_log.iter().any(|l| l.contains("RAISED")));
    assert!(a.alert_log.iter().any(|l| l.contains("cleared")));
    assert!(!draw(&mut a, 120, 40).contains("ALERT 1"));
}

#[test]
fn alerts_keep_evaluating_while_the_display_is_paused() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    a.ingest(snap_cpu(Some(97.0), 0), t0);
    a.handle_key(k(KeyCode::Char(' ')));
    for i in 1..=16 {
        a.ingest(snap_cpu(Some(97.0), i), t0 + Duration::from_secs(i));
    }
    assert_eq!(
        a.active_alerts().len(),
        1,
        "pause freezes the view, not the watchdog"
    );
}

#[test]
fn warming_up_cpu_never_raises_an_alert() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    for i in 0..60 {
        a.ingest(snap_cpu(None, i), t0 + Duration::from_secs(i));
    }
    assert!(a.active_alerts().is_empty());
}

#[test]
fn alerts_disabled_by_default_do_nothing() {
    let mut a = App::new(Config::default());
    let t0 = Instant::now();
    for i in 0..60 {
        a.ingest(snap_cpu(Some(100.0), i), t0 + Duration::from_secs(i));
    }
    assert!(a.active_alerts().is_empty() && a.alert_log.is_empty());
    a.select_tab(Tab::Services);
    assert!(draw(&mut a, 120, 40).contains("Alerts (off)"));
}

#[test]
fn low_disk_alert_flows_through_the_app() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    for i in 0..=16 {
        let mut s = snapshot_with(i, many_procs(3));
        s.volumes[1].available = s.volumes[1].total / 2;
        s.volumes[0].available = s.volumes[0].total / 50; // 2 % free
        a.ingest(Arc::new(s), t0 + Duration::from_secs(i));
    }
    let al = a.active_alerts();
    assert_eq!(al.len(), 1);
    assert!(
        al[0].key.label().contains("Low disk space on /"),
        "{:?}",
        al[0].key
    );
}

fn health_snapshot(states: Vec<CheckState>) -> Arc<HealthSnapshot> {
    Arc::new(HealthSnapshot {
        seq: 1,
        checks: states,
    })
}

fn state(name: &str, status: Status, fails: u32, last_ok: bool) -> CheckState {
    let now = SystemTime::now();
    CheckState {
        name: name.into(),
        kind: CheckKind::Http,
        target: "http://127.0.0.1:7141/health?…".into(),
        enabled: true,
        status,
        latency: Some(Duration::from_millis(12)),
        message: Some("server answered with status 503".into()),
        last_checked: Some(now),
        last_success: last_ok.then_some(now - Duration::from_secs(95)),
        next_due: Some(now + Duration::from_secs(20)),
        consecutive_failures: fails,
        interval: Duration::from_secs(30),
        timeout: Duration::from_millis(1000),
    }
}

#[test]
fn service_failures_raise_after_the_configured_count_only() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    a.ingest(snap_cpu(Some(5.0), 0), t0);
    a.ingest_health(
        health_snapshot(vec![state("api", Status::HttpResponded(503), 2, false)]),
        t0,
    );
    assert!(a.active_alerts().is_empty(), "2 < 3");
    a.ingest_health(
        health_snapshot(vec![state("api", Status::HttpResponded(503), 3, false)]),
        t0 + Duration::from_secs(30),
    );
    assert_eq!(a.active_alerts().len(), 1);
    assert!(a.active_alerts()[0].key.label().contains("api"));
    a.ingest_health(
        health_snapshot(vec![state("api", Status::Healthy, 0, true)]),
        t0 + Duration::from_secs(60),
    );
    assert!(a.active_alerts().is_empty());
}

#[test]
fn a_port_that_is_merely_open_never_counts_as_a_failure() {
    let mut a = App::new(alert_cfg());
    let t0 = Instant::now();
    for i in 0..10 {
        a.ingest_health(
            health_snapshot(vec![state("db", Status::PortOpen, 0, true)]),
            t0 + Duration::from_secs(i * 30),
        );
    }
    assert!(a.active_alerts().is_empty());
}

// ------------------------------------------------------------- services tab

#[test]
fn services_tab_explains_when_health_is_off_and_when_no_checks_exist() {
    let mut a = App::new(Config::default());
    a.ingest(snap_cpu(Some(5.0), 0), Instant::now());
    a.select_tab(Tab::Services);
    let s = draw(&mut a, 120, 40);
    assert!(
        s.contains("Health checks are disabled") && s.contains("[modules] health = true"),
        "{s}"
    );
    let mut cfg = Config::default();
    cfg.modules.health = true;
    let mut b = App::new(cfg);
    b.ingest(snap_cpu(Some(5.0), 0), Instant::now());
    b.select_tab(Tab::Services);
    assert!(draw(&mut b, 120, 40).contains("no [[health.checks]]"));
}

#[test]
fn services_table_distinguishes_states_and_hides_secrets() {
    let mut cfg = Config::default();
    cfg.modules.health = true;
    cfg.health.checks.push(Default::default());
    let mut a = App::new(cfg);
    a.ingest(snap_cpu(Some(5.0), 0), Instant::now());
    let mut stale = state("old", Status::Healthy, 0, true);
    stale.last_checked = Some(SystemTime::now() - Duration::from_secs(600));
    let mut disabled = state("off", Status::Unknown, 0, false);
    disabled.enabled = false;
    let mut unknown = state("fresh", Status::Unknown, 0, false);
    unknown.last_checked = None;
    unknown.latency = None;
    unknown.message = None;
    a.ingest_health(
        health_snapshot(vec![
            state("api", Status::HttpResponded(503), 1, true),
            state("db", Status::PortOpen, 0, true),
            state("web", Status::Healthy, 0, true),
            state("gone", Status::NotReachable, 4, false),
            stale,
            disabled,
            unknown,
        ]),
        Instant::now(),
    );
    a.select_tab(Tab::Services);
    let s = draw(&mut a, 150, 40);
    for want in [
        "http 503",
        "port open",
        "healthy",
        "not reachable",
        "STALE",
        "disabled",
        "unknown",
        "12 ms",
        "1m 35s ago",
        "in ",
    ] {
        assert!(s.contains(want), "missing `{want}`:\n{s}");
    }
    assert!(s.contains("http://127.0.0.1:7141/health?…"));
    assert!(!s.contains("token") && !s.contains("SECRET"));
    assert!(s.contains("not proof the app is healthy"));
    for (w, h) in [(120, 40), (90, 26), (70, 20), (60, 18), (0, 0), (40, 10)] {
        let _ = draw(&mut a, w, h);
    }
}

#[test]
fn r_on_services_requests_checks_only_when_configured() {
    let mut a = App::new(Config::default());
    a.select_tab(Tab::Services);
    a.take_effects();
    a.handle_key(k(KeyCode::Char('R')));
    assert!(!a.take_effects().contains(&Effect::HealthCheckNow));
    assert!(a.notice.as_deref().unwrap().contains("not enabled"));
    let mut cfg = Config::default();
    cfg.modules.health = true;
    cfg.health.checks.push(Default::default());
    let mut b = App::new(cfg);
    b.select_tab(Tab::Services);
    b.take_effects();
    b.handle_key(k(KeyCode::Char('R')));
    assert!(b.take_effects().contains(&Effect::HealthCheckNow));
}

// ------------------------------------- end to end with a disposable local server

#[test]
fn end_to_end_disposable_server_going_down_raises_then_clears_an_alert() {
    // A tiny HTTP server we can switch between 200 and 503.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let hh = healthy.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            let reply: &[u8] = if hh.load(std::sync::atomic::Ordering::SeqCst) {
                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
            } else {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n"
            };
            let _ = s.write_all(reply);
        }
    });

    let mut cfg = alert_cfg();
    cfg.alerts.service_failures = 2;
    cfg.modules.health = true;
    cfg.health.checks.push(systui::config::CheckConfig {
        name: "disposable".into(),
        kind: CheckKind::Http,
        url: Some(format!("http://127.0.0.1:{port}/health")),
        interval_seconds: 5,
        timeout_ms: 500,
        ..Default::default()
    });
    let specs = specs_from_config(&cfg).unwrap();
    let h = HealthHandle::spawn(specs);
    let mut app = App::new(cfg);
    app.ingest(snap_cpu(Some(5.0), 0), Instant::now());

    let pump = |app: &mut App, secs: u64, until: &dyn Fn(&App) -> bool| {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if let Some(s) = h.take_latest() {
                app.ingest_health(s, Instant::now());
            }
            if until(app) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    };
    assert!(
        pump(&mut app, 10, &|a| a
            .health
            .as_ref()
            .is_some_and(|h| h.checks[0].status == Status::Healthy)),
        "healthy while the server answers 200"
    );
    assert!(app.active_alerts().is_empty());

    healthy.store(false, std::sync::atomic::Ordering::SeqCst);
    h.check_now();
    let first = pump(&mut app, 10, &|a| {
        a.health
            .as_ref()
            .is_some_and(|h| h.checks[0].consecutive_failures >= 1)
    });
    assert!(first);
    assert!(app.active_alerts().is_empty(), "1 failure < 2 required");
    h.check_now();
    assert!(
        pump(&mut app, 10, &|a| !a.active_alerts().is_empty()),
        "second consecutive failure raises the alert"
    );
    let shown = {
        app.select_tab(Tab::Services);
        draw(&mut app, 150, 40)
    };
    assert!(
        shown.contains("http 503") && shown.contains("Service unhealthy: disposable"),
        "{shown}"
    );

    healthy.store(true, std::sync::atomic::Ordering::SeqCst);
    h.check_now();
    assert!(
        pump(&mut app, 10, &|a| a.active_alerts().is_empty()),
        "recovers => cleared"
    );
}

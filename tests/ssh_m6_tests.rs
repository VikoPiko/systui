mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use systui::app::{App, Effect, Mode};
use systui::config::{Config, SshHostConfig};
use systui::modules::ssh::{HostState, SshCmd, SshHandle, SshSnapshot, SshStatus};
use systui::ssh::{Failure, RemoteStatus};
use systui::ui::{self, theme::Theme, RenderOpts};
use systui::util::exec::Output;

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn press(a: &mut App, s: &str) {
    for ch in s.chars() {
        a.handle_key(k(KeyCode::Char(ch)));
    }
}
fn eff(a: &mut App) -> Vec<Effect> {
    a.take_effects()
        .into_iter()
        .filter(|e| !matches!(e, Effect::Needs(_)))
        .collect()
}
fn parse(s: &str) -> Result<Config, systui::config::ConfigError> {
    Config::from_toml(s, Path::new("t.toml"))
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

fn host(name: &str, addr: &str) -> SshHostConfig {
    SshHostConfig {
        name: name.into(),
        host: addr.into(),
        user: Some("viko".into()),
        ..Default::default()
    }
}

fn cfg_with(hosts: Vec<SshHostConfig>, enabled: bool) -> Config {
    let mut c = Config::default();
    c.modules.ssh = enabled;
    c.ssh.hosts = hosts;
    c
}

fn app(hosts: Vec<SshHostConfig>) -> App {
    let mut a = App::new(cfg_with(hosts, true));
    a.ingest(Arc::new(snapshot(5)), Instant::now());
    a
}

fn to_services_ssh(a: &mut App) {
    press(a, "6");
    a.handle_key(k(KeyCode::Tab)); // focus the SSH table
}

const OK_OUT: &str = "0.52 0.58 0.59 1/467 12345\n#SYSTUI#\n\
MemTotal: 16000000 kB\nMemAvailable: 4000000 kB\n#SYSTUI#\n\
/dev/sda2 100000000000 40000000000 60000000000 40% /\n#SYSTUI#\n98765.4\n#SYSTUI#\nactive\ninactive\n";

fn snap_of(states: Vec<HostState>) -> Arc<SshSnapshot> {
    Arc::new(SshSnapshot {
        seq: 1,
        hosts: states,
    })
}

fn state_for(a: &App, i: usize) -> HostState {
    HostState::new(&a.ssh_specs[i])
}

// -------------------------------------------------------------------- config

#[test]
fn ssh_is_off_by_default_and_hosts_are_ignored_unless_enabled() {
    let c = Config::default();
    assert!(!c.modules.ssh && c.ssh.hosts.is_empty());
    let a = App::new(cfg_with(vec![host("lab", "10.0.0.5")], false));
    assert!(
        a.ssh_specs.is_empty(),
        "configured hosts are inert without [modules] ssh = true"
    );
}

#[test]
fn hosts_parse_with_defaults_and_validate_strictly() {
    let c = parse(
        "[modules]\nssh = true\n[[ssh.hosts]]\nname = \"lab\"\nhost = \"192.168.1.50\"\nuser = \"viko\"\nport = 2222\ninterval_seconds = 120\nunits = [\"docker\", \"nginx\"]\n",
    )
    .unwrap();
    assert_eq!(c.ssh.hosts[0].units, ["docker", "nginx"]);
    let d = parse("[[ssh.hosts]]\nname = \"x\"\nhost = \"h\"\n").unwrap();
    assert_eq!(
        (d.ssh.hosts[0].interval_seconds, d.ssh.hosts[0].enabled),
        (60, true)
    );
    for (body, want) in [
        ("host = \"-oProxyCommand=calc\"", "invalid"),
        ("host = \"a b\"", "invalid"),
        ("host = \"h\"\nuser = \"-l\"", "user"),
        ("host = \"h\"\nport = 0", "port"),
        ("host = \"h\"\ninterval_seconds = 5", "15-3600"),
        ("host = \"h\"\nunits = [\"x; reboot\"]", "unit"),
        ("host = \"\"", "invalid"),
    ] {
        let e = parse(&format!("[[ssh.hosts]]\nname = \"x\"\n{body}\n"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "{body}: {e}");
    }
    let dup =
        "[[ssh.hosts]]\nname = \"a\"\nhost = \"h\"\n[[ssh.hosts]]\nname = \"A\"\nhost = \"h2\"\n";
    assert!(parse(dup).unwrap_err().to_string().contains("duplicate"));
    let many: String = (0..9)
        .map(|i| format!("[[ssh.hosts]]\nname = \"h{i}\"\nhost = \"x\"\n"))
        .collect();
    assert!(parse(&many).is_err(), "max 8 hosts");
}

#[test]
fn example_config_documents_ssh_and_still_parses() {
    let t = std::fs::read_to_string("config/config.example.toml").unwrap();
    for w in [
        "[ssh",
        "ssh = false",
        "[recording]",
        "[docker]",
        "allow_actions",
    ] {
        assert!(t.contains(w), "example config mentions {w}");
    }
    let c = parse(&t).unwrap();
    assert!(
        !c.modules.ssh && c.ssh.hosts.iter().all(|h| !h.enabled),
        "shipped host entries are disabled"
    );
}

// ------------------------------------------------------------------- consent

#[test]
fn visibility_effect_only_when_ssh_is_configured() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    eff(&mut a);
    press(&mut a, "6");
    assert!(eff(&mut a).contains(&Effect::SshVisible(true)));
    press(&mut a, "1");
    assert!(eff(&mut a).contains(&Effect::SshVisible(false)));
    let mut none = app(vec![]);
    eff(&mut none);
    press(&mut none, "6");
    assert!(!eff(&mut none)
        .iter()
        .any(|e| matches!(e, Effect::SshVisible(_))));
}

#[test]
fn consent_requires_focus_then_the_exact_host_name() {
    let mut a = app(vec![host("home-lab", "10.0.0.5")]);
    press(&mut a, "6");
    eff(&mut a);
    press(&mut a, "C"); // table not focused
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("Tab"));
    a.handle_key(k(KeyCode::Tab));
    press(&mut a, "C");
    assert!(matches!(a.mode, Mode::SshConsent(_)));
    let s = draw(&mut a, 140, 40);
    for want in [
        "Consent: remote read-only check",
        "home-lab",
        "viko@10.0.0.5",
        "/proc/loadavg",
        "FIXED read-only script",
        "credentials",
        "password",
        "StrictHostKeyChecking=yes",
        "forwarding",
        "until you quit",
        "never saved",
    ] {
        assert!(s.contains(want), "dialog should mention `{want}`:\n{s}");
    }
    for key in ['y', 'Y', ' ', 'h'] {
        a.handle_key(k(KeyCode::Char(key)));
    }
    a.handle_key(k(KeyCode::Enter));
    assert!(eff(&mut a).is_empty(), "wrong text consents to nothing");
    assert!(matches!(a.mode, Mode::SshConsent(_)));
    for _ in 0..4 {
        a.handle_key(k(KeyCode::Backspace));
    }
    press(&mut a, "home-lab");
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.mode, Mode::Normal);
    assert_eq!(eff(&mut a), vec![Effect::SshConsent("home-lab".into())]);
}

#[test]
fn esc_cancels_without_any_effect() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    to_services_ssh(&mut a);
    eff(&mut a);
    press(&mut a, "C");
    press(&mut a, "lab");
    a.handle_key(k(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
    assert!(eff(&mut a).is_empty());
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .contains("no connection was made"));
}

#[test]
fn already_consented_and_disabled_hosts_do_not_reopen_the_dialog() {
    let mut off = host("off", "10.0.0.9");
    off.enabled = false;
    let mut a = app(vec![host("lab", "10.0.0.5"), off]);
    to_services_ssh(&mut a);
    let mut st = state_for(&a, 0);
    st.status = SshStatus::Ok;
    a.ingest_ssh(snap_of(vec![st, state_for(&a, 1)]));
    press(&mut a, "C");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("already consented"));
    a.handle_key(k(KeyCode::Down));
    press(&mut a, "C");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .contains("disabled in the config"));
}

#[test]
fn consent_without_ssh_enabled_explains_how_to_enable() {
    let mut a = app(vec![]);
    press(&mut a, "6");
    press(&mut a, "C");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .contains("[modules] ssh = true"));
}

#[test]
fn r_on_services_requests_an_ssh_recheck() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    press(&mut a, "6");
    eff(&mut a);
    press(&mut a, "R");
    assert!(eff(&mut a).contains(&Effect::SshCheckNow));
}

#[test]
fn arrow_keys_move_within_the_focused_table_only() {
    let mut a = app(vec![
        host("a", "10.0.0.1"),
        host("b", "10.0.0.2"),
        host("c", "10.0.0.3"),
    ]);
    to_services_ssh(&mut a);
    a.ingest_ssh(snap_of((0..3).map(|i| state_for(&a, i)).collect()));
    press(&mut a, "jjjj");
    assert_eq!(a.ssh_table.selected(), Some(2), "bounded");
    press(&mut a, "gk");
    assert_eq!(a.ssh_table.selected(), Some(0));
    assert_eq!(
        a.services_table.selected().unwrap_or(0),
        0,
        "the checks table was not moved"
    );
}

// ----------------------------------------------------------------- rendering

fn show_state(a: &mut App, mutate: impl FnOnce(&mut HostState)) -> String {
    let mut st = state_for(a, 0);
    mutate(&mut st);
    a.ingest_ssh(snap_of(vec![st]));
    draw(a, 160, 44)
}

#[test]
fn every_host_state_is_labelled_plainly() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    press(&mut a, "6");
    let s = draw(&mut a, 160, 44);
    assert!(
        s.contains("Remote hosts (SSH") && s.contains("starting"),
        "{s}"
    );
    let s = show_state(&mut a, |_| {});
    assert!(
        s.contains("needs consent") && s.contains("Tab, then C"),
        "{s}"
    );
    assert!(s.contains("Nothing is contacted until you consent"));
    let s = show_state(&mut a, |h| h.status = SshStatus::Checking);
    assert!(s.contains("checking"));
    let s = show_state(&mut a, |h| {
        h.apply(
            Ok(systui::ssh::parse_output(OK_OUT, &["docker".into(), "nginx".into()]).unwrap()),
            SystemTime::now(),
        )
    });
    for want in [
        "ok",
        "0.52 0.58 0.59",
        "75% of 15.3 GiB",
        "55.9 GiB free (40%)",
        "1d 3h",
        "docker:active",
        "nginx:inactive",
        "viko@10.0.0.5",
    ] {
        assert!(s.contains(want), "missing `{want}`:\n{s}");
    }
    let s = show_state(&mut a, |h| {
        h.apply(
            Err(Failure::Unreachable("connection refused".into())),
            SystemTime::now(),
        )
    });
    assert!(
        s.contains("unreachable") && s.contains("connection refused"),
        "{s}"
    );
    let s = show_state(&mut a, |h| {
        h.apply(
            Err(Failure::Error("host key not trusted".into())),
            SystemTime::now(),
        )
    });
    assert!(
        s.contains("error") && s.contains("host key not trusted"),
        "{s}"
    );
}

#[test]
fn stale_remote_data_is_flagged_and_failures_keep_old_values_visible() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    press(&mut a, "6");
    let old = SystemTime::now() - Duration::from_secs(1000);
    let s = show_state(&mut a, |h| {
        h.apply(Ok(systui::ssh::parse_output(OK_OUT, &[]).unwrap()), old)
    });
    assert!(s.contains("STALE"), "{s}");
    let s = show_state(&mut a, |h| {
        h.apply(
            Ok(systui::ssh::parse_output(OK_OUT, &[]).unwrap()),
            SystemTime::now(),
        );
        h.apply(
            Err(Failure::Unreachable("timed out".into())),
            SystemTime::now(),
        );
    });
    assert!(
        s.contains("unreachable") && s.contains("0.52"),
        "old load still shown next to the error:\n{s}"
    );
}

#[test]
fn missing_remote_fields_render_as_dashes_not_zeros() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    press(&mut a, "6");
    let s = show_state(&mut a, |h| {
        h.apply(
            Ok(RemoteStatus {
                load: Some((1.0, 2.0, 3.0)),
                ..Default::default()
            }),
            SystemTime::now(),
        )
    });
    let row = s
        .lines()
        .find(|l| l.contains("lab") && l.contains("1.00 2.00 3.00"))
        .expect(&s);
    assert!(row.contains('—'), "{row}");
    assert!(!row.contains("0 B"), "{row}");
}

#[test]
fn hostile_remote_text_cannot_reach_the_terminal() {
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    press(&mut a, "6");
    let s = show_state(&mut a, |h| {
        h.apply(
            Err(Failure::Error(systui::util::text::sanitize(
                "\u{1b}[2Jowned",
            ))),
            SystemTime::now(),
        )
    });
    assert!(!s.contains('\u{1b}'));
}

#[test]
fn services_tab_with_ssh_never_panics_at_any_size() {
    let mut a = app(vec![host("a", "10.0.0.1"), host("b", "10.0.0.2")]);
    to_services_ssh(&mut a);
    a.ingest_ssh(snap_of((0..2).map(|i| state_for(&a, i)).collect()));
    for (w, h) in [
        (160, 44),
        (120, 40),
        (90, 26),
        (70, 20),
        (60, 18),
        (0, 0),
        (1, 1),
        (200, 8),
    ] {
        let _ = draw(&mut a, w, h);
    }
    press(&mut a, "C");
    for (w, h) in [(160, 44), (60, 18), (30, 8), (0, 0)] {
        let _ = draw(&mut a, w, h);
    }
}

// ---------------------------------------------------- app + worker end to end

#[test]
fn app_worker_pipeline_with_a_fake_ssh() {
    let calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let c2 = calls.clone();
    let runner: systui::modules::ssh::Runner = Arc::new(move |args, _t| {
        c2.lock().unwrap().push(args.to_vec());
        Ok(Output {
            stdout: OK_OUT.as_bytes().to_vec(),
            stderr: vec![],
            truncated: false,
        })
    });
    let mut a = app(vec![host("lab", "10.0.0.5")]);
    let h = SshHandle::spawn(a.ssh_specs.clone(), runner);
    to_services_ssh(&mut a);
    h.send(SshCmd::Visible(true));
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        calls.lock().unwrap().is_empty(),
        "nothing was contacted before consent"
    );

    press(&mut a, "C");
    press(&mut a, "lab");
    a.handle_key(k(KeyCode::Enter));
    for e in eff(&mut a) {
        if let Effect::SshConsent(n) = e {
            h.send(SshCmd::Consent(n));
        }
    }
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if let Some(s) = h.take_latest() {
            a.ingest_ssh(s);
            if a.ssh
                .as_ref()
                .is_some_and(|s| s.hosts[0].status == SshStatus::Ok)
            {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let s = draw(&mut a, 160, 44);
    assert!(s.contains("0.52 0.58 0.59") && s.contains("ok"), "{s}");
    let seen = calls.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(
        seen[0].contains(&"BatchMode=yes".to_string()) && seen[0].contains(&"10.0.0.5".to_string())
    );
}

#[test]
fn real_ssh_to_a_closed_port_through_the_whole_stack() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut hc = host("closed", "127.0.0.1");
    hc.port = Some(port);
    let mut a = app(vec![hc]);
    let h = SshHandle::spawn(a.ssh_specs.clone(), Arc::new(systui::ssh::real_runner));
    to_services_ssh(&mut a);
    h.send(SshCmd::Visible(true));
    h.send(SshCmd::Consent("closed".into()));
    let t = Instant::now();
    let end = t + Duration::from_secs(20);
    let mut status = None;
    while Instant::now() < end {
        if let Some(s) = h.take_latest() {
            let st = s.hosts[0].status.clone();
            a.ingest_ssh(s);
            if matches!(st, SshStatus::Unreachable(_) | SshStatus::Error(_)) {
                status = Some(st);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    match status.expect("a result within 20 s") {
        SshStatus::Unreachable(_) => {}
        SshStatus::Error(m) if m.contains("not found") => eprintln!("no ssh client: skipped"),
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    let s = draw(&mut a, 160, 44);
    assert!(s.contains("unreachable") || s.contains("not found"), "{s}");
}

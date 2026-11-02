mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use systui::actions::docker::DockerAction;
use systui::app::{App, DevPane, Effect, Mode, TaskResult};
use systui::config::Config;
use systui::modules::docker::{Container, DockerData, LogPreview};
use systui::modules::{ModStatus, ModuleKind, ModuleSnapshot, Updates};
use systui::ui::{self, theme::Theme, RenderOpts};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn press(a: &mut App, s: &str) {
    for ch in s.chars() {
        a.handle_key(k(KeyCode::Char(ch)));
    }
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
fn eff(a: &mut App) -> Vec<Effect> {
    a.take_effects()
        .into_iter()
        .filter(|e| !matches!(e, Effect::Needs(_)))
        .collect()
}

const ID1: &str = "1a2b3c4d5e6f";
const ID2: &str = "2b3c4d5e6f70";
const ID3: &str = "0badc0ffee00";

fn container(id: &str, name: &str, project: &str, state: &str) -> Container {
    Container {
        id: id.into(),
        name: name.into(),
        image: "img:dev".into(),
        project: project.into(),
        state: state.into(),
        status: "Up".into(),
        cpu_percent: (state == "running").then_some(1.0),
        mem_usage: (state == "running").then(|| "10MiB / 1GiB".into()),
        mem_percent: None,
    }
}

fn docker_app(allow_actions: bool) -> App {
    let mut cfg = Config::default();
    cfg.modules.docker = true;
    cfg.docker.allow_actions = allow_actions;
    let mut a = App::new(cfg);
    a.ingest(Arc::new(snapshot(10)), Instant::now());
    press(&mut a, "5");
    a.handle_key(k(KeyCode::Tab)); // Docker pane
    assert_eq!(a.dev.pane, DevPane::Docker);
    let data = DockerData {
        version: Some("28".into()),
        // already in the order the poller produces: grouped by project, then name
        containers: vec![
            container(ID3, "batch-job", "", "exited"),
            container(ID1, "shop-web-1", "shop", "running"),
            container(ID2, "shop-gateway-1", "shop", "running"),
        ],
        df: None,
        df_at: None,
        warning: None,
        skipped_lines: 0,
    };
    a.ingest_modules(
        Updates {
            docker: Some(Arc::new(ModuleSnapshot {
                seq: 1,
                status: ModStatus::Ok,
                data: Some(data),
                observed_at: SystemTime::now(),
            })),
            ..Default::default()
        },
        Instant::now(),
    );
    a.dev.docker_table.select(Some(1)); // shop-web-1
    eff(&mut a);
    a
}

fn preview(n: usize) -> LogPreview {
    LogPreview {
        lines: (0..n)
            .map(|i| format!("2026-11-02T10:00:{:02}.000000000Z line number {i}", i % 60))
            .collect(),
        truncated: false,
    }
}

// ---------------------------------------------------------------------- logs

#[test]
fn l_opens_a_loading_overlay_and_requests_the_logs_of_the_selected_container() {
    let mut a = docker_app(false);
    press(&mut a, "l");
    let Mode::DockerLogs(v) = &a.mode else {
        panic!("{:?}", a.mode)
    };
    assert!(v.loading && v.id == ID1 && v.name == "shop-web-1");
    assert_eq!(
        eff(&mut a),
        vec![Effect::DockerLogs {
            id: ID1.into(),
            lines: 100
        }]
    );
    assert!(draw(&mut a, 120, 40).contains("loading"));
}

#[test]
fn logs_are_shown_newest_last_scrollable_and_labelled_as_possibly_secret() {
    let mut a = docker_app(false);
    press(&mut a, "l");
    eff(&mut a);
    a.ingest_task(TaskResult::Logs(ID1.into(), Ok(preview(80))));
    let s = draw(&mut a, 120, 40);
    assert!(s.contains("Logs: shop-web-1"), "{s}");
    assert!(
        s.contains("line number 79") && !s.contains("line number 0 "),
        "tail view:\n{s}"
    );
    assert!(
        s.contains("80 line(s)")
            && s.contains("can contain secrets")
            && s.contains("never saved or exported")
    );
    press(&mut a, "g");
    let s = draw(&mut a, 120, 40);
    assert!(
        s.contains("line number 0") || s.contains("line number 1"),
        "scrolled to the top:\n{s}"
    );
    a.handle_key(k(KeyCode::End));
    for _ in 0..200 {
        a.handle_key(k(KeyCode::Down)); // bounded
        a.handle_key(k(KeyCode::PageDown));
    }
    for _ in 0..200 {
        a.handle_key(k(KeyCode::Up));
        a.handle_key(k(KeyCode::PageUp));
    }
    let _ = draw(&mut a, 120, 40);
    a.handle_key(k(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn log_errors_empty_output_and_stale_results_are_handled() {
    let mut a = docker_app(false);
    press(&mut a, "l");
    eff(&mut a); // the initial request
    a.ingest_task(TaskResult::Logs("ffffffffffff".into(), Ok(preview(5)))); // some other container
    assert!(
        matches!(&a.mode, Mode::DockerLogs(v) if v.loading && v.lines.is_empty()),
        "ignored"
    );
    a.ingest_task(TaskResult::Logs(
        ID1.into(),
        Err("engine not reachable".into()),
    ));
    assert!(draw(&mut a, 120, 40).contains("error: engine not reachable"));
    press(&mut a, "r"); // reload
    assert_eq!(eff(&mut a).len(), 1);
    a.ingest_task(TaskResult::Logs(
        ID1.into(),
        Ok(LogPreview {
            lines: vec![],
            truncated: false,
        }),
    ));
    assert!(draw(&mut a, 120, 40).contains("(no log output)"));
    // results arriving after the overlay closed are ignored without panicking
    a.handle_key(k(KeyCode::Esc));
    a.ingest_task(TaskResult::Logs(ID1.into(), Ok(preview(3))));
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn log_overlay_survives_tiny_terminals_and_hostile_text() {
    let mut a = docker_app(false);
    press(&mut a, "l");
    let hostile = LogPreview {
        lines: vec!["t \u{1b}[2J pwned".into()],
        truncated: true,
    };
    a.ingest_task(TaskResult::Logs(ID1.into(), Ok(hostile)));
    for (w, h) in [(120, 40), (60, 18), (20, 6), (0, 0), (1, 1)] {
        let s = draw(&mut a, w, h);
        assert!(!s.contains('\u{1b}'));
    }
}

#[test]
fn logs_need_docker_enabled_and_a_selection() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot(5)), Instant::now());
    press(&mut a, "5");
    a.handle_key(k(KeyCode::Tab));
    press(&mut a, "l");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("disabled"));
    let mut b = docker_app(false);
    b.dev.docker_table.select(None);
    press(&mut b, "l");
    assert_eq!(b.mode, Mode::Normal);
    assert!(b
        .notice
        .as_deref()
        .unwrap()
        .contains("no container selected"));
}

#[test]
fn l_is_ignored_outside_the_docker_panel() {
    let mut a = docker_app(false);
    a.handle_key(k(KeyCode::Tab)); // WSL pane
    press(&mut a, "l");
    assert_eq!(a.mode, Mode::Normal);
}

// ------------------------------------------------------------------- actions

#[test]
fn actions_are_off_by_default_with_an_explanation() {
    let mut a = docker_app(false);
    press(&mut a, "S");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("allow_actions"));
    press(&mut a, "X");
    assert_eq!(a.mode, Mode::Normal);
    assert!(eff(&mut a).is_empty());
}

#[test]
fn stop_requires_typing_the_exact_container_name() {
    let mut a = docker_app(true);
    press(&mut a, "S");
    let Mode::DockerConfirm(d) = &a.mode else {
        panic!("{:?}", a.mode)
    };
    assert_eq!(
        (d.id.as_str(), d.name.as_str(), d.action),
        (ID1, "shop-web-1", DockerAction::Stop)
    );
    let s = draw(&mut a, 120, 40);
    assert!(
        s.contains("STOP container?")
            && s.contains("shop-web-1")
            && s.contains(ID1)
            && s.contains("re-checked"),
        "{s}"
    );
    for key in ['y', 'Y', ' ', 'S', 'x'] {
        a.handle_key(k(KeyCode::Char(key)));
    }
    a.handle_key(k(KeyCode::Enter));
    assert!(eff(&mut a).is_empty(), "no single key or wrong text acts");
    press(&mut a, "shop-web-"); // a prefix is not enough
    a.handle_key(k(KeyCode::Enter));
    assert!(eff(&mut a).is_empty());
    assert!(matches!(a.mode, Mode::DockerConfirm(_)));
}

#[test]
fn exact_name_plus_enter_emits_one_action_for_the_captured_container() {
    let mut a = docker_app(true);
    press(&mut a, "X");
    // fresh data re-sorts the table and moves the selection while the dialog is open
    a.dev.docker_table.select(Some(2));
    // clear what was typed by mistake first: a stray char then backspace
    a.handle_key(k(KeyCode::Char('z')));
    a.handle_key(k(KeyCode::Backspace));
    press(&mut a, "shop-web-1");
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.mode, Mode::Normal);
    assert_eq!(
        eff(&mut a),
        vec![Effect::DockerAction {
            action: DockerAction::Restart,
            id: ID1.into(),
            name: "shop-web-1".into()
        }],
        "acts on the container shown in the dialog, not the current selection"
    );
}

#[test]
fn esc_cancels_and_stopping_a_stopped_container_is_refused_early() {
    let mut a = docker_app(true);
    press(&mut a, "S");
    a.handle_key(k(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("cancelled"));
    assert!(eff(&mut a).is_empty());
    a.dev.docker_table.select(Some(0)); // batch-job: exited
    press(&mut a, "S");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.notice.as_deref().unwrap().contains("not running"));
    press(&mut a, "X"); // restart of an exited container is fine
    assert!(matches!(a.mode, Mode::DockerConfirm(_)));
}

#[test]
fn action_results_are_reported_and_trigger_a_docker_refresh() {
    let mut a = docker_app(true);
    a.ingest_task(TaskResult::Action(Ok("stopped shop-web-1".into())));
    assert_eq!(a.notice.as_deref(), Some("Docker: stopped shop-web-1"));
    assert_eq!(
        eff(&mut a),
        vec![Effect::ModuleRefresh(ModuleKind::Docker, false)]
    );
    a.ingest_task(TaskResult::Action(Err("container changed".into())));
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .starts_with("Docker action failed: container changed"));
}

#[test]
fn compose_projects_are_shown_as_a_column() {
    let mut a = docker_app(false);
    let s = draw(&mut a, 140, 44);
    assert!(s.contains("PROJECT") && s.contains("shop"), "{s}");
    let job = s.lines().find(|l| l.contains("batch-job")).unwrap();
    assert!(
        job.contains("exited") && !job.contains("shop"),
        "no project => blank: {job}"
    );
}

#[test]
fn docker_config_bounds() {
    let p = |s: &str| Config::from_toml(s, Path::new("t.toml"));
    assert!(p("[docker]\nlog_lines = 5\n").is_err());
    assert!(p("[docker]\nlog_lines = 5000\n").is_err());
    let c = p("[docker]\nlog_lines = 250\nallow_actions = true\n").unwrap();
    assert_eq!((c.docker.log_lines, c.docker.allow_actions), (250, true));
    let d = Config::default();
    assert_eq!(
        (d.docker.log_lines, d.docker.allow_actions),
        (100, false),
        "actions are off by default"
    );
}

// ----------------------------------------------------------------- real Docker

fn docker_ok() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn docker(args: &[&str]) -> (bool, String) {
    match std::process::Command::new("docker").args(args).output() {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

#[test]
fn real_logs_of_an_existing_container_are_read_only_and_clean() {
    if !docker_ok() {
        eprintln!("docker not available: skipped");
        return;
    }
    let (_, ids) = docker(&["ps", "-q", "--no-trunc"]);
    let Some(id) = ids.lines().next() else {
        eprintln!("no running container: skipped");
        return;
    };
    let p = systui::modules::docker::logs(id, 20).expect("logs");
    assert!(p.lines.len() <= 20);
    assert!(
        p.lines.iter().all(|l| !l.chars().any(char::is_control)),
        "no control characters survive"
    );
    eprintln!("real logs: {} line(s) from {}", p.lines.len(), &id[..12]);
}

/// Finds a local image that has `sleep`, to build a throw-away container from.
fn image_with_sleep() -> Option<String> {
    let (_, imgs) = docker(&["images", "--format", "{{.Repository}}:{{.Tag}}"]);
    imgs.lines()
        .filter(|i| !i.contains("<none>"))
        .take(15)
        .find(|img| {
            docker(&[
                "run",
                "--rm",
                "--network",
                "none",
                "--entrypoint",
                "sleep",
                img,
                "0",
            ])
            .0
        })
        .map(String::from)
}

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

#[test]
fn real_stop_and_restart_of_a_disposable_container() {
    if !docker_ok() {
        return;
    }
    let Some(img) = image_with_sleep() else {
        eprintln!("no local image with `sleep`: skipped");
        return;
    };
    let name = format!("systui-m6-test-{}", std::process::id());
    let (ok, id) = docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--network",
        "none",
        "--entrypoint",
        "sleep",
        &img,
        "600",
    ]);
    assert!(ok, "could not create the disposable container: {id}");
    let _cleanup = Cleanup(name.clone());
    let short = &id[..12];
    let running = |want: &str| docker(&["inspect", "-f", "{{.State.Running}}", &name]).1 == want;
    assert!(running("true"));

    // a wrong name must not touch it
    assert!(systui::actions::docker::run(DockerAction::Stop, short, "someone-else").is_err());
    assert!(running("true"));

    let msg = systui::actions::docker::run(DockerAction::Stop, short, &name).expect("stop");
    assert_eq!(msg, format!("stopped {name}"));
    assert!(running("false"), "container is actually stopped");

    let e = systui::actions::docker::run(DockerAction::Stop, short, &name).unwrap_err();
    assert!(e.contains("not running"), "stopping twice is refused: {e}");

    let msg = systui::actions::docker::run(DockerAction::Restart, short, &name).expect("restart");
    assert_eq!(msg, format!("restarted {name}"));
    assert!(running("true"), "container is running again");
    // _cleanup removes it
    let _ = Duration::from_secs(0);
}

// Safety rule for this file: real Docker tests may READ existing containers (logs) but may only
// ever stop/restart containers they created themselves (`systui-m6-test-*`). Never add a test
// that picks "the first running container" as the target of a mutating action.

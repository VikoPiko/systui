mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use systui::app::App;
use systui::config::Config;
use systui::recorder::list_recordings;
use systui::ui::{self, theme::Theme, RenderOpts};

fn parse(s: &str) -> Result<Config, systui::config::ConfigError> {
    Config::from_toml(s, Path::new("t.toml"))
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("systui-rt-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}
fn cfg_in(dir: &Path) -> Config {
    let mut c = Config::default();
    c.recording.enabled = true;
    c.recording.directory = Some(dir.to_string_lossy().to_string());
    c
}
fn draw(a: &mut App) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| ui::render(f, a, &opts)).unwrap();
    let b = t.backend().buffer().clone();
    (0..40u16)
        .map(|y| {
            (0..120u16)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn lines_of(dir: &Path) -> Vec<serde_json::Value> {
    let mut files = list_recordings(dir);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
        .iter()
        .flat_map(|f| {
            std::fs::read_to_string(&f.path)
                .unwrap()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .map(|l| serde_json::from_str(&l).expect("every line is valid JSON"))
        .collect()
}

#[test]
fn defaults_are_off_and_bounded() {
    let c = Config::default();
    assert!(!c.recording.enabled);
    assert_eq!(
        (
            c.recording.interval_seconds,
            c.recording.max_files,
            c.recording.max_total_mb
        ),
        (5, 10, 100)
    );
    assert_eq!(
        (
            c.recording.max_file_mb,
            c.recording.max_age_days,
            c.recording.top_processes
        ),
        (10, 7, 10)
    );
    assert!(c.recording.resolved_dir().unwrap().ends_with("recordings"));
    let l = c.recording.limits();
    assert_eq!(l.max_file_bytes, 10 << 20);
    assert_eq!(l.max_age, Duration::from_secs(7 * 86_400));
}

#[test]
fn config_bounds_and_directory_rules() {
    for bad in [
        "interval_seconds = 0",
        "interval_seconds = 99999",
        "top_processes = 500",
        "max_file_mb = 0",
        "max_file_mb = 5000",
        "max_files = 0",
        "max_total_mb = 0",
        "max_age_days = 0",
        "directory = \"relative/dir\"",
        "directory = \"/tmp/../etc\"",
    ] {
        assert!(parse(&format!("[recording]\n{bad}\n")).is_err(), "{bad}");
    }
    let abs = if cfg!(windows) {
        "C:\\\\logs\\\\systui"
    } else {
        "/var/log/systui"
    };
    assert!(parse(&format!(
        "[recording]\nenabled = true\ndirectory = \"{abs}\"\n"
    ))
    .is_ok());
}

#[test]
fn ctrl_r_does_nothing_but_explain_when_not_enabled() {
    let dir = tmpdir("off");
    let mut cfg = Config::default();
    cfg.recording.directory = Some(dir.to_string_lossy().to_string()); // directory set, but not enabled
    let mut a = App::new(cfg);
    a.ingest(Arc::new(snapshot(5)), Instant::now());
    a.handle_key(ctrl('r'));
    assert!(!a.is_recording());
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .contains("[recording] enabled = true"));
    assert!(!dir.exists(), "no directory or file is created");
}

#[test]
fn nothing_is_recorded_until_the_user_starts_a_session() {
    let dir = tmpdir("idle");
    let mut a = App::new(cfg_in(&dir));
    let t0 = Instant::now();
    for i in 0..30 {
        a.ingest(
            Arc::new(snapshot_with(i, many_procs(5))),
            t0 + Duration::from_secs(i),
        );
    }
    assert!(!a.is_recording());
    assert!(!dir.exists(), "enabled=true is permission, not a recorder");
}

#[test]
fn session_records_at_the_interval_with_sanitized_documents_and_stops_cleanly() {
    let dir = tmpdir("run");
    let mut a = App::new(cfg_in(&dir));
    let t0 = Instant::now();
    a.ingest(Arc::new(snapshot_with(0, many_procs(30))), t0);
    a.handle_key(ctrl('r'));
    assert!(a.is_recording());
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .starts_with("recording every 5 s to "));
    assert!(draw(&mut a).contains("REC"), "header badge");
    for i in 1..=20u64 {
        a.ingest(
            Arc::new(snapshot_with(i, many_procs(30))),
            t0 + Duration::from_secs(i),
        );
    }
    a.handle_key(ctrl('r'));
    assert!(!a.is_recording());
    assert!(
        a.notice
            .as_deref()
            .unwrap()
            .starts_with("recording stopped: 5 line(s)"),
        "{:?}",
        a.notice
    );
    assert!(!draw(&mut a).contains(" REC "));

    let docs = lines_of(&dir);
    assert_eq!(docs.len(), 5, "t = 0, 5, 10, 15, 20 s at a 5 s interval");
    for d in &docs {
        assert_eq!(d["schema_version"], 1);
        assert_eq!(d["top_processes"].as_array().unwrap().len(), 10);
        assert_eq!(d["processes_fresh"], true);
        let text = d.to_string();
        assert!(
            !text.contains("--flag") && !text.contains("testhost"),
            "no cmdline / hostname: {text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recording_continues_while_the_display_is_paused() {
    let dir = tmpdir("paused");
    let mut a = App::new(cfg_in(&dir));
    let t0 = Instant::now();
    a.ingest(Arc::new(snapshot_with(0, many_procs(5))), t0);
    a.handle_key(ctrl('r'));
    a.handle_key(k(KeyCode::Char(' ')));
    for i in 1..=10u64 {
        a.ingest(
            Arc::new(snapshot_with(i, many_procs(5))),
            t0 + Duration::from_secs(i),
        );
    }
    a.handle_key(ctrl('r'));
    assert_eq!(lines_of(&dir).len(), 3, "t = 0, 5, 10");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lines_taken_while_processes_were_not_refreshed_withhold_them() {
    let dir = tmpdir("stale");
    let mut a = App::new(cfg_in(&dir));
    let t0 = Instant::now();
    a.ingest(Arc::new(snapshot_with(0, many_procs(5))), t0);
    a.handle_key(ctrl('r'));
    let mut s = snapshot_with(5, many_procs(5));
    s.processes_fresh = false;
    s.network_fresh = false;
    a.ingest(Arc::new(s), t0 + Duration::from_secs(5));
    a.handle_key(ctrl('r'));
    let docs = lines_of(&dir);
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[1]["processes_fresh"], false);
    assert!(docs[1]["process_count"].is_null(), "no stale count");
    assert_eq!(docs[1]["top_processes"].as_array().unwrap().len(), 0);
    assert!(
        docs[1]["network"][1]["rx_bytes_per_sec"].is_null(),
        "no stale rates"
    );
    assert!(
        docs[1]["memory"]["total_bytes"].is_number(),
        "cheap always-sampled data is still recorded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn start_failure_is_reported_and_leaves_no_recorder() {
    let f = std::env::temp_dir().join(format!("systui-rt-file-{}", std::process::id()));
    std::fs::write(&f, "not a directory").unwrap();
    let mut cfg = Config::default();
    cfg.recording.enabled = true;
    cfg.recording.directory = Some(f.join("sub").to_string_lossy().to_string());
    let mut a = App::new(cfg);
    a.ingest(Arc::new(snapshot(3)), Instant::now());
    a.handle_key(ctrl('r'));
    assert!(!a.is_recording());
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .contains("cannot start recording"));
    let _ = std::fs::remove_file(&f);
}

#[test]
fn starting_applies_retention_but_only_to_recording_files() {
    let dir = tmpdir("retention");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..8 {
        std::fs::write(dir.join(format!("systui-2020010{i}-000000.jsonl")), "{}\n").unwrap();
        std::thread::sleep(Duration::from_millis(15)); // distinct modified times
    }
    std::fs::write(dir.join("important.txt"), "do not touch").unwrap();
    let mut cfg = cfg_in(&dir);
    cfg.recording.max_files = 3;
    let mut a = App::new(cfg);
    a.ingest(Arc::new(snapshot(3)), Instant::now());
    a.handle_key(ctrl('r'));
    a.handle_key(ctrl('r'));
    assert_eq!(
        list_recordings(&dir).len(),
        3,
        "oldest removed down to the limit (active file included)"
    );
    assert!(dir.join("important.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palette_command_toggles_recording() {
    let dir = tmpdir("palette");
    let mut a = App::new(cfg_in(&dir));
    a.ingest(Arc::new(snapshot(3)), Instant::now());
    a.handle_key(ctrl('k'));
    for c in "record".chars() {
        a.handle_key(k(KeyCode::Char(c)));
    }
    a.handle_key(k(KeyCode::Enter));
    assert!(a.is_recording());
    a.handle_key(ctrl('r'));
    assert!(!a.is_recording());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_the_app_stops_the_writer_thread() {
    let dir = tmpdir("drop");
    let mut a = App::new(cfg_in(&dir));
    a.ingest(Arc::new(snapshot(3)), Instant::now());
    a.handle_key(ctrl('r'));
    let t = Instant::now();
    drop(a); // must join promptly
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(
        lines_of(&dir).len(),
        1,
        "the first line was flushed before shutdown"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_r_is_ignored_inside_dialogs() {
    let dir = tmpdir("modal");
    let mut a = App::new(cfg_in(&dir));
    a.ingest(Arc::new(snapshot(3)), Instant::now());
    a.handle_key(k(KeyCode::Char('?'))); // help open
    a.handle_key(ctrl('r'));
    assert!(
        !a.is_recording(),
        "no hidden side effects while a dialog is open"
    );
}

#[test]
fn real_collector_recording_is_valid_bounded_and_free_of_user_and_host_names() {
    use systui::collectors::Collector;
    let dir = tmpdir("real");
    let mut cfg = cfg_in(&dir);
    cfg.recording.interval_seconds = 1;
    let mut a = App::new(cfg);
    let c = Collector::spawn(Duration::from_millis(500));
    let end = Instant::now() + Duration::from_secs(5);
    let mut started = false;
    while Instant::now() < end {
        if let Some(s) = c.take_latest() {
            a.ingest(s, Instant::now());
            if !started {
                a.handle_key(ctrl('r'));
                started = true;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    a.handle_key(ctrl('r'));
    let docs = lines_of(&dir);
    assert!(docs.len() >= 3, "got {} lines", docs.len());
    assert!(docs
        .iter()
        .all(|d| d["memory"]["total_bytes"].as_u64().unwrap() > 0));
    assert!(
        docs.last().unwrap()["cpu"]["usage_percent"].is_number(),
        "CPU warmed up by the end"
    );
    let raw: String = list_recordings(&dir)
        .iter()
        .map(|f| std::fs::read_to_string(&f.path).unwrap())
        .collect();
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    for secret in [user, host] {
        if secret.len() >= 3 {
            assert!(
                !raw.to_lowercase().contains(&secret.to_lowercase()),
                "recording leaked `{secret}`"
            );
        }
    }
    let total: u64 = list_recordings(&dir).iter().map(|f| f.size).sum();
    assert!(
        total < 2 << 20,
        "a few seconds of recording stays small: {total} bytes"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

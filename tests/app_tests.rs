mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use systui::actions::process::{ActionError, ProcessActions};
use systui::app::{App, Effect, Mode, Tab};
use systui::config::Config;
use systui::domain::ProcessId;

type Calls = Rc<RefCell<Vec<(ProcessId, String, bool)>>>;

struct Mock {
    calls: Calls,
    result: Result<(), ActionError>,
    graceful: bool,
}

impl ProcessActions for Mock {
    fn can_graceful(&self) -> bool {
        self.graceful
    }
    fn terminate(&self, t: &ProcessId, name: &str, force: bool) -> Result<(), ActionError> {
        self.calls.borrow_mut().push((*t, name.to_string(), force));
        self.result.clone()
    }
}

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn press(app: &mut App, s: &str) {
    for ch in s.chars() {
        app.handle_key(k(KeyCode::Char(ch)));
    }
}

fn app_with(n: u32, result: Result<(), ActionError>, graceful: bool) -> (App, Calls) {
    let calls: Calls = Rc::default();
    let mut app = App::with_actions(
        Config::default(),
        Box::new(Mock {
            calls: calls.clone(),
            result,
            graceful,
        }),
    );
    app.ingest(Arc::new(snapshot(n)), Instant::now());
    app.select_tab(Tab::Processes);
    (app, calls)
}

fn selected_pid(app: &App) -> u32 {
    app.procs.selected.unwrap().pid
}

#[test]
fn navigation_keys_move_selection_and_stay_bounded() {
    let (mut app, _) = app_with(300, Ok(()), false);
    app.procs.page = 20;
    let first = selected_pid(&app);
    press(&mut app, "j");
    assert_ne!(selected_pid(&app), first);
    app.handle_key(k(KeyCode::PageDown));
    assert_eq!(app.procs.cursor, 21);
    app.handle_key(k(KeyCode::End));
    assert_eq!(app.procs.cursor, 299);
    app.handle_key(k(KeyCode::Down));
    assert_eq!(app.procs.cursor, 299);
    app.handle_key(k(KeyCode::Home));
    assert_eq!(app.procs.cursor, 0);
    app.handle_key(k(KeyCode::Up));
    assert_eq!(app.procs.cursor, 0);
}

#[test]
fn filter_mode_captures_text_and_blocks_shortcuts() {
    let (mut app, calls) = app_with(100, Ok(()), false);
    press(&mut app, "/");
    assert_eq!(app.mode, Mode::Filter);
    press(&mut app, "q"); // typed, must not quit
    assert!(!app.should_quit);
    press(&mut app, "K"); // typed, must not open the kill dialog
    assert!(!matches!(app.mode, Mode::Kill(_)));
    assert_eq!(app.procs.filter, "qK");
    assert!(calls.borrow().is_empty());
    app.handle_key(k(KeyCode::Backspace));
    app.handle_key(k(KeyCode::Backspace));
    assert!(app.procs.filter.is_empty());
    press(&mut app, "proc0007");
    assert_eq!(app.procs.rows.len(), 1);
    app.handle_key(k(KeyCode::Enter));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.procs.filter, "proc0007", "Enter keeps the filter");
    app.handle_key(k(KeyCode::Esc));
    assert!(app.procs.filter.is_empty(), "Esc in normal mode clears it");
    assert_eq!(app.procs.rows.len(), 100);
}

#[test]
fn esc_in_filter_mode_clears_and_exits() {
    let (mut app, _) = app_with(100, Ok(()), false);
    press(&mut app, "/zz");
    app.handle_key(k(KeyCode::Esc));
    assert_eq!(app.mode, Mode::Normal);
    assert!(app.procs.filter.is_empty());
    assert_eq!(app.procs.rows.len(), 100);
}

#[test]
fn detail_open_close_emits_watch_effects_and_reveal_toggles() {
    let (mut app, _) = app_with(50, Ok(()), false);
    app.take_effects();
    let id = app.procs.selected.unwrap();
    app.handle_key(k(KeyCode::Enter));
    assert!(app.procs.detail_open);
    assert_eq!(app.take_effects(), vec![Effect::Watch(Some(id))]);
    assert!(!app.procs.reveal_cmd);
    press(&mut app, "e");
    assert!(app.procs.reveal_cmd);
    app.handle_key(k(KeyCode::Esc));
    assert!(!app.procs.detail_open && !app.procs.reveal_cmd);
    assert_eq!(app.take_effects(), vec![Effect::Watch(None)]);
}

#[test]
fn parent_and_child_navigation() {
    let (mut app, _) = app_with(10, Ok(()), false);
    // sort by pid asc so row index == pid-1
    app.handle_key(k(KeyCode::Char('s')));
    app.handle_key(k(KeyCode::Char('s')));
    app.handle_key(k(KeyCode::Char('s')));
    app.procs
        .move_to(4, &app.snapshot.clone().unwrap().processes);
    assert_eq!(selected_pid(&app), 5);
    press(&mut app, "p");
    assert_eq!(
        selected_pid(&app),
        1,
        "parent of every test process is PID 1"
    );
    press(&mut app, "c");
    assert_eq!(selected_pid(&app), 2, "lowest-PID child");
    assert!(app.notice.as_deref().unwrap().contains("children"));
    // PID 1 has no parent
    press(&mut app, "p");
    press(&mut app, "p");
    assert!(app.notice.as_deref().unwrap().contains("no parent"));
}

// ---------------------------------------------------------------- terminate

fn open_kill(app: &mut App) -> ProcessId {
    let id = app.procs.selected.unwrap();
    press(app, "K");
    assert!(matches!(app.mode, Mode::Kill(_)), "dialog should open");
    id
}

#[test]
fn single_key_never_terminates() {
    let (mut app, calls) = app_with(50, Ok(()), false);
    open_kill(&mut app);
    for c in ['y', 'Y', 'k', 'K', 'x', ' '] {
        app.handle_key(k(KeyCode::Char(c)));
    }
    app.handle_key(k(KeyCode::Enter));
    assert!(calls.borrow().is_empty());
    assert!(
        matches!(app.mode, Mode::Kill(_)),
        "still waiting for the PID"
    );
}

#[test]
fn wrong_pid_does_not_terminate_and_esc_cancels() {
    let (mut app, calls) = app_with(50, Ok(()), false);
    let id = open_kill(&mut app);
    press(&mut app, &format!("{}9", id.pid));
    app.handle_key(k(KeyCode::Enter));
    assert!(calls.borrow().is_empty());
    app.handle_key(k(KeyCode::Esc));
    assert_eq!(app.mode, Mode::Normal);
    assert!(calls.borrow().is_empty());
}

#[test]
fn typed_pid_plus_enter_terminates_the_captured_target() {
    let (mut app, calls) = app_with(50, Ok(()), false);
    app.take_effects();
    let id = open_kill(&mut app);
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    let c = calls.borrow();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, id);
    assert!(c[0].2, "no graceful option => force");
    assert_eq!(app.mode, Mode::Normal);
    drop(c);
    assert_eq!(app.take_effects(), vec![Effect::RefreshNow]);
}

#[test]
fn dialog_target_does_not_follow_selection_changes() {
    let (mut app, calls) = app_with(50, Ok(()), false);
    let id = open_kill(&mut app);
    // A new snapshot reorders everything and the selection could move.
    let mut procs = many_procs(50);
    for p in procs.iter_mut() {
        p.cpu = p.cpu.map(|c| 100.0 - c);
    }
    app.ingest(Arc::new(snapshot_with(2, procs)), Instant::now());
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    assert_eq!(
        calls.borrow()[0].0,
        id,
        "still the process the dialog showed"
    );
}

#[test]
fn identity_mismatch_error_is_reported_and_clears_dialog() {
    let (mut app, calls) = app_with(50, Err(ActionError::IdentityMismatch(1)), false);
    let id = open_kill(&mut app);
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(app.mode, Mode::Normal);
    let n = app.notice.clone().unwrap();
    assert!(
        n.contains("Not terminated") && n.contains("different process"),
        "{n}"
    );
}

#[test]
fn graceful_first_then_distinct_force_confirmation() {
    let (mut app, calls) = app_with(50, Ok(()), true);
    let id = open_kill(&mut app);
    // `f` switches to force and discards what was typed
    press(&mut app, &id.pid.to_string());
    press(&mut app, "f");
    app.handle_key(k(KeyCode::Enter));
    assert!(
        calls.borrow().is_empty(),
        "force needs a fresh confirmation"
    );
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    assert_eq!(calls.borrow().len(), 1);
    assert!(calls.borrow()[0].2);
}

#[test]
fn graceful_request_is_not_forced() {
    let (mut app, calls) = app_with(50, Ok(()), true);
    let id = open_kill(&mut app);
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    assert!(!calls.borrow()[0].2);
}

#[test]
fn protected_and_self_processes_never_open_a_dialog() {
    let calls: Calls = Rc::default();
    let mut app = App::with_actions(
        Config::default(),
        Box::new(Mock {
            calls: calls.clone(),
            result: Ok(()),
            graceful: false,
        }),
    );
    let me = std::process::id();
    let procs = vec![
        proc(me, "systui", Some(1.0), 1),
        proc(900_001, "lsass.exe", Some(0.5), 1),
        proc(900_002, "svchost.exe", Some(0.1), 1),
    ];
    app.ingest(Arc::new(snapshot_with(1, procs)), Instant::now());
    app.select_tab(Tab::Processes);
    app.handle_key(k(KeyCode::Char('s'))); // mem
    app.handle_key(k(KeyCode::Char('s'))); // name
    app.handle_key(k(KeyCode::Char('s'))); // pid asc => me first
    for target in 0..3 {
        let snap = app.snapshot.clone().unwrap();
        app.procs.move_to(target, &snap.processes);
        press(&mut app, "K");
        let name = &snap.processes[app.procs.rows[target]].name;
        if name == "svchost.exe" {
            assert!(matches!(app.mode, Mode::Kill(_)), "caution still allowed");
            app.handle_key(k(KeyCode::Esc));
        } else {
            assert_eq!(app.mode, Mode::Normal, "{name} must be refused");
            assert!(app.notice.is_some());
        }
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn exit_is_confirmed_from_later_snapshots() {
    let (mut app, _) = app_with(50, Ok(()), false);
    let id = open_kill(&mut app);
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    let mut procs = many_procs(50);
    procs.retain(|p| p.id != id);
    app.ingest(Arc::new(snapshot_with(2, procs)), Instant::now());
    assert!(app.notice.as_deref().unwrap().contains("has exited"));
}

#[test]
fn survivor_is_reported_as_still_running() {
    let (mut app, _) = app_with(50, Ok(()), false);
    let id = open_kill(&mut app);
    press(&mut app, &id.pid.to_string());
    app.handle_key(k(KeyCode::Enter));
    app.ingest(Arc::new(snapshot_with(2, many_procs(50))), Instant::now());
    app.ingest(Arc::new(snapshot_with(3, many_procs(50))), Instant::now());
    assert!(app.notice.as_deref().unwrap().contains("still running"));
}

#[test]
fn confirm_terminate_false_uses_single_explicit_y() {
    let calls: Calls = Rc::default();
    let mut cfg = Config::default();
    cfg.processes.confirm_terminate = false;
    let mut app = App::with_actions(
        cfg,
        Box::new(Mock {
            calls: calls.clone(),
            result: Ok(()),
            graceful: false,
        }),
    );
    app.ingest(Arc::new(snapshot(20)), Instant::now());
    app.select_tab(Tab::Processes);
    press(&mut app, "K");
    press(&mut app, "x");
    assert!(calls.borrow().is_empty());
    press(&mut app, "y");
    assert_eq!(calls.borrow().len(), 1);
}

// ------------------------------------------------------------------- data

#[test]
fn pause_holds_display_and_resume_applies_latest_only() {
    let (mut app, _) = app_with(10, Ok(()), false);
    let shown = app.snapshot.clone().unwrap().seq;
    press(&mut app, " ");
    assert!(app.paused);
    app.ingest(Arc::new(snapshot_with(2, many_procs(10))), Instant::now());
    app.ingest(Arc::new(snapshot_with(3, many_procs(10))), Instant::now());
    assert_eq!(app.snapshot.clone().unwrap().seq, shown, "display frozen");
    press(&mut app, " ");
    assert_eq!(app.snapshot.clone().unwrap().seq, 3, "latest, not a replay");
}

#[test]
fn histories_are_bounded_and_skip_unavailable_values() {
    let mut cfg = Config::default();
    cfg.ui.history_samples = 30;
    let mut app = App::new(cfg);
    for i in 0..100 {
        let mut s = snapshot_with(i, many_procs(5));
        if i == 0 {
            s.cpu.usage = None; // warming up: must not record a fake 0
            s.network.iter_mut().for_each(|n| {
                n.rx_rate = None;
                n.tx_rate = None
            });
        }
        app.ingest(Arc::new(s), Instant::now());
    }
    assert_eq!(app.hist.cpu.len(), 30);
    assert_eq!(app.hist.net_rx.len(), 30);
    assert!(app.hist.cpu.iter().all(|v| v == 37.0));
}

#[test]
fn network_selection_falls_back_when_interface_disappears() {
    let mut app = App::new(Config::default());
    app.ingest(Arc::new(snapshot(5)), Instant::now());
    assert_eq!(
        app.net.selected.as_deref(),
        Some("eth0"),
        "prefers non-loopback"
    );
    let mut s = snapshot(5);
    s.network.retain(|i| i.name != "eth0");
    app.ingest(Arc::new(s), Instant::now());
    assert_eq!(app.net.selected.as_deref(), Some("lo"));
    assert!(!app.hist.per_iface.contains_key("eth0"), "history pruned");
    let mut s = snapshot(5);
    s.network.clear();
    app.ingest(Arc::new(s), Instant::now());
    assert_eq!(app.net.selected, None);
}

#[test]
fn stale_detection() {
    let mut app = App::new(Config::default());
    let t0 = Instant::now();
    assert!(!app.is_stale(t0), "no data yet is 'waiting', not stale");
    app.ingest(Arc::new(snapshot(3)), t0);
    assert!(!app.is_stale(t0 + std::time::Duration::from_secs(2)));
    assert!(app.is_stale(t0 + std::time::Duration::from_secs(10)));
}

#[test]
fn overview_top_list_switches_between_cpu_and_mem() {
    let mut app = App::new(Config::default());
    app.ingest(Arc::new(snapshot(100)), Instant::now());
    let snap = app.snapshot.clone().unwrap();
    let top_cpu = snap.processes[app.overview_top[0]].cpu;
    assert!(snap.processes.iter().all(|p| p.cpu <= top_cpu));
    assert_eq!(app.overview_top.len(), 10);
    press(&mut app, "s");
    let top_mem = snap.processes[app.overview_top[0]].rss;
    assert!(snap.processes.iter().all(|p| p.rss <= top_mem));
}

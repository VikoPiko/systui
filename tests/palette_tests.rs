mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::Instant;
use systui::app::{App, Mode, Tab};
use systui::config::Config;
use systui::ui::{self, theme::Theme, RenderOpts};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn press(app: &mut App, s: &str) {
    for ch in s.chars() {
        app.handle_key(k(KeyCode::Char(ch)));
    }
}
fn app() -> App {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot(20)), Instant::now());
    a
}
fn draw(w: u16, h: u16, app: &mut App) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(f, app, &opts)).unwrap();
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
fn ctrl_k_opens_and_closes_palette_and_esc_cancels() {
    let mut a = app();
    a.handle_key(ctrl('k'));
    assert!(matches!(a.mode, Mode::Palette(_)));
    a.handle_key(ctrl('k'));
    assert_eq!(a.mode, Mode::Normal);
    a.handle_key(ctrl('k'));
    a.handle_key(k(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn typing_is_text_until_enter_runs_best_match() {
    let mut a = app();
    a.handle_key(ctrl('k'));
    press(&mut a, "quit");
    assert!(!a.should_quit, "letters are text inside the palette");
    a.handle_key(k(KeyCode::Enter));
    assert!(a.should_quit);
}

#[test]
fn navigation_commands_and_shell_text_does_nothing() {
    let mut a = app();
    a.handle_key(ctrl('k'));
    press(&mut a, "go net");
    a.handle_key(k(KeyCode::Enter));
    assert_eq!(a.tab, Tab::Network);

    a.handle_key(ctrl('k'));
    press(&mut a, "calc.exe && del *");
    a.handle_key(k(KeyCode::Enter)); // nothing matches => nothing runs
    assert_eq!(a.tab, Tab::Network);
    assert!(!a.should_quit);
    a.handle_key(k(KeyCode::Esc));

    a.handle_key(k(KeyCode::Char('1')));
    a.handle_key(ctrl('k'));
    a.handle_key(k(KeyCode::Down));
    a.handle_key(k(KeyCode::Down));
    a.handle_key(k(KeyCode::Enter)); // 3rd item = Go to Network
    assert_eq!(a.tab, Tab::Network);
}

#[test]
fn terminate_via_palette_still_needs_pid_confirmation() {
    let mut a = app();
    a.handle_key(ctrl('k'));
    press(&mut a, "terminate");
    a.handle_key(k(KeyCode::Enter));
    assert!(matches!(a.mode, Mode::Kill(_)));
}

#[test]
fn unbound_ctrl_combos_do_not_act_like_plain_keys() {
    let mut a = app();
    a.select_tab(Tab::Processes);
    let before = a.procs.cursor;
    a.handle_key(ctrl('j'));
    a.handle_key(ctrl('q'));
    assert_eq!(a.procs.cursor, before);
    assert!(!a.should_quit);
}

#[test]
fn x_exports_sanitized_process_without_overwriting() {
    let mut a = app();
    a.select_tab(Tab::Processes);
    let dir = std::env::temp_dir().join(format!("systui-x-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    a.export_dir = dir.clone();
    press(&mut a, "x");
    assert!(
        a.notice.as_deref().unwrap().starts_with("exported"),
        "{:?}",
        a.notice
    );
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"pid\""));
    assert!(!text.contains("--flag"), "no command line: {text}");
    press(&mut a, "x"); // same pid + same sample second => must refuse
    let n = a.notice.clone().unwrap();
    assert!(n.contains("already exists"), "{n}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn palette_overlay_renders_filters_and_survives_tiny_sizes() {
    let mut a = app();
    a.handle_key(ctrl('k'));
    let s = draw(100, 30, &mut a);
    assert!(s.contains("Commands") && s.contains("Go to Overview") && s.contains("Quit"));
    press(&mut a, "xyzzy");
    assert!(draw(100, 30, &mut a).contains("no matching command"));
    for (w, h) in [(60, 18), (80, 20), (0, 0), (1, 1)] {
        let _ = draw(w, h, &mut a);
    }
}

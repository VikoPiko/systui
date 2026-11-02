use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use systui::app::{App, Focus, Mode, Tab};
use systui::config::Config;
use systui::ui::{self, layout::*, theme::Theme, RenderOpts};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn app() -> App {
    App::new(Config::default())
}

fn opts() -> RenderOpts {
    RenderOpts {
        theme: Theme::mono(),
        unicode: true,
    }
}

fn draw(w: u16, h: u16, app: &mut App) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(f, app, &opts())).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn number_keys_switch_available_tabs() {
    let mut a = app();
    for (c, tab) in [
        ('2', Tab::Processes),
        ('3', Tab::Network),
        ('4', Tab::Storage),
        ('1', Tab::Overview),
    ] {
        assert!(a.handle_key(key(KeyCode::Char(c))));
        assert_eq!(a.tab, tab);
    }
}

#[test]
fn all_six_tabs_are_reachable_by_number() {
    let mut a = app();
    for (c, tab) in [
        ('1', Tab::Overview),
        ('2', Tab::Processes),
        ('3', Tab::Network),
        ('4', Tab::Storage),
        ('5', Tab::Developer),
        ('6', Tab::Services),
    ] {
        a.handle_key(key(KeyCode::Char(c)));
        assert_eq!(a.tab, tab);
    }
    assert!(a.notice.is_none());
}

#[test]
fn tab_cycles_focus() {
    let mut a = app();
    a.handle_key(key(KeyCode::Tab));
    assert_eq!(a.focus, Focus::Secondary);
    a.handle_key(key(KeyCode::BackTab));
    assert_eq!(a.focus, Focus::Primary);
}

#[test]
fn help_opens_and_closes_one_layer() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('?')));
    assert_eq!(a.mode, Mode::Help);
    a.handle_key(key(KeyCode::Char('2'))); // swallowed while help is open
    assert_eq!(a.tab, Tab::Overview);
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
    assert!(!a.should_quit);
}

#[test]
fn quit_keys() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('q')));
    assert!(a.should_quit);
    let mut b = app();
    b.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(b.should_quit);
}

#[test]
fn quit_works_even_with_help_open() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('?')));
    a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(a.should_quit);
}

#[test]
fn space_toggles_pause() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char(' ')));
    assert!(a.paused);
    a.handle_key(key(KeyCode::Char(' ')));
    assert!(!a.paused);
}

#[test]
fn key_release_events_are_ignored() {
    let mut a = app();
    let release = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    };
    assert!(!a.handle_key(release));
    assert!(!a.should_quit);
}

#[test]
fn size_tiers() {
    use ratatui::layout::Rect;
    let r = |w, h| tier(Rect::new(0, 0, w, h));
    assert_eq!(r(120, 40), SizeTier::Wide);
    assert_eq!(r(110, 30), SizeTier::Wide);
    assert_eq!(r(100, 30), SizeTier::Standard);
    assert_eq!(r(80, 24), SizeTier::Standard);
    assert_eq!(r(79, 30), SizeTier::Compact);
    assert_eq!(r(100, 20), SizeTier::Compact);
    assert_eq!(r(59, 40), SizeTier::TooSmall);
    assert_eq!(r(100, 17), SizeTier::TooSmall);
}

#[test]
fn renders_tabs_and_footer_at_all_tiers() {
    let mut a = app();
    for (w, h) in [(120, 40), (90, 26), (70, 20)] {
        let s = draw(w, h, &mut a);
        assert!(s.contains("systui"), "{w}x{h}");
        assert!(s.contains("quit"), "{w}x{h}");
        assert!(s.contains("Overview"), "{w}x{h}");
    }
    assert!(draw(120, 40, &mut a).contains("Processes"));
}

#[test]
fn too_small_shows_message_and_never_panics() {
    let mut a = app();
    let s = draw(40, 10, &mut a);
    assert!(s.contains("too small"));
    // Pathological sizes must not panic.
    for (w, h) in [(0, 0), (1, 1), (5, 3), (60, 17), (59, 18), (300, 5)] {
        let mut a = app();
        a.mode = Mode::Help;
        let _ = draw(w, h, &mut a);
    }
}

#[test]
fn help_overlay_renders() {
    let mut a = app();
    a.handle_key(key(KeyCode::Char('?')));
    assert!(draw(100, 30, &mut a).contains("Help"));
}

#[test]
fn no_fabricated_readings_in_skeleton() {
    let s = draw(100, 30, &mut app());
    assert!(s.contains("no readings"));
}

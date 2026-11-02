mod common;
use common::*;
use ratatui::style::Color;
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::Instant;
use systui::app::{App, Tab};
use systui::config::{Config, ThemeName};
use systui::ui::{self, theme::Theme, RenderOpts};

/// Every (fg, bg) colour used anywhere on screen for a theme.
fn colors_used(theme: Theme, tab: Tab) -> Vec<(Color, Color)> {
    let mut app = App::new(Config::default());
    app.ingest(Arc::new(snapshot(60)), Instant::now());
    app.select_tab(tab);
    let opts = RenderOpts {
        theme,
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut v: Vec<(Color, Color)> = buf.content().iter().map(|c| (c.fg, c.bg)).collect();
    v.sort_by_key(|p| format!("{p:?}"));
    v.dedup();
    v
}

#[test]
fn no_color_and_mono_emit_no_colors_on_any_view() {
    for tab in [Tab::Overview, Tab::Processes, Tab::Network, Tab::Storage] {
        for theme in [
            Theme::resolve(ThemeName::Dark, true),
            Theme::resolve(ThemeName::Mono, false),
        ] {
            let used = colors_used(theme, tab);
            assert!(
                used.iter()
                    .all(|&(f, b)| f == Color::Reset && b == Color::Reset),
                "{tab:?}: {used:?}"
            );
        }
    }
}

#[test]
fn no_color_still_marks_selection_without_color() {
    // Selection must remain visible through attributes (reverse video), not hue.
    let mut app = App::new(Config::default());
    app.ingest(Arc::new(snapshot(20)), Instant::now());
    app.select_tab(Tab::Processes);
    let opts = RenderOpts {
        theme: Theme::resolve(ThemeName::Auto, true),
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| ui::render(f, &mut app, &opts)).unwrap();
    let buf = t.backend().buffer().clone();
    assert!(buf
        .content()
        .iter()
        .any(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED)));
}

#[test]
fn dark_and_light_use_color_and_differ() {
    let dark = colors_used(Theme::resolve(ThemeName::Dark, false), Tab::Overview);
    let light = colors_used(Theme::resolve(ThemeName::Light, false), Tab::Overview);
    assert!(dark.iter().any(|&(f, _)| f != Color::Reset));
    assert_ne!(dark, light);
    assert_eq!(
        colors_used(Theme::resolve(ThemeName::Auto, false), Tab::Overview),
        dark,
        "auto resolves to dark (documented)"
    );
}

#[test]
fn only_basic_ansi_colors_are_used() {
    for theme in [Theme::dark(), Theme::light()] {
        for (f, b) in colors_used(theme, Tab::Processes) {
            for c in [f, b] {
                assert!(
                    !matches!(c, Color::Rgb(..) | Color::Indexed(_)),
                    "needs truecolor/256: {c:?}"
                );
            }
        }
    }
}

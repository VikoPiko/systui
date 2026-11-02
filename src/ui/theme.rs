//! Color themes with graceful no-color / mono fallback.

use crate::config::ThemeName;
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub text: Style,
    pub dim: Style,
    pub border: Style,
    pub accent: Style,
    pub selected: Style,
    pub good: Style,
    pub warn: Style,
    pub bad: Style,
    pub graph: Style,
}

impl Theme {
    pub fn resolve(name: ThemeName, no_color: bool) -> Theme {
        if no_color || name == ThemeName::Mono {
            return Self::mono();
        }
        match name {
            ThemeName::Light => Self::light(),
            // `auto` cannot reliably query the terminal background; use dark.
            ThemeName::Dark | ThemeName::Auto | ThemeName::Mono => Self::dark(),
        }
    }

    pub fn mono() -> Theme {
        let plain = Style::default();
        Theme {
            text: plain,
            dim: plain.add_modifier(Modifier::DIM),
            border: plain,
            accent: plain.add_modifier(Modifier::BOLD),
            selected: plain.add_modifier(Modifier::REVERSED),
            good: plain,
            warn: plain.add_modifier(Modifier::BOLD),
            bad: plain.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            graph: plain,
        }
    }

    // Basic 16-color ANSI only, so it works on any terminal.
    pub fn dark() -> Theme {
        Theme {
            text: Style::default().fg(Color::Gray),
            dim: Style::default().fg(Color::DarkGray),
            border: Style::default().fg(Color::DarkGray),
            accent: Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
            selected: Style::default().fg(Color::Black).bg(Color::LightRed),
            good: Style::default().fg(Color::Green),
            warn: Style::default().fg(Color::Yellow),
            bad: Style::default().fg(Color::Red),
            graph: Style::default().fg(Color::Cyan),
        }
    }

    pub fn light() -> Theme {
        Theme {
            text: Style::default().fg(Color::Black),
            dim: Style::default().fg(Color::DarkGray),
            border: Style::default().fg(Color::DarkGray),
            accent: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            selected: Style::default().fg(Color::White).bg(Color::Red),
            good: Style::default().fg(Color::Green),
            warn: Style::default().fg(Color::Magenta),
            bad: Style::default().fg(Color::Red),
            graph: Style::default().fg(Color::Blue),
        }
    }
}

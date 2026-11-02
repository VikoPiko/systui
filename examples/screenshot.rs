//! Renders the real UI with REAL data from this machine into plain-text "screenshots"
//! (box drawing characters, no colors) for the README.
//!
//!   cargo run --release --example screenshot [-- <out-dir>]      (default: docs/screens)
//!
//! Privacy: the host name is replaced by `demo-host`; nothing else is altered. Process names,
//! interface names and volumes ARE the real ones from the machine running this, so review the
//! output before publishing it. The Dev tab (container names) and Services tab are not captured.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use systui::app::{App, Tab};
use systui::collectors::Collector;
use systui::config::Config;
use systui::ui::{self, theme::Theme, RenderOpts};

fn render(app: &mut App, w: u16, h: u16) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode: true,
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    t.draw(|f| ui::render(f, app, &opts)).expect("draw");
    let b = t.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| b[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn main() {
    let out: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "docs/screens".into())
        .into();
    std::fs::create_dir_all(&out).expect("create output dir");

    let collector = Collector::spawn(Duration::from_millis(500));
    let mut app = App::new(Config::default());
    let end = Instant::now() + Duration::from_secs(8);
    let mut got = 0;
    while got < 6 && Instant::now() < end {
        if let Some(s) = collector.take_latest() {
            let mut snap = (*s).clone();
            snap.host.hostname = Some("demo-host".into()); // the only thing we change
            app.ingest(std::sync::Arc::new(snap), Instant::now());
            got += 1;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(got >= 4, "collector produced only {got} samples");
    let key = |app: &mut App, c: KeyCode| {
        app.handle_key(KeyEvent::new(c, KeyModifiers::NONE));
    };

    // Feed a few more real samples (used so the selected process has a visible history).
    let pump = |app: &mut App, n: usize| {
        let end = Instant::now() + Duration::from_secs(6);
        let mut k = 0;
        while k < n && Instant::now() < end {
            if let Some(s) = collector.take_latest() {
                let mut snap = (*s).clone();
                snap.host.hostname = Some("demo-host".into());
                app.ingest(std::sync::Arc::new(snap), Instant::now());
                k += 1;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    let shots: Vec<(&str, String)> = {
        let mut v = Vec::new();
        app.select_tab(Tab::Overview);
        v.push(("overview", render(&mut app, 118, 34)));
        app.select_tab(Tab::Processes);
        key(&mut app, KeyCode::Home); // the busiest process (the table is sorted by CPU)
        pump(&mut app, 4);
        v.push(("processes", render(&mut app, 118, 30)));
        key(&mut app, KeyCode::Enter); // open the details pane for the selected process
        v.push(("process-details", render(&mut app, 118, 30)));
        key(&mut app, KeyCode::Esc);
        app.select_tab(Tab::Network);
        v.push(("network", render(&mut app, 118, 26)));
        app.select_tab(Tab::Storage);
        v.push(("storage", render(&mut app, 118, 22)));
        app.select_tab(Tab::Overview);
        v.push(("overview-compact", render(&mut app, 72, 22)));
        v
    };
    for (name, text) in shots {
        let path = out.join(format!("{name}.txt"));
        std::fs::write(&path, &text).expect("write screenshot");
        println!("wrote {} ({} lines)", path.display(), text.lines().count());
    }
}

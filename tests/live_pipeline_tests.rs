//! Real collector thread -> App -> renderer, no mocks (headless TestBackend).

use ratatui::{backend::TestBackend, Terminal};
use std::time::{Duration, Instant};
use systui::app::{App, Tab};
use systui::collectors::Collector;
use systui::config::Config;
use systui::ui::{self, theme::Theme, RenderOpts};

fn screen(app: &mut App, w: u16, h: u16) -> String {
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
fn live_data_flows_from_worker_to_screen() {
    let collector = Collector::spawn(Duration::from_millis(500));
    let mut app = App::new(Config::default());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut snaps = 0;
    while snaps < 3 && Instant::now() < deadline {
        if let Some(s) = collector.take_latest() {
            app.ingest(s, Instant::now());
            snaps += 1;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(snaps, 3, "worker must keep publishing");

    let s = screen(&mut app, 120, 40);
    assert!(
        !s.contains("warming up"),
        "CPU warmed up after 3 samples:\n{s}"
    );
    assert!(s.contains("live"), "{s}");
    assert!(s.contains("GiB"), "real memory totals shown:\n{s}");
    assert!(s.contains("History"));
    assert!(app.hist.cpu.len() >= 2);

    app.select_tab(Tab::Processes);
    let s = screen(&mut app, 120, 40);
    let n = app.snapshot.as_ref().unwrap().processes.len();
    assert!(s.contains(&format!("Processes {n}/{n}")), "{s}");
    app.select_tab(Tab::Storage);
    assert!(screen(&mut app, 120, 40).contains("Volumes"));
    app.select_tab(Tab::Network);
    assert!(screen(&mut app, 120, 40).contains("Interfaces"));
}

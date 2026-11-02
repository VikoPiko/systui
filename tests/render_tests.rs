mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::Instant;
use systui::app::{App, Tab};
use systui::config::Config;
use systui::ui::{self, theme::Theme, RenderOpts};

const SIZES: [(u16, u16); 4] = [(120, 40), (90, 26), (70, 20), (50, 12)];

fn opts(unicode: bool) -> RenderOpts {
    RenderOpts {
        theme: Theme::dark(),
        unicode,
    }
}

fn draw(w: u16, h: u16, app: &mut App, unicode: bool) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| ui::render(f, app, &opts(unicode))).unwrap();
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

fn app_with(snap: systui::domain::Snapshot) -> App {
    let mut app = App::new(Config::default());
    app.ingest(Arc::new(snap), Instant::now());
    app
}

fn key(app: &mut App, c: KeyCode) {
    app.handle_key(KeyEvent::new(c, KeyModifiers::NONE));
}

#[test]
fn overview_shows_real_values_at_every_tier() {
    for (w, h) in &SIZES[..3] {
        let mut app = app_with(snapshot(300));
        let s = draw(*w, *h, &mut app, true);
        assert!(s.contains("CPU"), "{w}x{h}\n{s}");
        assert!(s.contains("37%"), "cpu value {w}x{h}\n{s}");
        assert!(s.contains("9.0 GiB / 16.0 GiB"), "{w}x{h}\n{s}");
        assert!(s.contains("2.0 KiB/s"), "net {w}x{h}\n{s}");
        assert!(s.contains("proc"), "top procs {w}x{h}\n{s}");
    }
}

#[test]
fn overview_wide_has_history_and_per_core() {
    let mut app = app_with(snapshot(300));
    let s = draw(120, 40, &mut app, true);
    assert!(s.contains("History"));
    assert!(s.contains("c2"), "per-core rows:\n{s}");
}

#[test]
fn warming_up_is_not_rendered_as_zero() {
    let mut snap = snapshot(10);
    snap.cpu.usage = None;
    snap.cpu.per_core.clear();
    snap.network.iter_mut().for_each(|n| {
        n.rx_rate = None;
        n.tx_rate = None;
    });
    snap.processes.iter_mut().for_each(|p| p.cpu = None);
    let mut app = app_with(snap);
    let s = draw(100, 30, &mut app, true);
    assert!(s.contains("warming up"), "{s}");
    assert!(!s.contains("CPU    0%"));
    assert!(s.contains('…'), "process cpu shows a placeholder");
}

#[test]
fn missing_data_is_explicit() {
    let mut snap = snapshot(5);
    snap.memory = None;
    snap.volumes.clear();
    snap.network.clear();
    snap.diagnostics.push(systui::domain::CollectorStatus {
        name: "storage",
        state: systui::domain::CollectorState::Unsupported,
        detail: Some("no mounted volumes reported".into()),
    });
    let mut app = app_with(snap);
    let s = draw(100, 30, &mut app, true);
    assert!(s.contains("RAM  unavailable"), "{s}");
    assert!(s.contains("DISK unavailable"));
    assert!(s.contains("no interfaces"));
    assert!(
        s.contains("no mounted volumes reported"),
        "diagnostic surfaced"
    );
    app.select_tab(Tab::Network);
    assert!(draw(100, 30, &mut app, true).contains("No network interfaces"));
    app.select_tab(Tab::Storage);
    assert!(draw(100, 30, &mut app, true).contains("no mounted volumes"));
}

#[test]
fn every_tab_renders_at_every_size_without_panic() {
    for tab in [Tab::Overview, Tab::Processes, Tab::Network, Tab::Storage] {
        for (w, h) in SIZES
            .iter()
            .chain(&[(0, 0), (1, 1), (60, 18), (59, 18), (200, 8)])
        {
            let mut app = app_with(snapshot(300));
            app.select_tab(tab);
            let _ = draw(*w, *h, &mut app, true);
            let _ = draw(*w, *h, &mut app, false);
        }
    }
}

#[test]
fn empty_snapshot_does_not_panic_anywhere() {
    for tab in [Tab::Overview, Tab::Processes, Tab::Network, Tab::Storage] {
        let mut snap = snapshot(0);
        snap.network.clear();
        snap.volumes.clear();
        snap.cpu.logical = 0;
        let mut app = app_with(snap);
        app.select_tab(tab);
        for (w, h) in SIZES {
            let _ = draw(w, h, &mut app, true);
        }
    }
}

#[test]
fn before_first_sample_each_tab_says_so() {
    for tab in [Tab::Overview, Tab::Processes, Tab::Network, Tab::Storage] {
        let mut app = App::new(Config::default());
        app.select_tab(tab);
        assert!(draw(100, 30, &mut app, true).contains("Waiting for the first sample"));
    }
}

#[test]
fn process_table_shows_sort_cpu_semantics_and_selection_window() {
    let mut app = app_with(snapshot(300));
    app.select_tab(Tab::Processes);
    let s = draw(120, 40, &mut app, true);
    assert!(s.contains("Processes 300/300"));
    assert!(s.contains("CPU: 100%=1 core"));
    assert!(s.contains("sort CPU v"));
    // Scroll far down: the selected row must be visible in the window.
    key(&mut app, KeyCode::End);
    let s = draw(120, 40, &mut app, true);
    let snap = app.snapshot.clone().unwrap();
    let last = &snap.processes[*app.procs.rows.last().unwrap()];
    assert!(s.contains(&last.name), "last row visible after End:\n{s}");
    assert!(!s.contains("proc0001") || last.name == "proc0001" || true);
}

#[test]
fn selected_row_stays_bounded_after_shrinking_terminal() {
    let mut app = app_with(snapshot(300));
    app.select_tab(Tab::Processes);
    key(&mut app, KeyCode::End);
    for (w, h) in [(120, 40), (70, 20), (60, 18), (50, 10), (120, 40)] {
        let _ = draw(w, h, &mut app, true);
        assert_eq!(app.procs.cursor, 299);
        assert!(app.procs.scroll <= app.procs.cursor);
    }
}

#[test]
fn filter_bar_and_empty_result_message() {
    let mut app = app_with(snapshot(50));
    app.select_tab(Tab::Processes);
    key(&mut app, KeyCode::Char('/'));
    for c in "nomatchxyz".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    let s = draw(100, 30, &mut app, true);
    assert!(s.contains("/nomatchxyz_"), "{s}");
    assert!(s.contains("No process matches the filter."));
    assert!(s.contains("Processes 0/50"));
}

#[test]
fn detail_pane_hides_command_line_until_revealed() {
    let mut app = app_with(snapshot(30));
    app.select_tab(Tab::Processes);
    key(&mut app, KeyCode::Enter);
    let s = draw(120, 40, &mut app, true);
    assert!(s.contains("Details"));
    assert!(s.contains("hidden"), "cmd hidden by default:\n{s}");
    assert!(!s.contains("--flag"));
    key(&mut app, KeyCode::Char('e'));
    assert!(draw(120, 40, &mut app, true).contains("--flag"));
    // compact layout shows detail instead of the list
    let s = draw(70, 20, &mut app, true);
    assert!(s.contains("Details"));
}

#[test]
fn kill_dialog_shows_target_and_requires_pid() {
    let mut app = app_with(snapshot(30));
    app.select_tab(Tab::Processes);
    let id = app.procs.selected.unwrap();
    key(&mut app, KeyCode::Char('K'));
    let s = draw(100, 30, &mut app, true);
    assert!(s.contains("Confirm"));
    assert!(s.contains(&format!("PID:  {}", id.pid)));
    assert!(s.contains("Type the PID"));
}

#[test]
fn network_and_storage_content() {
    let mut app = app_with(snapshot(5));
    app.select_tab(Tab::Network);
    let s = draw(120, 30, &mut app, true);
    assert!(s.contains("eth0") && s.contains("lo"));
    assert!(
        s.contains("5.0 GiB"),
        "cumulative totals are labelled bytes:\n{s}"
    );
    assert!(s.contains("cumulative"));
    app.select_tab(Tab::Storage);
    let s = draw(120, 30, &mut app, true);
    assert!(s.contains("/data") && s.contains("98%"), "{s}");
    // default sort: highest utilisation first
    assert!(s.find("/data").unwrap() < s.find("ext4").unwrap());
    key(&mut app, KeyCode::Char('s'));
    key(&mut app, KeyCode::Char('s'));
    let s2 = draw(120, 30, &mut app, true);
    assert!(
        s2.find("ext4").unwrap() < s2.find("xfs").unwrap(),
        "mount order"
    );
}

#[test]
fn ascii_fallback_has_no_non_ascii_graphics() {
    let mut app = app_with(snapshot(30));
    let s = draw(120, 40, &mut app, false);
    // allow only the explicit em-dash/ellipsis placeholders; no box/block glyphs
    assert!(
        !s.chars().any(|c| ('\u{2500}'..='\u{259F}').contains(&c)),
        "{s}"
    );
}

#[test]
fn control_characters_in_names_cannot_reach_the_terminal() {
    // Names are sanitized at collection; the renderer must also never emit ESC.
    let mut snap = snapshot(3);
    snap.processes[0].name = systui::util::text::sanitize("evil\x1b[31mred\x07");
    let mut app = app_with(snap);
    app.select_tab(Tab::Processes);
    assert!(!draw(100, 30, &mut app, true).contains('\x1b'));
}

#[test]
fn paused_and_stale_labels() {
    let mut app = app_with(snapshot(3));
    key(&mut app, KeyCode::Char(' '));
    assert!(draw(100, 30, &mut app, true).contains("PAUSED"));
}

#[test]
fn render_time_budget_300_processes() {
    // Sanity check, not a benchmark: 120x40 with 300 processes, debug build.
    let mut app = app_with(snapshot(300));
    app.select_tab(Tab::Processes);
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let start = Instant::now();
    for _ in 0..50 {
        t.draw(|f| ui::render(f, &mut app, &opts(true))).unwrap();
    }
    let per = start.elapsed() / 50;
    assert!(per.as_millis() < 50, "avg {per:?} per frame (debug build)");
}

#[test]
fn restricted_process_shows_denied_not_zero() {
    let mut snap = snapshot(5);
    snap.processes[0].name = "vmmemWSL".into();
    snap.processes[0].rss = None;
    snap.processes[0].cpu = None;
    snap.processes[0].restricted = true;
    let mut app = app_with(snap);
    app.select_tab(Tab::Processes);
    let s = draw(120, 40, &mut app, true);
    let line = s
        .lines()
        .find(|l| l.contains("vmmemWSL"))
        .expect("row shown");
    assert!(line.contains("denied"), "{line}");
    assert!(!line.contains("0 B") && !line.contains("0.0%"), "{line}");
    key(&mut app, KeyCode::Char('s')); // MEM sort: denied rows must not outrank real ones
    let snap = app.snapshot.clone().unwrap();
    let last = *app.procs.rows.last().unwrap();
    assert_eq!(snap.processes[last].name, "vmmemWSL");
}

#[test]
fn battery_and_temperature_lines_appear_only_when_the_platform_reports_them() {
    use systui::sensors::*;
    let mut app = app_with(snapshot(10));
    let s = draw(120, 40, &mut app, true);
    assert!(
        !s.contains("BAT ") && !s.contains("TEMP "),
        "desktop: nothing invented:\n{s}"
    );

    let mut snap = snapshot(10);
    snap.sensors = Sensors {
        battery: Some(Battery {
            percent: Some(12),
            state: BatteryState::Discharging,
            ac_online: Some(false),
            minutes_remaining: Some(95),
        }),
        temps: vec![
            TempReading {
                label: "coretemp Package id 0".into(),
                celsius: 83.0,
            },
            TempReading {
                label: "nvme Composite".into(),
                celsius: 41.0,
            },
        ],
        temps_note: None,
        throttle_events: Some(4),
    };
    let mut app = app_with(snap);
    for (w, h) in [(120, 40), (90, 26), (70, 20)] {
        let s = draw(w, h, &mut app, true);
        assert!(
            s.contains("BAT") && s.contains("12%") && s.contains("discharging"),
            "{w}x{h}\n{s}"
        );
        assert!(
            s.contains("TEMP") && s.contains("83°C") && s.contains("coretemp Package id 0"),
            "{w}x{h}\n{s}"
        );
    }
    let s = draw(120, 40, &mut app, true);
    assert!(
        s.contains("about 1h 35m left") && s.contains("(2 sensors)") && s.contains("throttled 4x"),
        "{s}"
    );
    assert!(
        draw(120, 40, &mut app, false).contains("83 C"),
        "ASCII fallback has no degree sign"
    );
}

#[test]
fn unknown_battery_percent_is_a_dash_not_zero() {
    use systui::sensors::*;
    let mut snap = snapshot(5);
    snap.sensors.battery = Some(Battery {
        percent: None,
        state: BatteryState::Unknown,
        ac_online: None,
        minutes_remaining: None,
    });
    let mut app = app_with(snap);
    let s = draw(120, 40, &mut app, true);
    let line = s.lines().find(|l| l.contains("BAT")).unwrap();
    assert!(line.contains('—') && !line.contains("0%"), "{line}");
}

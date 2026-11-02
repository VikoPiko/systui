mod common;
use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::sync::Arc;
use std::time::{Duration, Instant};
use systui::app::{App, Effect, Mode, Tab};
use systui::config::{Config, SortKey};
use systui::domain::ProcessSample;
use systui::ports::{Listener, Listeners};
use systui::processes::ProcessView;
use systui::ui::{self, theme::Theme, RenderOpts};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}
fn press(a: &mut App, s: &str) {
    for ch in s.chars() {
        a.handle_key(k(KeyCode::Char(ch)));
    }
}
fn draw_with(a: &mut App, w: u16, h: u16, unicode: bool) -> String {
    let opts = RenderOpts {
        theme: Theme::mono(),
        unicode,
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
fn draw(a: &mut App, w: u16, h: u16) -> String {
    draw_with(a, w, h, true)
}

/// parent/child process builder
fn pp(pid: u32, parent: Option<u32>, name: &str, cpu: f32, rss_mb: u64) -> ProcessSample {
    let mut p = proc(pid, name, Some(cpu), rss_mb << 20);
    p.parent = parent;
    p
}

/// 1 -> {2 (cpu 5), 3 (cpu 50)};  2 -> 4 -> 5;  9 orphan (parent missing)
fn forest() -> Vec<ProcessSample> {
    vec![
        pp(1, None, "init", 1.0, 10),
        pp(2, Some(1), "svc-a", 5.0, 100),
        pp(3, Some(1), "svc-b", 50.0, 200),
        pp(4, Some(2), "worker", 2.0, 300),
        pp(5, Some(4), "leaf", 1.0, 400),
        pp(9, Some(777), "orphan", 0.5, 20),
    ]
}

fn names(v: &ProcessView, procs: &[ProcessSample]) -> Vec<String> {
    v.rows.iter().map(|&i| procs[i].name.clone()).collect()
}

fn tree_view(procs: &[ProcessSample]) -> ProcessView {
    let mut v = ProcessView::new(SortKey::Cpu, 30);
    v.tree = true;
    v.rebuild(procs);
    v
}

// ----------------------------------------------------------------- tree

#[test]
fn tree_orders_depth_first_with_siblings_sorted_by_the_current_key() {
    let procs = forest();
    let v = tree_view(&procs);
    // CPU desc at every level; roots: init (1.0) vs orphan (0.5)
    assert_eq!(
        names(&v, &procs),
        ["init", "svc-b", "svc-a", "worker", "leaf", "orphan"]
    );
    let depths: Vec<u16> = v.tree_rows.iter().map(|t| t.depth).collect();
    assert_eq!(depths, [0, 1, 1, 2, 3, 0]);
    assert_eq!(v.tree_rows[0].kids, 2);
    assert_eq!(v.tree_rows[4].kids, 0);
}

#[test]
fn subtree_totals_sum_known_values_only() {
    let mut procs = forest();
    procs[4].rss = None; // leaf unreadable
    procs[4].restricted = true;
    let v = tree_view(&procs);
    let root = &v.tree_rows[0];
    assert_eq!(root.sub_count, 5);
    assert_eq!(
        root.sub_rss,
        (10 + 100 + 200 + 300) << 20,
        "unknown leaf not counted as 0"
    );
    assert_eq!(root.sub_rss_unknown, 1);
    assert_eq!(root.sub_cpu, Some(1.0 + 5.0 + 50.0 + 2.0 + 1.0));
}

#[test]
fn collapse_hides_descendants_and_selection_stays_put() {
    let procs = forest();
    let mut v = tree_view(&procs);
    let svc_a = v
        .rows
        .iter()
        .position(|&i| procs[i].name == "svc-a")
        .unwrap();
    v.move_to(svc_a, &procs);
    let id = v.selected.unwrap();
    v.tree_left(&procs); // collapse svc-a
    assert_eq!(names(&v, &procs), ["init", "svc-b", "svc-a", "orphan"]);
    assert_eq!(v.selected, Some(id), "still on svc-a");
    assert!(v.tree_rows[v.cursor].collapsed);
    assert_eq!(v.tree_rows[v.cursor].sub_count, 3, "svc-a + worker + leaf");
    v.tree_right(&procs); // expand again
    assert_eq!(v.rows.len(), 6);
    assert_eq!(v.selected, Some(id));
}

#[test]
fn collapse_survives_refreshes_by_identity() {
    let mut procs = forest();
    let mut v = tree_view(&procs);
    let svc_a = v
        .rows
        .iter()
        .position(|&i| procs[i].name == "svc-a")
        .unwrap();
    v.move_to(svc_a, &procs);
    v.tree_left(&procs);
    // new snapshot, values changed, order of the input vector shuffled
    for p in procs.iter_mut() {
        p.cpu = p.cpu.map(|c| c + 0.1);
    }
    procs.reverse();
    v.rebuild(&procs);
    assert!(
        !names(&v, &procs).contains(&"worker".to_string()),
        "svc-a stays collapsed"
    );
}

#[test]
fn left_right_navigate_the_hierarchy() {
    let procs = forest();
    let mut v = tree_view(&procs);
    let worker = v
        .rows
        .iter()
        .position(|&i| procs[i].name == "worker")
        .unwrap();
    v.move_to(worker, &procs);
    v.tree_left(&procs); // worker has a child and is expanded => collapses
    assert!(v.tree_rows[v.cursor].collapsed);
    v.tree_left(&procs); // already collapsed => jump to parent
    assert_eq!(procs[v.rows[v.cursor]].name, "svc-a");
    v.tree_right(&procs); // expanded with kids => first child
    assert_eq!(procs[v.rows[v.cursor]].name, "worker");
    let leaf_row = v
        .rows
        .iter()
        .position(|&i| procs[i].name == "orphan")
        .unwrap();
    v.move_to(leaf_row, &procs);
    v.tree_left(&procs); // root with no kids: nothing to do, no panic
    assert_eq!(procs[v.rows[v.cursor]].name, "orphan");
}

#[test]
fn cycles_self_parents_and_pid_reuse_cannot_hang_or_lose_rows() {
    let mut procs = vec![
        pp(20, Some(21), "cycle-a", 1.0, 1),
        pp(21, Some(20), "cycle-b", 1.0, 1),
        pp(22, Some(22), "self-parent", 1.0, 1),
        pp(30, Some(31), "child-of-reused", 1.0, 1),
        pp(31, None, "reused-pid", 1.0, 1),
    ];
    // pid 31 now belongs to a process that started AFTER its supposed child
    procs[4].id.started_at = Some(procs[3].id.started_at.unwrap() + 1000);
    let v = tree_view(&procs);
    let mut got = names(&v, &procs);
    got.sort();
    assert_eq!(got.len(), 5, "every process appears exactly once: {got:?}");
    let child = v
        .rows
        .iter()
        .position(|&i| procs[i].name == "child-of-reused")
        .unwrap();
    assert_eq!(
        v.tree_rows[child].depth, 0,
        "reused PID is not accepted as its parent"
    );
}

#[test]
fn filter_and_family_pause_the_tree_and_flat_mode_is_unchanged() {
    let procs = forest();
    let mut v = tree_view(&procs);
    assert!(v.tree_active());
    v.filter = "svc".into();
    v.rebuild(&procs);
    assert!(!v.tree_active() && v.tree_rows.is_empty());
    assert_eq!(v.rows.len(), 2);
    v.filter.clear();
    v.tree = false;
    v.rebuild(&procs);
    assert!(v.tree_rows.is_empty());
    assert_eq!(v.rows.len(), 6);
}

#[test]
fn tree_scales_to_hundreds_of_processes() {
    let mut procs = many_procs(600);
    for (i, p) in procs.iter_mut().enumerate() {
        p.parent = if i == 0 {
            None
        } else {
            Some(1 + (i as u32 - 1) / 3)
        }; // ternary tree
    }
    let t = Instant::now();
    let v = tree_view(&procs);
    assert_eq!(v.rows.len(), 600);
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(v.tree_rows[0].sub_count, 600);
}

#[test]
fn tree_mode_renders_markers_indentation_and_collapsed_sums() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot_with(1, forest())), Instant::now());
    a.select_tab(Tab::Processes);
    press(&mut a, "t");
    assert!(a.procs.tree);
    let s = draw(&mut a, 120, 40);
    assert!(s.contains("tree (Σ = subtree sum)"), "{s}");
    assert!(
        s.contains("▾ init") && s.contains("    ▾ worker") || s.contains("  ▾ svc-a"),
        "{s}"
    );
    // collapse svc-a: row shows the subtree sum and the descendant count
    let row = a
        .procs
        .rows
        .iter()
        .position(|&i| forest()[i].name == "svc-a")
        .unwrap();
    let snap = a.snapshot.clone().unwrap();
    let row = a
        .procs
        .rows
        .iter()
        .position(|&i| snap.processes[i].name == "svc-a")
        .unwrap_or(row);
    a.procs.move_to(row, &snap.processes);
    a.handle_key(k(KeyCode::Left));
    let s = draw(&mut a, 120, 40);
    assert!(s.contains("▸ svc-a (+2)"), "{s}");
    assert!(s.contains("Σ 800 MiB"), "100+300+400 MiB subtree: {s}");
    // ASCII fallback uses +/-
    let s = draw_with(&mut a, 120, 40, false);
    assert!(s.contains("+ svc-a (+2)") && s.contains("- init"), "{s}");
    // filtering pauses the tree with an explanation
    press(&mut a, "/svc");
    assert!(draw(&mut a, 120, 40).contains("tree paused while filtering"));
}

#[test]
fn t_key_toggles_and_left_right_are_ignored_in_flat_mode() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot_with(1, forest())), Instant::now());
    a.select_tab(Tab::Processes);
    let before = a.procs.cursor;
    a.handle_key(k(KeyCode::Left));
    a.handle_key(k(KeyCode::Right));
    assert_eq!(a.procs.cursor, before);
    press(&mut a, "t");
    assert!(a.notice.as_deref().unwrap().contains("tree view on"));
    press(&mut a, "t");
    assert!(!a.procs.tree && a.procs.tree_rows.is_empty());
}

// ------------------------------------------------------------ what changed

fn proc_set(extra: &[ProcessSample], cpu_a: f32, rss_a_mb: u64) -> Vec<ProcessSample> {
    let mut v = vec![
        pp(1, None, "steady", 1.0, 10),
        pp(2, None, "mover", cpu_a, rss_a_mb),
    ];
    v.extend_from_slice(extra);
    v
}

fn feed(a: &mut App, t0: Instant, secs: u64, procs: Vec<ProcessSample>) {
    a.ingest(
        Arc::new(snapshot_with(secs, procs)),
        t0 + Duration::from_secs(secs),
    );
}

#[test]
fn w_opens_the_overlay_and_explains_when_history_is_missing() {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot_with(1, forest())), Instant::now());
    press(&mut a, "w");
    assert_eq!(a.mode, Mode::Changes(1));
    assert!(draw(&mut a, 120, 40).contains("Collecting history"));
    press(&mut a, "w");
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn overlay_reports_started_exited_and_deltas_between_real_snapshots() {
    let mut a = App::new(Config::default());
    let t0 = Instant::now();
    feed(
        &mut a,
        t0,
        0,
        proc_set(&[pp(7, None, "short-lived", 3.0, 50)], 5.0, 100),
    );
    feed(
        &mut a,
        t0,
        30,
        proc_set(&[pp(8, None, "brand-new", 9.0, 250)], 70.0, 900),
    );
    press(&mut a, "w");
    let s = draw(&mut a, 120, 40);
    assert!(s.contains("compare with ~30s"), "{s}");
    assert!(s.contains("Started (1)") && s.contains("brand-new"), "{s}");
    assert!(s.contains("Exited (1)") && s.contains("short-lived"), "{s}");
    assert!(
        s.contains("mover") && s.contains("5% -> 70%"),
        "cpu delta: {s}"
    );
    assert!(s.contains("100 MiB -> 900 MiB"), "mem delta: {s}");
    assert!(s.contains("Session-only, in memory"));
    // window cycling
    a.handle_key(k(KeyCode::Char(']')));
    assert_eq!(a.mode, Mode::Changes(2));
    a.handle_key(k(KeyCode::Char(']')));
    assert_eq!(a.mode, Mode::Changes(2), "clamped");
    a.handle_key(k(KeyCode::Char('[')));
    a.handle_key(k(KeyCode::Char('[')));
    a.handle_key(k(KeyCode::Char('[')));
    assert_eq!(a.mode, Mode::Changes(0));
    assert!(draw(&mut a, 120, 40).contains("~5s"));
    a.handle_key(k(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn stale_process_lists_are_not_recorded_as_history() {
    let mut a = App::new(Config::default());
    let t0 = Instant::now();
    feed(&mut a, t0, 0, proc_set(&[], 5.0, 100));
    for i in 1..20u64 {
        let mut s = snapshot_with(i, vec![]);
        s.processes_fresh = false;
        a.ingest(Arc::new(s), t0 + Duration::from_secs(i * 6));
    }
    assert_eq!(
        a.changes.len(),
        1,
        "non-fresh lists must not enter the diff ring"
    );
}

#[test]
fn overlay_never_panics_at_any_size_and_via_the_palette() {
    let mut a = App::new(Config::default());
    let t0 = Instant::now();
    feed(&mut a, t0, 0, proc_set(&[], 5.0, 100));
    feed(
        &mut a,
        t0,
        10,
        proc_set(&[pp(8, None, "x", 9.0, 250)], 70.0, 900),
    );
    a.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    press(&mut a, "what changed");
    a.handle_key(k(KeyCode::Enter));
    assert!(matches!(a.mode, Mode::Changes(_)));
    for (w, h) in [(120, 40), (80, 24), (60, 18), (0, 0), (1, 1), (200, 10)] {
        let _ = draw(&mut a, w, h);
    }
}

// ------------------------------------------------------------- ports view

fn listeners() -> Listeners {
    let l = |proto, addr: &str, port, pid| Listener {
        proto,
        addr: addr.into(),
        port,
        pid,
    };
    Listeners {
        items: vec![
            l("tcp", "127.0.0.1", 5432, Some(2)),
            l("tcp", "[::]", 8080, None),
            l("udp", "0.0.0.0", 5353, Some(3)),
        ],
        truncated: false,
    }
}

fn net_app() -> App {
    let mut a = App::new(Config::default());
    a.ingest(Arc::new(snapshot_with(1, forest())), Instant::now());
    a.select_tab(Tab::Network);
    a.take_effects();
    a
}

#[test]
fn l_requests_one_collection_and_asks_for_process_names() {
    let mut a = net_app();
    assert!(
        !a.desired_needs().processes,
        "Network tab alone does not refresh processes"
    );
    press(&mut a, "L");
    assert!(a.ports.open && a.ports.loading);
    let eff = a.take_effects();
    assert_eq!(
        eff.iter()
            .filter(|e| **e == Effect::CollectListeners)
            .count(),
        1
    );
    assert!(
        a.desired_needs().processes,
        "owner names need the process list while this view is open"
    );
    press(&mut a, "R"); // already loading: no second command
    assert!(!a.take_effects().contains(&Effect::CollectListeners));
    a.ingest_ports(Ok(listeners()));
    assert!(!a.ports.loading);
    press(&mut a, "R");
    assert!(
        a.take_effects().contains(&Effect::CollectListeners),
        "explicit refresh"
    );
    a.ingest_ports(Ok(listeners()));
    a.handle_key(k(KeyCode::Esc));
    assert!(!a.ports.open);
    assert!(
        !a.desired_needs().processes,
        "back to the cheap Network needs"
    );
}

#[test]
fn nothing_is_collected_until_asked() {
    let mut a = net_app();
    for _ in 0..5 {
        a.ingest(Arc::new(snapshot_with(2, forest())), Instant::now());
    }
    assert!(!a.take_effects().contains(&Effect::CollectListeners));
    assert!(a.ports.data.is_none());
}

#[test]
fn ports_table_shows_owner_names_unknown_owners_and_the_honesty_note() {
    let mut a = net_app();
    press(&mut a, "L");
    assert!(draw(&mut a, 120, 40).contains("collecting"));
    a.ingest_ports(Ok(listeners()));
    let s = draw(&mut a, 120, 40);
    for want in [
        "Listening sockets (3)",
        "5432",
        "svc-a",
        "owner unknown",
        "5353",
        "svc-b",
        "not traffic capture",
    ] {
        assert!(s.contains(want), "missing `{want}`:\n{s}");
    }
    let mut t = listeners();
    t.truncated = true;
    a.ingest_ports(Ok(t));
    assert!(draw(&mut a, 120, 40).contains("OUTPUT TRUNCATED"));
    a.ingest_ports(Err("`ss` is not available on this machine".into()));
    assert!(draw(&mut a, 120, 40).contains("unavailable: `ss` is not available"));
    a.ingest_ports(Ok(Listeners {
        items: vec![],
        truncated: false,
    }));
    let _ = draw(&mut a, 70, 20);
}

#[test]
fn ports_navigation_is_bounded() {
    let mut a = net_app();
    press(&mut a, "L");
    a.ingest_ports(Ok(listeners()));
    for _ in 0..10 {
        press(&mut a, "j");
    }
    assert_eq!(a.ports.table.selected(), Some(2));
    for _ in 0..10 {
        press(&mut a, "k");
    }
    assert_eq!(a.ports.table.selected(), Some(0));
    a.handle_key(k(KeyCode::End));
    assert_eq!(a.ports.table.selected(), Some(2));
    // keys that mean something elsewhere must not leak to the interface list
    assert!(a.net.table.selected().is_some());
}

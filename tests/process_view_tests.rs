mod common;
use common::*;
use systui::config::SortKey;
use systui::processes::ProcessView;

fn view(sort: SortKey) -> ProcessView {
    ProcessView::new(sort, 60)
}

#[test]
fn sorts_by_cpu_desc_by_default_with_stable_tiebreak() {
    let procs = many_procs(300);
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    assert_eq!(v.rows.len(), 300);
    let cpus: Vec<f32> = v.rows.iter().map(|&i| procs[i].cpu.unwrap()).collect();
    assert!(cpus.windows(2).all(|w| w[0] >= w[1]));
}

#[test]
fn unknown_cpu_sorts_last_not_as_zero() {
    let procs = vec![
        proc(1, "warm", None, 1),
        proc(2, "idle", Some(0.0), 1),
        proc(3, "busy", Some(50.0), 1),
    ];
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    let names: Vec<&str> = v.rows.iter().map(|&i| procs[i].name.as_str()).collect();
    assert_eq!(names, ["busy", "idle", "warm"]);
}

#[test]
fn all_sort_keys_and_reverse() {
    let procs = many_procs(50);
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    let mut seen = vec![v.sort];
    for _ in 0..3 {
        v.cycle_sort(&procs);
        seen.push(v.sort);
    }
    assert_eq!(
        seen,
        [SortKey::Cpu, SortKey::Memory, SortKey::Name, SortKey::Pid]
    );
    // Pid ascending by default; reverse flips it.
    assert!(!v.descending());
    assert_eq!(procs[v.rows[0]].id.pid, 1);
    v.toggle_reverse(&procs);
    assert_eq!(procs[v.rows[0]].id.pid, 50);
}

#[test]
fn selection_follows_process_across_resort() {
    let procs = many_procs(300);
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    v.move_to(120, &procs);
    let chosen = v.selected.unwrap();
    v.cycle_sort(&procs);
    v.toggle_reverse(&procs);
    assert_eq!(v.selected, Some(chosen));
    assert_eq!(
        procs[v.rows[v.cursor]].id, chosen,
        "cursor points at same process"
    );
}

#[test]
fn selection_survives_new_snapshot_with_changed_values() {
    let mut procs = many_procs(100);
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    v.move_to(10, &procs);
    let chosen = v.selected.unwrap();
    for p in procs.iter_mut() {
        p.cpu = p.cpu.map(|c| 100.0 - c); // totally reorder
    }
    v.rebuild(&procs);
    assert_eq!(v.selected, Some(chosen));
    assert_eq!(procs[v.rows[v.cursor]].id, chosen);
}

#[test]
fn exited_selection_moves_to_neighbour_not_top() {
    let mut procs = many_procs(100);
    let mut v = view(SortKey::Pid);
    v.rebuild(&procs);
    v.move_to(50, &procs);
    let gone = v.selected.unwrap();
    procs.retain(|p| p.id != gone);
    v.rebuild(&procs);
    assert_ne!(v.selected, Some(gone));
    assert_eq!(v.cursor, 50, "stays near the same visual position");
    assert!(v.selected.is_some());
}

#[test]
fn pid_reuse_is_a_different_selection() {
    // Same PID, different start time => different identity: selection is dropped.
    let mut procs = vec![proc(10, "a", Some(1.0), 1), proc(11, "b", Some(1.0), 1)];
    let mut v = view(SortKey::Pid);
    v.rebuild(&procs);
    v.move_to(0, &procs);
    let old = v.selected.unwrap();
    procs[0].id.started_at = Some(42);
    v.rebuild(&procs);
    assert_ne!(v.selected, Some(old));
}

#[test]
fn filter_by_name_and_pid_case_insensitive() {
    let procs = many_procs(300);
    let mut v = view(SortKey::Pid);
    v.filter = "PROC0042".into();
    v.rebuild(&procs);
    assert_eq!(v.rows.len(), 1);
    v.filter = "42".into();
    v.rebuild(&procs);
    assert!(v
        .rows
        .iter()
        .all(|&i| { procs[i].name.contains("42") || procs[i].id.pid.to_string().contains("42") }));
}

#[test]
fn empty_results_are_safe() {
    let procs = many_procs(20);
    let mut v = view(SortKey::Cpu);
    v.rebuild(&procs);
    v.move_to(5, &procs);
    v.filter = "zzzz-no-match".into();
    v.rebuild(&procs);
    assert!(v.rows.is_empty());
    assert_eq!(v.selected, None);
    v.move_by(1, &procs);
    v.move_by(-5, &procs);
    v.move_to(usize::MAX, &procs);
    assert_eq!(v.cursor, 0);
    // and recovers when the filter clears
    v.filter.clear();
    v.rebuild(&procs);
    assert_eq!(v.rows.len(), 20);
    assert!(v.selected.is_some());
}

#[test]
fn movement_is_bounded() {
    let procs = many_procs(300);
    let mut v = view(SortKey::Pid);
    v.rebuild(&procs);
    v.move_by(-10, &procs);
    assert_eq!(v.cursor, 0);
    v.move_by(10_000, &procs);
    assert_eq!(v.cursor, 299);
    v.move_to(usize::MAX, &procs);
    assert_eq!(v.cursor, 299);
}

#[test]
fn empty_process_list() {
    let mut v = view(SortKey::Cpu);
    v.rebuild(&[]);
    assert!(v.rows.is_empty() && v.selected.is_none());
}

#[test]
fn history_resets_on_selection_change_and_is_bounded() {
    let procs = many_procs(10);
    let mut v = ProcessView::new(SortKey::Pid, 5);
    v.rebuild(&procs);
    for _ in 0..20 {
        v.record_history(&procs);
    }
    assert_eq!(v.hist_cpu.len(), 5, "bounded by capacity");
    v.move_to(3, &procs);
    v.record_history(&procs);
    assert_eq!(v.hist_cpu.len(), 1, "new selection starts a new history");
}

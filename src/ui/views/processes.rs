//! Process explorer: windowed table (only visible rows are built) + details.

use super::{cpu_cell, mem_cell, scroll_for};
use crate::app::{App, Focus, Mode};
use crate::config::CpuMode;
use crate::domain::{CmdLine, ProcessSample, Snapshot};
use crate::ui::layout::SizeTier;
use crate::ui::widgets::spark;
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    let th = &opts.theme;
    let Some(snap) = app.snapshot.clone() else {
        frame.render_widget(
            Paragraph::new("Waiting for the first sample… (no readings yet)").style(th.dim),
            area,
        );
        return;
    };
    if !snap.processes_fresh {
        frame.render_widget(
            Paragraph::new("Refreshing the process list…").style(th.dim),
            area,
        );
        return;
    }
    let detail = app.procs.detail_open && app.procs.selected.is_some();
    match (detail, t) {
        (true, SizeTier::Compact) => render_detail(frame, area, app, &snap, opts),
        (true, _) => {
            let cols = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(area);
            render_list(frame, cols[0], app, &snap, opts, t);
            render_detail(frame, cols[1], app, &snap, opts);
        }
        _ => render_list(frame, area, app, &snap, opts, t),
    }
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    snap: &Snapshot,
    opts: &RenderOpts,
    t: SizeTier,
) {
    let th = &opts.theme;
    let pv = &app.procs;
    let arrow = if pv.descending() { "v" } else { "^" };
    let cpu_note = match app.config.processes.cpu_mode {
        CpuMode::PerCoreEquivalent => "CPU: 100%=1 core",
        CpuMode::HostShare => "CPU: share of host",
    };
    let mut title = format!(
        "Processes {}/{}  sort {} {arrow}  {cpu_note}",
        pv.rows.len(),
        snap.processes.len(),
        pv.sort_label()
    );
    if app.proc_io {
        title.push_str("  I/O on");
    }
    if pv.tree_active() {
        title.push_str("  tree (Σ = subtree sum)");
    } else if pv.tree {
        title.push_str("  tree paused while filtering");
    }
    if pv.family.is_some() {
        title.push_str("  family filter (Esc clears)");
    }
    if !pv.filter.is_empty() {
        title.push_str(&format!("  filter \"{}\"", pv.filter));
    }
    let block = panel(title, app.focus == Focus::Primary, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return;
    }

    let show_filter = app.mode == Mode::Filter || !app.procs.filter.is_empty();
    let parts = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(u16::from(show_filter)),
    ])
    .split(inner);
    let list = parts[0];
    let body_h = list.height.saturating_sub(1) as usize; // minus header
    app.procs.page = body_h.max(1);

    let total = app.procs.rows.len();
    let cursor = app.procs.cursor;
    let scroll = scroll_for(cursor, app.procs.scroll, body_h);
    app.procs.scroll = scroll;

    let compact = t == SizeTier::Compact;
    let mut rows = Vec::with_capacity(body_h);
    for (k, &i) in app.procs.rows.iter().enumerate().skip(scroll).take(body_h) {
        let tr = app.procs.tree_rows.get(k);
        rows.push(proc_row(&snap.processes[i], app, compact, tr, opts));
    }
    let io_cols = app.procs_io_visible(compact);
    let mut widths: Vec<Constraint> = if compact {
        vec![
            Constraint::Length(7),
            Constraint::Min(8),
            Constraint::Length(7),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Length(7),
            Constraint::Min(12),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(7),
        ]
    };
    let mut header: Vec<&str> = if compact {
        vec!["PID", "NAME", "CPU", "MEM"]
    } else {
        vec!["PID", "NAME", "CPU", "MEM", "STATUS", "PPID"]
    };
    if io_cols {
        widths.insert(4, Constraint::Length(11));
        widths.insert(4, Constraint::Length(11));
        header.insert(4, "WRITE/s");
        header.insert(4, "READ/s");
    }
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(th.dim))
        .row_highlight_style(th.selected)
        .style(th.text);
    let mut st = TableState::default();
    if total > 0 {
        st.select(Some(cursor.saturating_sub(scroll)));
    }
    frame.render_stateful_widget(table, list, &mut st);
    if total == 0 {
        let msg = if snap.processes.is_empty() {
            "No processes could be listed."
        } else {
            "No process matches the filter."
        };
        let r = Rect::new(
            list.x,
            list.y + 1,
            list.width,
            1.min(list.height.saturating_sub(1)),
        );
        frame.render_widget(Paragraph::new(msg).style(th.dim), r);
    }
    if show_filter {
        let cursor_mark = if app.mode == Mode::Filter { "_" } else { "" };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("/", th.accent),
                Span::raw(format!("{}{cursor_mark}", app.procs.filter)),
            ])),
            parts[1],
        );
    }
}

fn proc_row<'a>(
    p: &ProcessSample,
    app: &App,
    compact: bool,
    tree: Option<&crate::processes::TreeRow>,
    opts: &RenderOpts,
) -> Row<'a> {
    let (mut name, mut cpu, mut mem) = (p.name.clone(), cpu_cell(p, app), mem_cell(p));
    if let Some(tr) = tree {
        let marker = match (tr.kids > 0, tr.collapsed, opts.unicode) {
            (false, _, _) => "  ",
            (true, false, true) => "▾ ",
            (true, true, true) => "▸ ",
            (true, false, false) => "- ",
            (true, true, false) => "+ ",
        };
        name = format!(
            "{}{marker}{}",
            "  ".repeat(tr.depth.min(12) as usize),
            p.name
        );
        if tr.collapsed {
            // Collapsed: show the subtree totals instead of this process alone.
            name.push_str(&format!(" (+{})", tr.sub_count - 1));
            mem = format!(
                "Σ {}{}",
                format::bytes(tr.sub_rss),
                if tr.sub_rss_unknown > 0 { "+?" } else { "" }
            );
            cpu = tr.sub_cpu.map_or("Σ …".to_string(), |c| {
                format!("Σ {}", format::percent_fine(Some(app.cpu_display(c))))
            });
        }
    }
    let mut cells = vec![
        Cell::from(p.id.pid.to_string()),
        Cell::from(name),
        Cell::from(cpu),
        Cell::from(mem),
    ];
    if app.proc_io && !compact {
        let rate = |v: Option<f64>| match (v, p.restricted) {
            (Some(x), _) => format::rate(x),
            (None, true) => "denied".to_string(),
            (None, false) => "…".to_string(),
        };
        cells.push(Cell::from(rate(p.io.map(|x| x.0))));
        cells.push(Cell::from(rate(p.io.map(|x| x.1))));
    }
    if !compact {
        cells.push(Cell::from(p.status.clone().unwrap_or_else(|| "—".into())));
        cells.push(Cell::from(
            p.parent.map_or_else(|| "—".into(), |x| x.to_string()),
        ));
    }
    Row::new(cells)
}

fn render_detail(frame: &mut Frame, area: Rect, app: &App, snap: &Snapshot, opts: &RenderOpts) {
    let th = &opts.theme;
    let block = panel(
        "Details (Esc close)".into(),
        app.focus == Focus::Secondary,
        opts,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(p) = app.procs.selected_sample(&snap.processes) else {
        frame.render_widget(
            Paragraph::new("The selected process is no longer in the list.").style(th.warn),
            inner,
        );
        return;
    };
    let kv = |k: &str, v: String| {
        Line::from(vec![Span::styled(format!("{k:<8}"), th.dim), Span::raw(v)])
    };
    let parent = p.parent.map(|pp| {
        let name = snap
            .processes
            .iter()
            .find(|x| x.id.pid == pp)
            .map_or("not in list", |x| x.name.as_str());
        format!("{pp} ({name})")
    });
    let started = match p.id.started_at {
        Some(s) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(s);
            format!("running {}", format::duration(now.saturating_sub(s)))
        }
        None => "unknown (reduced identity safety)".into(),
    };
    let cmd = match (&p.info.cmd, app.procs.reveal_cmd) {
        (CmdLine::Available(c), true) => c.clone(),
        (CmdLine::Available(_), false) => "hidden — press e to reveal".into(),
        (CmdLine::Unavailable, _) => "unavailable (access restricted or none)".into(),
        (CmdLine::NotRequested, _) => "loading…".into(),
    };
    let w = inner.width.saturating_sub(10) as usize;
    let mut lines = vec![
        kv("Name", p.name.clone()),
        kv("PID", p.id.pid.to_string()),
        kv("Parent", parent.unwrap_or_else(|| "none reported".into())),
        kv(
            "Status",
            p.status
                .clone()
                .unwrap_or_else(|| "not reported by this OS API".into()),
        ),
        kv("Started", started),
        kv(
            "CPU",
            match (p.restricted, p.cpu) {
                (true, _) => "denied (access restricted)".to_string(),
                (_, Some(c)) => format!("{:.1}%", app.cpu_display(c)),
                (_, None) => "warming up…".to_string(),
            },
        ),
        kv(
            "Memory",
            match p.rss {
                Some(r) => format!("{} (resident/working set)", format::bytes(r)),
                None => "denied (access restricted; counters unavailable)".into(),
            },
        ),
        kv(
            "I/O",
            if !app.proc_io {
                "off (press i)".to_string()
            } else {
                match (p.restricted, p.io) {
                    (true, _) => "denied (access restricted)".to_string(),
                    (_, Some((r, w))) => {
                        format!("read {}  write {}", format::rate(r), format::rate(w))
                    }
                    (_, None) => "warming up…".to_string(),
                }
            },
        ),
        kv(
            "Path",
            p.info
                .exe
                .clone()
                .unwrap_or_else(|| "unavailable (access restricted)".into()),
        ),
        kv("Cmd", cmd),
        Line::from(""),
        Line::from(vec![
            Span::styled("CPU hist ", th.dim),
            Span::styled(
                spark(&app.procs.hist_cpu.to_vec(), w.min(40), None, opts.unicode),
                th.graph,
            ),
        ]),
        Line::from(vec![
            Span::styled("MEM hist ", th.dim),
            Span::styled(
                spark(&app.procs.hist_rss.to_vec(), w.min(40), None, opts.unicode),
                th.graph,
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("p parent  c child  K terminate", th.dim)),
    ];
    // keep only what fits
    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

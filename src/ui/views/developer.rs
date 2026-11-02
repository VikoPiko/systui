//! Dev tab: process families + optional GPU / Docker / WSL panels.
//! Every panel states clearly whether it is disabled, unavailable, failing or stale.

use crate::app::{App, DevPane, ModView};
use crate::modules::{ModStatus, ModuleSnapshot};
use crate::ui::layout::SizeTier;
use crate::ui::widgets::{bar, pct_style};
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
    Frame,
};
use std::time::Instant;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    if app.snapshot.is_none() {
        frame.render_widget(
            Paragraph::new("Waiting for the first sample… (no readings yet)").style(opts.theme.dim),
            area,
        );
        return;
    }
    if t == SizeTier::Compact {
        // One panel at a time; Tab cycles.
        match app.dev.pane {
            DevPane::Families => render_families(frame, area, app, opts),
            DevPane::Docker => render_docker(frame, area, app, opts),
            DevPane::Wsl => render_wsl(frame, area, app, opts),
            DevPane::Gpu => render_gpu(frame, area, app, opts),
        }
        return;
    }
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    render_families(frame, cols[0], app, opts);
    let gpu_h = (app
        .dev
        .gpu
        .snap
        .as_ref()
        .and_then(|s| s.data.as_ref())
        .map_or(1, |d| d.gpus.len().max(1)) as u16
        * 3
        + 5) // driver line + 3 lines per GPU + footnote, plus the border
    .clamp(5, 17);
    let right = Layout::vertical([
        Constraint::Length(gpu_h.min(cols[1].height / 2)),
        Constraint::Min(6),
        Constraint::Length(8.min(cols[1].height / 3)),
    ])
    .split(cols[1]);
    render_gpu(frame, right[0], app, opts);
    render_docker(frame, right[1], app, opts);
    render_wsl(frame, right[2], app, opts);
}

fn stale(v: &ModView<impl Sized>, cadence_s: u64, now: Instant) -> bool {
    v.received_at
        .is_some_and(|t| now.saturating_duration_since(t).as_secs() > cadence_s * 3 + 12)
}

/// First line(s) for a module panel when there is no usable data to show.
/// Returns `Some(lines)` when the panel body should be just these lines.
fn status_lines<T>(
    enabled: bool,
    cfg_hint: &str,
    v: &ModView<T>,
    opts: &RenderOpts,
    cadence_s: u64,
) -> (Vec<Line<'static>>, bool) {
    let th = &opts.theme;
    if !enabled {
        return (
            vec![
                Line::from(Span::styled("disabled (nothing is probed or run)", th.dim)),
                Line::from(Span::styled(cfg_hint.to_string(), th.dim)),
            ],
            false,
        );
    }
    let Some(s) = &v.snap else {
        return (vec![Line::from(Span::styled("checking…", th.dim))], false);
    };
    let mut lines = Vec::new();
    let mut has_data = s.data.is_some();
    match &s.status {
        ModStatus::Ok => {
            if stale(v, cadence_s, Instant::now()) {
                lines.push(Line::from(Span::styled("STALE: no recent update", th.bad)));
            }
        }
        ModStatus::Unavailable(m) => {
            lines.push(Line::from(Span::styled(
                format!("unavailable: {m}"),
                th.warn,
            )));
            has_data = false;
        }
        ModStatus::Error(m) => {
            lines.push(Line::from(Span::styled(format!("error: {m}"), th.bad)));
            if has_data {
                lines.push(Line::from(Span::styled(
                    "showing last good data (stale):",
                    th.dim,
                )));
            }
        }
        ModStatus::Pending | ModStatus::Disabled => {
            lines.push(Line::from(Span::styled("checking…", th.dim)));
            has_data = false;
        }
    }
    (lines, has_data)
}

fn title(base: &str, app: &App, pane: DevPane) -> (String, bool) {
    let f = app.dev.pane == pane;
    (
        if f {
            format!("{base}  (R refresh)")
        } else {
            base.to_string()
        },
        f,
    )
}

fn render_families(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts) {
    let th = &opts.theme;
    let (t, focused) = {
        let (t, f) = title("Process families", app, DevPane::Families);
        (
            if f {
                format!("{t}  Enter: open in Processes").replace("  (R refresh)", "")
            } else {
                t
            },
            f,
        )
    };
    let block = panel(t, focused, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return;
    }
    if app.snapshot.as_ref().is_some_and(|s| !s.processes_fresh) {
        frame.render_widget(
            Paragraph::new("Refreshing the process list…").style(th.dim),
            inner,
        );
        return;
    }
    let sel = app.dev.family_table.selected();
    let detail_h = if inner.height >= 14 { 7 } else { 0 };
    let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(detail_h)]).split(inner);
    let rows: Vec<Row> = app
        .dev
        .stats
        .iter()
        .map(|s| {
            let mem = if s.count == 0 {
                "—".to_string()
            } else if s.rss_unknown > 0 {
                format!("{} +{}?", format::bytes(s.rss_total), s.rss_unknown)
            } else {
                format::bytes(s.rss_total)
            };
            let style = if s.count == 0 { th.dim } else { th.text };
            Row::new(vec![
                Cell::from(s.name.clone()),
                Cell::from(s.count.to_string()),
                Cell::from(mem),
                Cell::from(
                    s.cpu_sum
                        .map_or("—".to_string(), |c| format!("{:.0}%", app.cpu_display(c))),
                ),
                Cell::from(if s.flags.is_empty() {
                    String::new()
                } else {
                    "!".to_string()
                }),
            ])
            .style(style)
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(14),
            Constraint::Length(7),
            Constraint::Length(2),
        ],
    )
    .header(Row::new(vec!["FAMILY", "PROCS", "MEM (known)", "CPU~", ""]).style(th.dim))
    .row_highlight_style(th.selected)
    .style(th.text);
    frame.render_stateful_widget(table, parts[0], &mut app.dev.family_table);

    if detail_h > 0 {
        let mut lines = Vec::new();
        if let Some(s) = sel.and_then(|i| app.dev.stats.get(i)) {
            lines.push(Line::from(Span::styled(
                format!("{}: {} process(es)", s.name, s.count),
                th.accent,
            )));
            if let Some((pid, name, rss)) = &s.top {
                lines.push(Line::from(format!(
                    "largest: {name} (PID {pid}) {}",
                    format::bytes(*rss)
                )));
            }
            if !s.names.is_empty() {
                lines.push(Line::from(format!("names: {}", s.names.join(", "))));
            }
            for f in &s.flags {
                lines.push(Line::from(Span::styled(format!("possible: {f}"), th.warn)));
            }
            lines.push(Line::from(Span::styled(
                "MEM sums per-process working sets (shared pages counted repeatedly); CPU~ is approximate.",
                th.dim,
            )));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), parts[1]);
    }
}

fn gpu_field(label: &str, v: Option<String>) -> String {
    format!("{label} {}", v.unwrap_or_else(|| "n/a".into()))
}

fn render_gpu(frame: &mut Frame, area: Rect, app: &App, opts: &RenderOpts) {
    let th = &opts.theme;
    let (t, focused) = title("GPU (NVIDIA)", app, DevPane::Gpu);
    let block = panel(t, focused, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (mut lines, has_data) = status_lines(
        app.config.modules.gpu,
        "enable with [modules] gpu = true",
        &app.dev.gpu,
        opts,
        2,
    );
    if has_data {
        if let Some(d) = app.dev.gpu.snap.as_ref().and_then(|s| s.data.as_ref()) {
            if let Some(drv) = &d.driver {
                lines.push(Line::from(Span::styled(format!("driver {drv}"), th.dim)));
            }
            if d.gpus.is_empty() {
                lines.push(Line::from(Span::styled("NVML reports 0 GPUs", th.warn)));
            }
            let deg = if opts.unicode { "°C" } else { " C" };
            for g in &d.gpus {
                lines.push(Line::from(Span::styled(
                    format!(
                        "[{}] {}",
                        g.index,
                        g.name.as_deref().unwrap_or("unnamed GPU")
                    ),
                    th.accent,
                )));
                let mem_pct = g
                    .mem_used
                    .zip(g.mem_total)
                    .filter(|(_, t)| *t > 0)
                    .map(|(u, t)| u as f64 * 100.0 / t as f64);
                let util = g.util_gpu_percent.map(|u| u as f64);
                lines.push(Line::from(vec![
                    Span::raw("util "),
                    Span::styled(
                        util.map_or("n/a".to_string(), |u| format!("{u:.0}%")),
                        util.map_or(Style::default(), |u| pct_style(th, u)),
                    ),
                    Span::raw(format!(
                        "   {}",
                        gpu_field(
                            "VRAM",
                            g.mem_used.zip(g.mem_total).map(|(u, t)| format!(
                                "{} / {}",
                                format::bytes(u),
                                format::bytes(t)
                            ))
                        )
                    )),
                    Span::raw(mem_pct.map_or(String::new(), |p| {
                        format!(" {} {p:.0}%", bar(p / 100.0, 6, opts.unicode))
                    })),
                ]));
                lines.push(Line::from(format!(
                    "{}   {}   {}",
                    gpu_field("temp", g.temp_c.map(|c| format!("{c}{deg}"))),
                    gpu_field("power", g.power_watts.map(|w| format!("{w:.0} W"))),
                    gpu_field("fan", g.fan_percent.map(|f| format!("{f}%"))),
                )));
            }
            if d.more > 0 {
                lines.push(Line::from(Span::styled(
                    format!("+{} more GPU(s) not shown", d.more),
                    th.dim,
                )));
            }
            lines.push(Line::from(Span::styled(
                "n/a = the driver does not report it. No per-process GPU data.",
                th.dim,
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_docker(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts) {
    let th = &opts.theme;
    let (t, focused) = title("Docker (read-only)", app, DevPane::Docker);
    let block = panel(t, focused, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (head, has_data) = status_lines(
        app.config.modules.docker,
        "opt in with [modules] docker = true",
        &app.dev.docker,
        opts,
        5,
    );
    let snap: Option<std::sync::Arc<ModuleSnapshot<_>>> = app.dev.docker.snap.clone();
    let data = snap
        .as_ref()
        .and_then(|s| s.data.as_ref())
        .filter(|_| has_data);
    let Some(d) = data else {
        frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: true }), inner);
        return;
    };
    let extra_rows = 3 + d.df.as_ref().map_or(1, |r| r.len() + 2);
    let parts = Layout::vertical([
        Constraint::Length(head.len() as u16 + 1),
        Constraint::Min(3),
        Constraint::Length((extra_rows as u16).min(inner.height / 2)),
    ])
    .split(inner);
    let running = d.containers.iter().filter(|c| c.state == "running").count();
    let mut top = head;
    top.push(Line::from(Span::styled(
        format!(
            "engine {}  {} running / {} total",
            d.version.as_deref().unwrap_or("?"),
            running,
            d.containers.len()
        ),
        th.dim,
    )));
    frame.render_widget(Paragraph::new(top), parts[0]);

    let rows: Vec<Row> = d
        .containers
        .iter()
        .map(|c| {
            let st = if c.state == "running" {
                th.text
            } else {
                th.dim
            };
            Row::new(vec![
                Cell::from(c.name.clone()),
                Cell::from(c.project.clone()),
                Cell::from(c.state.clone()),
                Cell::from(
                    c.cpu_percent
                        .map_or("—".to_string(), |v| format!("{v:.1}%")),
                ),
                Cell::from(c.mem_usage.clone().unwrap_or_else(|| "—".into())),
            ])
            .style(st)
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(10),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(7),
            Constraint::Length(18),
        ],
    )
    .header(Row::new(vec!["CONTAINER", "PROJECT", "STATE", "CPU", "MEM (docker)"]).style(th.dim))
    .row_highlight_style(th.selected)
    .style(th.text);
    frame.render_stateful_widget(table, parts[1], &mut app.dev.docker_table);

    let mut foot = Vec::new();
    match &d.df {
        Some(rows) => {
            foot.push(Line::from(Span::styled(
                "system df (logical sizes, not .vhdx savings):",
                th.dim,
            )));
            for r in rows {
                foot.push(Line::from(format!(
                    "{:<12} {:>4} ({} active) {}  reclaimable {}",
                    r.kind, r.total, r.active, r.size, r.reclaimable
                )));
            }
        }
        None => foot.push(Line::from(Span::styled(
            "R: `docker system df` once   l: logs   S/X: stop/restart (needs allow_actions)",
            th.dim,
        ))),
    }
    if let Some(w) = &d.warning {
        foot.push(Line::from(Span::styled(format!("warning: {w}"), th.warn)));
    }
    foot.push(Line::from(Span::styled(
        "container memory is part of host/WSL memory: do not add it to host totals",
        th.dim,
    )));
    frame.render_widget(Paragraph::new(foot).wrap(Wrap { trim: true }), parts[2]);
}

fn render_wsl(frame: &mut Frame, area: Rect, app: &App, opts: &RenderOpts) {
    let th = &opts.theme;
    let (t, focused) = title("WSL (Windows host)", app, DevPane::Wsl);
    let block = panel(t, focused, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (mut lines, has_data) = status_lines(
        app.config.modules.wsl,
        "opt in with [modules] wsl = true (Windows only)",
        &app.dev.wsl,
        opts,
        10,
    );
    if has_data {
        if let Some(d) = app.dev.wsl.snap.as_ref().and_then(|s| s.data.as_ref()) {
            if d.distros.is_empty() {
                lines.push(Line::from(Span::styled(
                    "no distributions installed",
                    th.dim,
                )));
            }
            for x in &d.distros {
                lines.push(Line::from(format!(
                    "{}{}  {}  WSL{}",
                    if x.default { "* " } else { "  " },
                    x.name,
                    x.state,
                    x.version
                )));
            }
            if d.vhdx.is_empty() {
                lines.push(Line::from(Span::styled(
                    "virtual disk sizes: set [wsl] tracked_vhdx_paths (no automatic search)",
                    th.dim,
                )));
            }
            for v in &d.vhdx {
                let sz = match &v.size {
                    crate::modules::wsl::VhdxSize::Bytes(b) => format::bytes(*b),
                    crate::modules::wsl::VhdxSize::NotFound => "not found".into(),
                    crate::modules::wsl::VhdxSize::Denied => "permission denied".into(),
                    crate::modules::wsl::VhdxSize::Error(e) => format!("error: {e}"),
                };
                lines.push(Line::from(format!("{sz}  {}", v.path)));
            }
            if !d.vhdx.is_empty() {
                lines.push(Line::from(Span::styled(
                    "file size on the host disk, not guest free space; systui never compacts it",
                    th.dim,
                )));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

//! Overview: live KPIs, history and top processes. All data comes from the
//! latest snapshot; missing data is shown as unavailable, never as zero.

use super::{cpu_cell, mem_cell};
use crate::app::App;
use crate::domain::{Snapshot, Volume};
use crate::ui::layout::SizeTier;
use crate::ui::widgets::{bar, pct_style, spark};
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table},
    Frame,
};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    let th = &opts.theme;
    let Some(snap) = app.snapshot.clone() else {
        frame.render_widget(
            Paragraph::new("Waiting for the first sample… (no readings yet)").style(th.dim),
            area,
        );
        return;
    };

    let issues: Vec<String> = snap
        .errors()
        .map(|d| {
            format!(
                "{}: {}",
                d.name,
                d.detail.as_deref().unwrap_or("unavailable")
            )
        })
        .collect();
    let mut title = "Resources".to_string();
    let active_alerts = app.active_alerts();
    if !issues.is_empty() {
        title = format!("Resources {} {}", opts.sep(), issues.join("; "));
    }
    if !active_alerts.is_empty() {
        let first = &active_alerts[0];
        title = format!(
            "{title} {} ALERT: {}{}",
            opts.sep(),
            first.key.label(),
            if active_alerts.len() > 1 {
                format!(" (+{} more)", active_alerts.len() - 1)
            } else {
                String::new()
            }
        );
    }

    let kpi_lines = kpi_lines(app, &snap, opts, t);
    let kpi_h = match t {
        SizeTier::Compact => kpi_lines.len() as u16 + 2,
        _ => (kpi_lines.len().div_ceil(2) as u16 + 2).max(5),
    };
    let rows = Layout::vertical([
        Constraint::Length(kpi_h.min(area.height)),
        Constraint::Min(0),
    ])
    .split(area);

    let block = panel(title, false, opts);
    let inner = block.inner(rows[0]);
    frame.render_widget(block, rows[0]);
    if t == SizeTier::Compact {
        frame.render_widget(Paragraph::new(kpi_lines.clone()), inner);
    } else {
        let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);
        let half = kpi_lines.len().div_ceil(2);
        frame.render_widget(Paragraph::new(kpi_lines[..half].to_vec()), cols[0]);
        frame.render_widget(Paragraph::new(kpi_lines[half..].to_vec()), cols[1]);
    }

    if rows[1].height < 3 {
        return;
    }
    if t == SizeTier::Wide {
        let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(rows[1]);
        render_history(frame, cols[0], app, &snap, opts);
        render_top(frame, cols[1], app, &snap, opts);
    } else {
        render_top(frame, rows[1], app, &snap, opts);
    }
}

fn primary_volume(vols: &[Volume]) -> Option<&Volume> {
    vols.iter()
        .find(|v| v.mount == "/" || v.mount.to_ascii_uppercase().starts_with("C:"))
        .or_else(|| vols.iter().max_by_key(|v| v.total))
}

fn kpi_lines(app: &App, s: &Snapshot, opts: &RenderOpts, t: SizeTier) -> Vec<Line<'static>> {
    let th = &opts.theme;
    let sw = if t == SizeTier::Compact { 8 } else { 14 };
    let mut out = Vec::new();

    // CPU
    out.push(match s.cpu.usage {
        Some(u) => Line::from(vec![
            Span::styled("CPU  ", th.dim),
            Span::styled(
                format!("{:>4}", format::percent(Some(u as f64))),
                pct_style(th, u as f64),
            ),
            Span::raw("  "),
            Span::styled(
                spark(&app.hist.cpu.to_vec(), sw, Some(100.0), opts.unicode),
                th.graph,
            ),
            Span::styled(format!("  {} logical", s.cpu.logical), th.dim),
        ]),
        None => Line::from(vec![
            Span::styled("CPU  ", th.dim),
            Span::styled("warming up…", th.dim),
        ]),
    });

    // Memory
    out.push(match &s.memory {
        Some(m) => {
            let pct = m.used as f64 * 100.0 / m.total as f64;
            Line::from(vec![
                Span::styled("RAM  ", th.dim),
                Span::raw(format!(
                    "{} / {}  ",
                    format::bytes(m.used),
                    format::bytes(m.total)
                )),
                Span::styled(bar(pct / 100.0, 8, opts.unicode), pct_style(th, pct)),
                Span::raw(format!(" {pct:.0}%")),
            ])
        }
        None => Line::from(vec![Span::styled("RAM  unavailable", th.bad)]),
    });
    out.push(match &s.memory {
        Some(m) if m.swap_total > 0 => Line::from(vec![
            Span::styled("SWAP ", th.dim),
            Span::raw(format!(
                "{} / {}",
                format::bytes(m.swap_used),
                format::bytes(m.swap_total)
            )),
        ]),
        Some(_) => Line::from(Span::styled("SWAP none configured", th.dim)),
        None => Line::from(Span::styled("SWAP —", th.dim)),
    });

    // Disk
    out.push(match primary_volume(&s.volumes) {
        Some(v) => {
            let pct = v.used_pct();
            Line::from(vec![
                Span::styled("DISK ", th.dim),
                Span::raw(format!("{} ", v.mount)),
                Span::styled(bar(pct / 100.0, 8, opts.unicode), pct_style(th, pct)),
                Span::raw(format!(" {pct:.0}%  {} free", format::bytes(v.available))),
            ])
        }
        None => Line::from(Span::styled(
            "DISK unavailable (no volumes reported)",
            th.dim,
        )),
    });

    // Network
    out.push(match s.net_totals() {
        Some((rx, tx)) => Line::from(vec![
            Span::styled("NET  ", th.dim),
            Span::raw(format!(
                "{} {}  {} {}",
                opts.down(),
                format::rate(rx),
                opts.up(),
                format::rate(tx)
            )),
        ]),
        None => Line::from(Span::styled(
            if s.network.is_empty() {
                "NET  no interfaces"
            } else {
                "NET  warming up…"
            },
            th.dim,
        )),
    });

    // Battery / temperature: only when the platform reports them (absent otherwise)
    if let Some(b) = &s.sensors.battery {
        let pct = b.percent;
        let eta = b.minutes_remaining.map_or(String::new(), |m| {
            format!(", about {} left", format::duration(u64::from(m) * 60))
        });
        out.push(Line::from(vec![
            Span::styled("BAT  ", th.dim),
            Span::styled(
                pct.map_or("—".to_string(), |p| format!("{p}%")),
                pct.map_or(th.text, |p| {
                    if b.state == crate::sensors::BatteryState::Discharging && p <= 15 {
                        th.bad
                    } else if b.state == crate::sensors::BatteryState::Discharging && p <= 30 {
                        th.warn
                    } else {
                        th.text
                    }
                }),
            ),
            Span::raw(format!("  {}{eta}", b.state.label())),
        ]));
    }
    if let Some(h) = s.sensors.hottest() {
        let deg = if opts.unicode { "°C" } else { " C" };
        let thr = s
            .sensors
            .throttle_events
            .filter(|n| *n > 0)
            .map_or(String::new(), |n| format!("  throttled {n}x"));
        out.push(Line::from(vec![
            Span::styled("TEMP ", th.dim),
            Span::styled(
                format!("{:.0}{deg}", h.celsius),
                if h.celsius >= 90.0 {
                    th.bad
                } else if h.celsius >= 80.0 {
                    th.warn
                } else {
                    th.text
                },
            ),
            Span::styled(
                format!("  {} ({} sensors){thr}", h.label, s.sensors.temps.len()),
                th.dim,
            ),
        ]));
    }
    // System
    let host = s
        .host
        .hostname
        .clone()
        .unwrap_or_else(|| "unknown host".into());
    let os = s.host.os.clone().unwrap_or_else(|| "unknown OS".into());
    let up = s
        .host
        .uptime_secs
        .map(format::duration)
        .unwrap_or_else(|| "—".into());
    out.push(Line::from(Span::styled(
        format!("{host} {} {os} {} up {up}", opts.sep(), opts.sep()),
        th.dim,
    )));
    out
}

fn render_history(frame: &mut Frame, area: Rect, app: &App, s: &Snapshot, opts: &RenderOpts) {
    let th = &opts.theme;
    let interval = app.config.ui.refresh_ms as f64 / 1000.0;
    let span_s = app.hist.cpu.len() as f64 * interval;
    let block = panel(
        format!("History (last {})", format::duration(span_s as u64)),
        false,
        opts,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let w = inner.width.saturating_sub(12) as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("CPU  ", th.dim),
            Span::styled(
                spark(&app.hist.cpu.to_vec(), w, Some(100.0), opts.unicode),
                th.graph,
            ),
        ]),
        Line::from(vec![
            Span::styled("RAM  ", th.dim),
            Span::styled(
                spark(&app.hist.mem.to_vec(), w, Some(100.0), opts.unicode),
                th.graph,
            ),
        ]),
    ];
    let rx = app.hist.net_rx.to_vec();
    let tx = app.hist.net_tx.to_vec();
    let net_max = app
        .hist
        .net_rx
        .max()
        .unwrap_or(0.0)
        .max(app.hist.net_tx.max().unwrap_or(0.0));
    lines.push(Line::from(vec![
        Span::styled(format!("{}    ", opts.down()), th.dim),
        Span::styled(spark(&rx, w, Some(net_max), opts.unicode), th.graph),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{}    ", opts.up()), th.dim),
        Span::styled(spark(&tx, w, Some(net_max), opts.unicode), th.graph),
    ]));
    lines.push(Line::from(Span::styled(
        format!("net scale: 0 .. {}", format::rate(net_max)),
        th.dim,
    )));
    lines.push(Line::from(""));
    // per-core, as many as fit
    let room = inner.height as usize - lines.len().min(inner.height as usize);
    for (i, c) in s.cpu.per_core.iter().enumerate().take(room) {
        lines.push(Line::from(vec![
            Span::styled(format!("c{i:<3} "), th.dim),
            Span::styled(
                bar(*c as f64 / 100.0, w.min(20), opts.unicode),
                pct_style(th, *c as f64),
            ),
            Span::raw(format!(" {c:>3.0}%")),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_top(frame: &mut Frame, area: Rect, app: &App, s: &Snapshot, opts: &RenderOpts) {
    let th = &opts.theme;
    let by = if app.overview_by_mem { "MEM" } else { "CPU" };
    let block = panel(format!("Top by {by} (s: switch)"), false, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !s.processes_fresh {
        frame.render_widget(
            Paragraph::new("Refreshing the process list…").style(th.dim),
            inner,
        );
        return;
    }
    let take = (inner.height as usize).saturating_sub(1);
    let mut rows = Vec::new();
    for &i in app.overview_top.iter().take(take) {
        let p = &s.processes[i];
        rows.push(Row::new(vec![
            Cell::from(p.id.pid.to_string()),
            Cell::from(p.name.clone()),
            Cell::from(cpu_cell(p, app)),
            Cell::from(mem_cell(p)),
        ]));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(7),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(vec!["PID", "PROCESS", "CPU", "MEM"]).style(th.dim))
    .style(th.text);
    frame.render_widget(table, inner);
}

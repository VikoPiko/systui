//! Network: per-interface current rates (bytes/s) and cumulative counters.

use crate::app::{App, Focus};
use crate::domain::Snapshot;
use crate::ui::layout::SizeTier;
use crate::ui::widgets::spark;
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
    Frame,
};

fn rate_cell(r: Option<f64>) -> String {
    r.map_or_else(|| "—".to_string(), format::rate)
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    let th = &opts.theme;
    let Some(snap) = app.snapshot.clone() else {
        frame.render_widget(
            Paragraph::new("Waiting for the first sample… (no readings yet)").style(th.dim),
            area,
        );
        return;
    };
    if app.ports.open {
        render_ports(frame, area, app, &snap, opts);
        return;
    }
    if !snap.network_fresh {
        frame.render_widget(
            Paragraph::new("Refreshing network counters…").style(th.dim),
            area,
        );
        return;
    }
    if snap.network.is_empty() {
        frame.render_widget(
            Paragraph::new("No network interfaces were reported by the OS.").style(th.warn),
            area,
        );
        return;
    }
    let compact = t == SizeTier::Compact;
    let (list_area, side) = if compact {
        (area, None)
    } else {
        let c = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);
        (c[0], Some(c[1]))
    };

    let rows_layout =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(list_area);
    let block = panel(
        format!("Interfaces ({})", snap.network.len()),
        app.focus == Focus::Primary,
        opts,
    );
    let inner = block.inner(rows_layout[0]);
    frame.render_widget(block, rows_layout[0]);

    let rows: Vec<Row> = snap
        .network
        .iter()
        .map(|i| {
            let mut c = vec![
                Cell::from(i.name.clone()),
                Cell::from(i.state.clone()),
                Cell::from(rate_cell(i.rx_rate)),
                Cell::from(rate_cell(i.tx_rate)),
            ];
            if !compact {
                c.push(Cell::from(format::bytes(i.rx_total)));
                c.push(Cell::from(format::bytes(i.tx_total)));
            }
            Row::new(c)
        })
        .collect();
    let (widths, header): (Vec<Constraint>, Vec<String>) = if compact {
        (
            vec![
                Constraint::Min(10),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
            vec![
                "NAME".into(),
                "STATE".into(),
                format!("{} /s", opts.down()),
                format!("{} /s", opts.up()),
            ],
        )
    } else {
        (
            vec![
                Constraint::Min(10),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Length(10),
                Constraint::Length(10),
            ],
            vec![
                "NAME".into(),
                "STATE".into(),
                format!("{} RATE", opts.down()),
                format!("{} RATE", opts.up()),
                format!("{} TOTAL", opts.down()),
                format!("{} TOTAL", opts.up()),
            ],
        )
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(th.dim))
        .row_highlight_style(th.selected)
        .style(th.text);
    frame.render_stateful_widget(table, inner, &mut app.net.table);

    let agg = match snap.net_totals() {
        Some((rx, tx)) => format!(
            " Σ non-loopback (may include virtual adapters): {} {}  {} {}",
            opts.down(),
            format::rate(rx),
            opts.up(),
            format::rate(tx)
        ),
        None => " Σ non-loopback: warming up…".to_string(),
    };
    frame.render_widget(Paragraph::new(agg).style(th.dim), rows_layout[1]);

    if let Some(side) = side {
        render_side(frame, side, app, &snap, opts);
    }
}

fn render_side(frame: &mut Frame, area: Rect, app: &App, snap: &Snapshot, opts: &RenderOpts) {
    let th = &opts.theme;
    let Some(name) = app.net.selected.as_deref() else {
        return;
    };
    let block = panel(name.to_string(), app.focus == Focus::Secondary, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(iface) = snap.network.iter().find(|i| i.name == name) else {
        return;
    };
    let w = inner.width.saturating_sub(2) as usize;
    let (rx, tx) = app
        .hist
        .per_iface
        .get(name)
        .map(|(a, b)| (a.to_vec(), b.to_vec()))
        .unwrap_or_default();
    let peak = |v: &[f64]| v.iter().copied().fold(0.0, f64::max);
    let scale = peak(&rx).max(peak(&tx));
    let lines = vec![
        Line::from(Span::styled(format!("state {}", iface.state), th.dim)),
        Line::from(""),
        Line::from(format!(
            "{} now {}   peak {}",
            opts.down(),
            rate_cell(iface.rx_rate),
            format::rate(peak(&rx))
        )),
        Line::from(Span::styled(spark(&rx, w, Some(scale), opts.unicode), th.graph)),
        Line::from(format!(
            "{} now {}   peak {}",
            opts.up(),
            rate_cell(iface.tx_rate),
            format::rate(peak(&tx))
        )),
        Line::from(Span::styled(spark(&tx, w, Some(scale), opts.unicode), th.graph)),
        Line::from(""),
        Line::from(Span::styled(
            "Rates are bytes/second between samples. TOTAL columns are cumulative bytes since the counter started (boot/driver), not a rate.",
            th.dim,
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_ports(frame: &mut Frame, area: Rect, app: &mut App, snap: &Snapshot, opts: &RenderOpts) {
    let th = &opts.theme;
    let title = match (&app.ports.data, app.ports.loading) {
        (Some(Ok(l)), false) => {
            format!("Listening sockets ({})  [L back  R refresh]", l.items.len())
        }
        (_, true) => "Listening sockets (collecting…)".to_string(),
        _ => "Listening sockets".to_string(),
    };
    let block = panel(title, true, opts);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return;
    }
    let parts = Layout::vertical([Constraint::Min(2), Constraint::Length(2)]).split(inner);
    match &app.ports.data {
        None => {
            frame.render_widget(
                Paragraph::new("Collecting listening sockets with the OS's own tool…")
                    .style(th.dim),
                parts[0],
            );
        }
        Some(Err(e)) => {
            frame.render_widget(
                Paragraph::new(format!("unavailable: {e}"))
                    .style(th.warn)
                    .wrap(Wrap { trim: true }),
                parts[0],
            );
        }
        Some(Ok(l)) => {
            let names: std::collections::HashMap<u32, &str> = if snap.processes_fresh {
                snap.processes
                    .iter()
                    .map(|p| (p.id.pid, p.name.as_str()))
                    .collect()
            } else {
                Default::default()
            };
            let rows: Vec<Row> = l
                .items
                .iter()
                .map(|x| {
                    let owner = match x.pid {
                        None => "owner unknown (no permission?)".to_string(),
                        Some(p) => names
                            .get(&p)
                            .map_or_else(|| "…".to_string(), |n| (*n).to_string()),
                    };
                    Row::new(vec![
                        Cell::from(x.proto),
                        Cell::from(x.addr.clone()),
                        Cell::from(x.port.to_string()),
                        Cell::from(x.pid.map_or("—".to_string(), |p| p.to_string())),
                        Cell::from(owner),
                    ])
                })
                .collect();
            let table = Table::new(
                rows,
                [
                    Constraint::Length(5),
                    Constraint::Length(24),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Min(12),
                ],
            )
            .header(Row::new(vec!["PROTO", "ADDRESS", "PORT", "PID", "PROCESS"]).style(th.dim))
            .row_highlight_style(th.selected)
            .style(th.text);
            frame.render_stateful_widget(table, parts[0], &mut app.ports.table);
        }
    }
    let age = app
        .ports
        .collected_at
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .map_or(String::new(), |d| {
            format!("collected {} ago. ", format::duration(d.as_secs()))
        });
    let trunc = matches!(&app.ports.data, Some(Ok(l)) if l.truncated);
    frame.render_widget(
        Paragraph::new(format!(
            "{age}{}Snapshot of sockets the OS reports as listening/bound (not traffic capture); refreshed only when you press R.",
            if trunc { "OUTPUT TRUNCATED: list may be incomplete. " } else { "" }
        ))
        .style(th.dim)
        .wrap(Wrap { trim: true }),
        parts[1],
    );
}

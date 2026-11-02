//! Services & health: configured local checks + alerts. Everything shown is the
//! result of a check the user configured; nothing is probed implicitly.

use crate::app::App;
use crate::health::Status;
use crate::modules::health::CheckState;
use crate::ui::layout::SizeTier;
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, Table, Wrap},
    Frame,
};
use std::time::SystemTime;

fn ago(t: Option<SystemTime>, now: SystemTime) -> String {
    match t.and_then(|t| now.duration_since(t).ok()) {
        Some(d) => format!("{} ago", format::duration(d.as_secs())),
        None => "—".into(),
    }
}

fn until(t: Option<SystemTime>, now: SystemTime) -> String {
    match t {
        Some(t) => match t.duration_since(now) {
            Ok(d) => format!("in {}", format::duration(d.as_secs())),
            Err(_) => "due".into(),
        },
        None => "—".into(),
    }
}

fn status_style(c: &CheckState, stale: bool, opts: &RenderOpts) -> Style {
    let th = &opts.theme;
    if !c.enabled || stale {
        return th.dim;
    }
    match &c.status {
        Status::Healthy => th.good,
        Status::PortOpen => th.warn,
        Status::Unknown => th.dim,
        _ => th.bad,
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    let th = &opts.theme;
    let alerts_h = if area.height >= 16 {
        8.min(area.height / 3)
    } else {
        0
    };
    let ssh_wanted = app.config.modules.ssh;
    let ssh_h = if ssh_wanted && area.height >= 20 {
        (app.ssh_specs.len() as u16 + 4)
            .clamp(6, 10)
            .min(area.height / 3)
    } else {
        0
    };
    let parts = Layout::vertical([
        Constraint::Min(4),
        Constraint::Length(ssh_h),
        Constraint::Length(alerts_h),
    ])
    .split(area);

    // ---- checks
    let block = panel(
        "Health checks (local only)".into(),
        app.focus == crate::app::Focus::Primary || ssh_h == 0,
        opts,
    );
    let inner = block.inner(parts[0]);
    frame.render_widget(block, parts[0]);
    if !app.config.modules.health {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("Health checks are disabled (nothing is probed).", th.dim)),
                Line::from(Span::styled("Enable with [modules] health = true and add [[health.checks]] entries;", th.dim)),
                Line::from(Span::styled("see config/config.example.toml. Targets must be loopback (or private LAN with allow_lan).", th.dim)),
            ])
            .wrap(Wrap { trim: true }),
            inner,
        );
    } else if app.config.health.checks.is_empty() {
        frame.render_widget(
            Paragraph::new("health is enabled but no [[health.checks]] are configured.")
                .style(th.warn),
            inner,
        );
    } else if let Some(h) = app.health.clone() {
        let now = SystemTime::now();
        let compact = t == SizeTier::Compact;
        let rows: Vec<Row> = h
            .checks
            .iter()
            .map(|c| {
                let stale = c.is_stale(now);
                let status = if !c.enabled {
                    "disabled".to_string()
                } else if stale {
                    format!("STALE ({})", c.status.label())
                } else {
                    c.status.label()
                };
                let mut cells = vec![
                    Cell::from(c.name.clone()),
                    Cell::from(status).style(status_style(c, stale, opts)),
                    Cell::from(
                        c.latency
                            .map_or("—".to_string(), |d| format!("{} ms", d.as_millis())),
                    ),
                ];
                if !compact {
                    cells.push(Cell::from(c.target.clone()));
                    cells.push(Cell::from(ago(c.last_success, now)));
                    cells.push(Cell::from(if c.enabled {
                        until(c.next_due, now)
                    } else {
                        "—".into()
                    }));
                    cells.push(Cell::from(c.message.clone().unwrap_or_default()));
                }
                Row::new(cells)
            })
            .collect();
        let (widths, header): (Vec<Constraint>, Vec<&str>) = if compact {
            (
                vec![
                    Constraint::Min(8),
                    Constraint::Length(16),
                    Constraint::Length(8),
                ],
                vec!["NAME", "STATUS", "LATENCY"],
            )
        } else {
            (
                vec![
                    Constraint::Length(18),
                    Constraint::Length(16),
                    Constraint::Length(8),
                    Constraint::Min(18),
                    Constraint::Length(11),
                    Constraint::Length(9),
                    Constraint::Min(12),
                ],
                vec![
                    "NAME", "STATUS", "LATENCY", "TARGET", "LAST OK", "NEXT", "MESSAGE",
                ],
            )
        };
        let table = Table::new(rows, widths)
            .header(Row::new(header).style(th.dim))
            .row_highlight_style(th.selected)
            .style(th.text);
        let split = Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).split(inner);
        frame.render_stateful_widget(table, split[0], &mut app.services_table);
        frame.render_widget(
            Paragraph::new(
                " `port open` = TCP connected only, not proof the app is healthy.  R: check now",
            )
            .style(th.dim),
            split[1],
        );
    } else {
        frame.render_widget(Paragraph::new("starting checks…").style(th.dim), inner);
    }

    if ssh_h > 0 {
        render_ssh(frame, parts[1], app, opts, t);
    }

    // ---- alerts
    if alerts_h == 0 {
        return;
    }
    let active = app.active_alerts();
    let title = if !app.alerts.enabled() {
        "Alerts (off)".to_string()
    } else if active.is_empty() {
        "Alerts: none".to_string()
    } else {
        format!("Alerts: {} active", active.len())
    };
    let block = panel(title, false, opts);
    let inner = block.inner(parts[2]);
    frame.render_widget(block, parts[2]);
    let mut lines: Vec<Line> = Vec::new();
    if !app.alerts.enabled() {
        lines.push(Line::from(Span::styled(
            "disabled; set [alerts] enabled = true (no OS notifications are ever sent)",
            th.dim,
        )));
    }
    for a in &active {
        lines.push(Line::from(Span::styled(
            format!("! {} - {}", a.key.label(), a.detail),
            th.bad,
        )));
    }
    let room = (inner.height as usize).saturating_sub(lines.len());
    for l in app.alert_log.iter().rev().take(room).rev() {
        lines.push(Line::from(Span::styled(l.clone(), th.dim)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_ssh(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    use crate::modules::ssh::SshStatus;
    let th = &opts.theme;
    let block = panel(
        "Remote hosts (SSH, read-only, consent per session)".into(),
        app.focus == crate::app::Focus::Secondary,
        opts,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.ssh_specs.is_empty() {
        frame.render_widget(
            Paragraph::new("SSH is enabled but no valid [[ssh.hosts]] are configured.")
                .style(th.warn),
            inner,
        );
        return;
    }
    let now = SystemTime::now();
    let compact = t == SizeTier::Compact;
    let snap = app.ssh.clone();
    let rows: Vec<Row> = app
        .ssh_specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let st = snap.as_ref().and_then(|s| s.hosts.get(i));
            let (status, style, note) = match st.map(|h| (&h.status, h)) {
                None => ("starting".to_string(), th.dim, String::new()),
                Some((SshStatus::NeedsConsent, _)) => (
                    "needs consent".to_string(),
                    th.warn,
                    "Tab, then C to allow".to_string(),
                ),
                Some((SshStatus::Disabled, _)) => (
                    "disabled".to_string(),
                    th.dim,
                    "disabled in config".to_string(),
                ),
                Some((SshStatus::Checking, _)) => ("checking…".to_string(), th.dim, String::new()),
                Some((SshStatus::Ok, h)) if h.is_stale(now) => {
                    ("STALE".to_string(), th.bad, "no recent update".to_string())
                }
                Some((SshStatus::Ok, _)) => ("ok".to_string(), th.good, String::new()),
                Some((SshStatus::Unreachable(m), _)) => {
                    ("unreachable".to_string(), th.bad, m.clone())
                }
                Some((SshStatus::Error(m), _)) => ("error".to_string(), th.bad, m.clone()),
            };
            let d = st.and_then(|h| h.data.as_ref());
            let load = d.and_then(|d| d.load).map_or("—".to_string(), |(a, b, c)| {
                format!("{a:.2} {b:.2} {c:.2}")
            });
            let mem = d
                .and_then(|d| d.mem_total.zip(d.mem_available))
                .filter(|(t, _)| *t > 0)
                .map_or("—".to_string(), |(t, a)| {
                    format!(
                        "{:.0}% of {}",
                        t.saturating_sub(a) as f64 * 100.0 / t as f64,
                        format::bytes(t)
                    )
                });
            let disk = d
                .and_then(|d| d.disk_root)
                .map_or("—".to_string(), |(t, a)| {
                    format!(
                        "{} free ({:.0}%)",
                        format::bytes(a),
                        t.saturating_sub(a) as f64 * 100.0 / t as f64
                    )
                });
            let up = d
                .and_then(|d| d.uptime_secs)
                .map_or("—".to_string(), format::duration);
            let units = d.map_or(String::new(), |d| {
                d.units
                    .iter()
                    .map(|(u, s)| format!("{u}:{s}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            let mut cells = vec![
                Cell::from(spec.name.clone()),
                Cell::from(status).style(style),
                Cell::from(load),
            ];
            if !compact {
                cells.insert(1, Cell::from(spec.dest()));
                cells.push(Cell::from(mem));
                cells.push(Cell::from(disk));
                cells.push(Cell::from(up));
            }
            cells.push(Cell::from(if note.is_empty() { units } else { note }));
            Row::new(cells)
        })
        .collect();
    let (widths, header): (Vec<Constraint>, Vec<&str>) = if compact {
        (
            vec![
                Constraint::Length(12),
                Constraint::Length(14),
                Constraint::Length(16),
                Constraint::Min(8),
            ],
            vec!["NAME", "STATUS", "LOAD", "NOTE"],
        )
    } else {
        (
            vec![
                Constraint::Length(12),
                Constraint::Length(22),
                Constraint::Length(13),
                Constraint::Length(16),
                Constraint::Length(17),
                Constraint::Length(22),
                Constraint::Length(8),
                Constraint::Min(14),
            ],
            vec![
                "NAME",
                "DESTINATION",
                "STATUS",
                "LOAD 1/5/15",
                "MEM used",
                "ROOT DISK",
                "UP",
                "UNITS / NOTE",
            ],
        )
    };
    let split = Layout::vertical([Constraint::Min(2), Constraint::Length(1)]).split(inner);
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(th.dim))
        .row_highlight_style(th.selected)
        .style(th.text);
    frame.render_stateful_widget(table, split[0], &mut app.ssh_table);
    frame.render_widget(
        Paragraph::new(" Nothing is contacted until you consent (C). Read-only script; no forwarding; never saved.").style(th.dim),
        split[1],
    );
}

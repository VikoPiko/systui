//! Central render router. Pure function of state; never blocks or collects.

pub mod layout;
pub mod theme;
pub mod views;
pub mod widgets;

use crate::actions::process::Risk;
use crate::app::{App, KillDialog, Mode, Tab};
use layout::{tier, SizeTier, MIN_HEIGHT, MIN_WIDTH};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use std::time::Instant;
use theme::Theme;

const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Rendering options derived from config/CLI/environment.
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    pub theme: Theme,
    pub unicode: bool,
}

impl RenderOpts {
    pub fn border_set(&self) -> border::Set<'static> {
        if self.unicode {
            border::ROUNDED
        } else {
            ASCII_BORDER
        }
    }
    pub fn up(&self) -> &'static str {
        if self.unicode {
            "↑"
        } else {
            "U"
        }
    }
    pub fn down(&self) -> &'static str {
        if self.unicode {
            "↓"
        } else {
            "D"
        }
    }
    pub fn sep(&self) -> &'static str {
        if self.unicode {
            "·"
        } else {
            "|"
        }
    }
}

/// A bordered panel; the focused one gets the accent border.
pub fn panel<'a>(title: String, focused: bool, opts: &RenderOpts) -> Block<'a> {
    let th = &opts.theme;
    Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(if focused { th.accent } else { th.border })
        .title(Span::styled(format!(" {title} "), th.text))
}

pub fn render(frame: &mut Frame, app: &mut App, opts: &RenderOpts) {
    let area = frame.area();
    let t = tier(area);
    if t == SizeTier::TooSmall {
        render_too_small(frame, area, opts);
        return;
    }
    let th = opts.theme;
    let now = Instant::now();
    let (status, status_style) = status_label(app, now, &th);
    let title = Line::from(vec![
        Span::styled(" systui ", th.accent),
        Span::styled(format!("v{} ", env!("CARGO_PKG_VERSION")), th.dim),
    ]);
    let alerts = app.active_alerts().len();
    let mut right_spans = Vec::new();
    if app.is_recording() {
        right_spans.push(Span::styled(" REC ", th.bad));
    }
    if alerts > 0 {
        right_spans.push(Span::styled(format!(" ALERT {alerts} "), th.bad));
    }
    right_spans.push(Span::styled(format!(" {status} "), status_style));
    let right = Line::from(right_spans).right_aligned();
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.border)
        .title(title)
        .title(right);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let rows = Layout::vertical([
        Constraint::Length(1), // tabs
        Constraint::Length(1), // separator
        Constraint::Min(1),    // body
        Constraint::Length(1), // footer
    ])
    .split(inner);

    frame.render_widget(Paragraph::new(tab_line(app, opts, t)), rows[0]);
    frame.render_widget(
        Paragraph::new((if opts.unicode { "─" } else { "-" }).repeat(rows[1].width as usize))
            .style(th.border),
        rows[1],
    );
    match app.tab {
        Tab::Overview => views::overview::render(frame, rows[2], app, opts, t),
        Tab::Processes => views::processes::render(frame, rows[2], app, opts, t),
        Tab::Network => views::network::render(frame, rows[2], app, opts, t),
        Tab::Storage => views::storage::render(frame, rows[2], app, opts, t),
        Tab::Developer => views::developer::render(frame, rows[2], app, opts, t),
        Tab::Services => views::services::render(frame, rows[2], app, opts, t),
    }
    frame.render_widget(
        Paragraph::new(footer_line(app, opts, rows[3].width)),
        rows[3],
    );

    match &app.mode {
        Mode::Help => render_help(frame, area, opts),
        Mode::Kill(d) => render_kill(frame, area, d, opts),
        Mode::Palette(p) => render_palette(frame, area, p, opts),
        Mode::DockerLogs(v) => render_docker_logs(frame, area, v, opts),
        Mode::DockerConfirm(d) => render_docker_confirm(frame, area, d, opts),
        Mode::SshConsent(d) => render_ssh_consent(frame, area, d, opts),
        Mode::Changes(i) => {
            let i = *i;
            render_changes(frame, area, app, i, opts)
        }
        _ => {}
    }
}

fn status_label(app: &App, now: Instant, th: &Theme) -> (String, ratatui::style::Style) {
    if app.paused {
        return ("PAUSED".into(), th.warn);
    }
    match (&app.snapshot, app.sample_age(now)) {
        (None, _) | (_, None) => ("waiting for first sample".into(), th.dim),
        (Some(_), Some(age)) if app.is_stale(now) => (format!("STALE {}s", age.as_secs()), th.bad),
        (Some(s), Some(age)) => {
            let warm = s.cpu.usage.is_none();
            let dot = if warm { "warming up" } else { "live" };
            (format!("{dot} {:.1}s", age.as_secs_f32()), th.dim)
        }
    }
}

fn tab_line(app: &App, opts: &RenderOpts, t: SizeTier) -> Line<'static> {
    let th = &opts.theme;
    let mut spans = vec![Span::raw(" ")];
    for tab in Tab::ALL {
        let label = if t == SizeTier::Compact && tab != app.tab {
            format!("{}", tab.index() + 1)
        } else {
            format!("{} {}", tab.index() + 1, tab.title())
        };
        let style = if tab == app.tab { th.selected } else { th.text };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

fn footer_line(app: &App, opts: &RenderOpts, width: u16) -> Line<'static> {
    let th = &opts.theme;
    if let Some(n) = &app.notice {
        return Line::from(Span::styled(format!(" {n}"), th.warn));
    }
    let (full, short): (&str, &str) = match (&app.mode, app.tab) {
        (Mode::Filter, _) => (
            " type to filter   Enter keep   Esc clear   Backspace delete",
            " Enter keep  Esc clear",
        ),
        (Mode::Changes(_), _) => (
            " [ ] or Left/Right change window   w or Esc close",
            " [ ] window  Esc close",
        ),
        (Mode::DockerLogs(_), _) => (
            " j/k PgUp/PgDn g/G scroll   r reload   l or Esc close",
            " r reload  Esc close",
        ),
        (Mode::SshConsent(_), _) => (
            " type the host name + Enter to consent   Esc cancel (nothing is contacted)",
            " Esc cancel",
        ),
        (Mode::DockerConfirm(_), _) => (
            " type the container name + Enter to confirm   Esc cancel",
            " Esc cancel",
        ),
        (Mode::Palette(_), _) => (
            " type to search commands   Up/Down select   Enter run   Esc close",
            " Enter run  Esc close",
        ),
        (Mode::Kill(_), _) => (
            " type PID + Enter confirm   f force (if offered)   Esc cancel",
            " Esc cancel",
        ),
        (_, Tab::Processes) => (
            " j/k move  / filter  s sort  t tree  i I/O  Enter details  K terminate  w changes  ? help  q quit",
            " / filter  s sort  Enter info  ? help  q quit",
        ),
        (_, Tab::Overview) => (
            " 1-6 view  s CPU/MEM  w changes  Space pause  Ctrl+K cmds  ? help  q quit",
            " ? help  q quit",
        ),
        (_, Tab::Services) => (
            " j/k select  Tab checks/SSH hosts  C consent  R check now  ? help  q quit",
            " R check now  ? help  q quit",
        ),
        (_, Tab::Developer) => (
            " Tab panel  j/k select  Enter open family  l logs  S stop  X restart  R refresh  ? help  q quit",
            " Tab panel  R refresh  ? help  q quit",
        ),
        (_, Tab::Network | Tab::Storage) => (
            " 1-6 view  j/k select  s sort  Space pause  Ctrl+K cmds  ? help  q quit",
            " ? help  q quit",
        ),
    };
    let s = if width as usize >= full.chars().count() {
        full
    } else {
        short
    };
    Line::from(Span::styled(s.to_string(), th.dim))
}

fn render_too_small(frame: &mut Frame, area: Rect, opts: &RenderOpts) {
    let text = format!(
        "Terminal too small ({}x{}).\nNeed at least {}x{}.\nPress q to quit.",
        area.width, area.height, MIN_WIDTH, MIN_HEIGHT
    );
    frame.render_widget(
        Paragraph::new(text)
            .style(opts.theme.warn)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn render_help(frame: &mut Frame, area: Rect, opts: &RenderOpts) {
    let th = &opts.theme;
    let r = centered(area, 76, 29);
    frame.render_widget(Clear, r);
    let lines = [
        "Ctrl+K     command palette (searches built-in commands only)",
        "w          what changed? (new/exited processes, CPU and memory deltas)",
        "Ctrl+R     start/stop session recording (needs [recording] enabled)",
        "1-6        switch view",
        "Tab        cycle focus between panes",
        "Space      pause / resume the display (sampling continues)",
        "r          request an immediate refresh",
        "?          toggle this help      Esc  close overlay",
        "q, Ctrl+C  quit",
        "",
        "Processes",
        "j/k, arrows, PgUp/PgDn, Home/End   move selection",
        "/          filter by name or PID (Enter keep, Esc clear)",
        "s / S      cycle sort key / reverse order",
        "Enter      open details (Esc closes)   e  reveal command line",
        "p / c      jump to parent / first child",
        "t          tree view (Left/Right collapse/expand)   i  per-process I/O columns",
        "x          export selected process (sanitized JSON, no cmdline)",
        "K          terminate (asks you to type the PID to confirm)",
        "",
        "Overview: s switches top list CPU/MEM. Storage: s cycles sort.",
        "Dev (5): Tab cycles panels, Enter opens a family in Processes, R refreshes.",
    ];
    let body: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(Span::styled(*l, th.text)))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.accent)
        .title(" Help ");
    frame.render_widget(Paragraph::new(body).block(block), r);
}

fn render_kill(frame: &mut Frame, area: Rect, d: &KillDialog, opts: &RenderOpts) {
    let th = &opts.theme;
    let r = centered(area, 72, 16);
    frame.render_widget(Clear, r);
    let mut lines = vec![
        Line::from(Span::styled(
            if d.force {
                "FORCE KILL process?"
            } else {
                "Terminate process?"
            },
            th.bad,
        )),
        Line::from(""),
        Line::from(format!("  Name: {}", d.name)),
        Line::from(format!("  PID:  {}", d.target.pid)),
        Line::from(format!(
            "  Path: {}",
            d.exe
                .as_deref()
                .unwrap_or("unavailable (access restricted)")
        )),
    ];
    if let Risk::Caution(why) = &d.risk {
        lines.push(Line::from(Span::styled(
            format!("  WARNING: {why}"),
            th.warn,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(if d.force {
        "  Forced: the process cannot clean up and unsaved data is lost."
    } else {
        "  Graceful: asks the process to exit (SIGTERM); it may ignore this."
    }));
    if !d.can_graceful {
        lines.push(Line::from(Span::styled(
            "  This platform has no graceful option; this ends the process immediately.",
            th.dim,
        )));
    } else if !d.force {
        lines.push(Line::from(Span::styled(
            "  Press f to switch to force kill (needs a fresh confirmation).",
            th.dim,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(if d.require_typing {
        format!(
            "  Type the PID ({}) and press Enter: {}_",
            d.target.pid, d.typed
        )
    } else {
        "  Press y to confirm, Esc to cancel.".to_string()
    }));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.bad)
        .title(" Confirm ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        r,
    );
}

fn render_palette(
    frame: &mut Frame,
    area: Rect,
    p: &crate::palette::PaletteState,
    opts: &RenderOpts,
) {
    let th = &opts.theme;
    let results = crate::palette::search(&p.query);
    let h = (results.len() as u16 + 4).clamp(6, 22);
    let r = centered(area, 64, h);
    frame.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.accent)
        .title(" Commands ");
    let inner = block.inner(r);
    frame.render_widget(block, r);
    if inner.height < 2 {
        return;
    }
    let mut lines = vec![
        Line::from(vec![
            Span::styled("> ", th.accent),
            Span::raw(format!("{}_", p.query)),
        ]),
        Line::from(""),
    ];
    let room = inner.height as usize - 2;
    // Keep the selected row visible in long lists.
    let first = p.selected.saturating_sub(room.saturating_sub(1));
    if results.is_empty() {
        lines.push(Line::from(Span::styled("no matching command", th.dim)));
    }
    for (i, s) in results.iter().enumerate().skip(first).take(room) {
        let w = inner.width as usize;
        let keys = format!("[{}]", s.keys);
        let pad = w.saturating_sub(s.title.chars().count() + keys.chars().count() + 2);
        let text = format!(" {}{}{} ", s.title, " ".repeat(pad), keys);
        lines.push(Line::from(Span::styled(
            text,
            if i == p.selected {
                th.selected
            } else {
                th.text
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_changes(frame: &mut Frame, area: Rect, app: &mut App, idx: usize, opts: &RenderOpts) {
    use crate::changes::WINDOWS;
    use crate::util::format;
    let th = &opts.theme;
    let r = centered(area, 86, 30);
    frame.render_widget(Clear, r);
    let want = WINDOWS[idx.min(WINDOWS.len() - 1)];
    let title = format!(
        " What changed? - compare with ~{} ago ",
        format::duration(want as u64)
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.accent)
        .title(title);
    let inner = block.inner(r);
    frame.render_widget(block, r);

    let age = app.changes_age(Instant::now());
    let mut lines: Vec<Line> = Vec::new();
    match app.changes.compare(want) {
        None => {
            lines.push(Line::from(Span::styled(
                "Collecting history: needs two snapshots at least 5 s apart (only taken while a view that lists processes is open).",
                th.warn,
            )));
        }
        Some(rep) => {
            lines.push(Line::from(Span::styled(
                format!(
                    "Compared with a snapshot {} older than the newest (newest taken {} ago). Session-only, in memory.",
                    format::duration(rep.span_secs as u64),
                    age.map_or("?".to_string(), |a| format::duration(a as u64)),
                ),
                th.dim,
            )));
            let sec = |lines: &mut Vec<Line>, head: String, rows: Vec<String>, style| {
                lines.push(Line::from(Span::styled(head, style)));
                if rows.is_empty() {
                    lines.push(Line::from(Span::styled("   none", th.dim)));
                }
                for l in rows {
                    lines.push(Line::from(format!("   {l}")));
                }
            };
            let app_row = |a: &crate::changes::Appeared| {
                format!(
                    "{:<7} {:<28} {}",
                    a.pid,
                    a.name,
                    a.rss.map_or("—".to_string(), format::bytes)
                )
            };
            sec(
                &mut lines,
                format!("Started ({})", rep.started_total),
                rep.started.iter().map(app_row).collect(),
                th.good,
            );
            sec(
                &mut lines,
                format!("Exited ({})", rep.exited_total),
                rep.exited.iter().map(app_row).collect(),
                th.warn,
            );
            let cpu_row = |d: &crate::changes::Delta| {
                format!(
                    "{:<7} {:<28} {:.0}% -> {:.0}%  (Δ {:+.0})",
                    d.pid,
                    d.name,
                    d.from,
                    d.to,
                    d.to - d.from
                )
            };
            let mem_row = |d: &crate::changes::Delta| {
                format!(
                    "{:<7} {:<28} {} -> {}",
                    d.pid,
                    d.name,
                    format::bytes(d.from as u64),
                    format::bytes(d.to as u64)
                )
            };
            sec(
                &mut lines,
                "CPU up (percent of one core)".into(),
                rep.cpu_up.iter().map(cpu_row).collect(),
                th.text,
            );
            sec(
                &mut lines,
                "CPU down".into(),
                rep.cpu_down.iter().map(cpu_row).collect(),
                th.text,
            );
            sec(
                &mut lines,
                "Memory up".into(),
                rep.mem_up.iter().map(mem_row).collect(),
                th.text,
            );
            sec(
                &mut lines,
                "Memory down".into(),
                rep.mem_down.iter().map(mem_row).collect(),
                th.text,
            );
            lines.push(Line::from(Span::styled(
                "Processes that started and ended between two snapshots are not seen. Unreadable (denied) values are skipped, never treated as 0.",
                th.dim,
            )));
        }
    }
    lines.truncate(inner.height as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_docker_logs(frame: &mut Frame, area: Rect, v: &crate::app::LogsView, opts: &RenderOpts) {
    let th = &opts.theme;
    let r = centered(
        area,
        area.width.saturating_sub(4).min(150),
        area.height.saturating_sub(2).min(40),
    );
    frame.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.accent)
        .title(format!(" Logs: {} ", v.name));
    let inner = block.inner(r);
    frame.render_widget(block, r);
    if inner.height < 3 {
        return;
    }
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let h = parts[0].height as usize;
    let mut lines: Vec<Line> = Vec::new();
    if v.loading {
        lines.push(Line::from(Span::styled("loading…", th.dim)));
    }
    if let Some(e) = &v.error {
        lines.push(Line::from(Span::styled(format!("error: {e}"), th.bad)));
    }
    if !v.loading && v.error.is_none() && v.lines.is_empty() {
        lines.push(Line::from(Span::styled("(no log output)", th.dim)));
    }
    // window ending at `scroll` (usize::MAX = newest)
    let last = v.lines.len().saturating_sub(1);
    let end = v.scroll.min(last) + 1;
    let start = end.saturating_sub(h.saturating_sub(lines.len()));
    for l in v.lines.iter().take(end).skip(start) {
        lines.push(Line::from(l.clone()));
    }
    frame.render_widget(Paragraph::new(lines), parts[0]);
    frame.render_widget(
        Paragraph::new(format!(
            "{} line(s){}. Logs can contain secrets: shown here only, never saved or exported.",
            v.lines.len(),
            if v.truncated {
                ", output truncated"
            } else {
                ""
            }
        ))
        .style(th.warn),
        parts[1],
    );
}

fn render_docker_confirm(
    frame: &mut Frame,
    area: Rect,
    d: &crate::app::DockerDialog,
    opts: &RenderOpts,
) {
    let th = &opts.theme;
    let r = centered(area, 76, 14);
    frame.render_widget(Clear, r);
    let verb = d.action.verb();
    let lines = vec![
        Line::from(Span::styled(format!("{} container?", verb.to_uppercase()), th.bad)),
        Line::from(""),
        Line::from(format!("  Name:  {}", d.name)),
        Line::from(format!("  Image: {}", d.image)),
        Line::from(format!("  Id:    {}", d.id)),
        Line::from(format!("  State: {}", if d.running { "running" } else { "not running" })),
        Line::from(""),
        Line::from(Span::styled(
            match d.action {
                crate::actions::docker::DockerAction::Stop => "  Sends a stop request (10 s grace period, then killed). Data in volumes is kept.",
                crate::actions::docker::DockerAction::Restart => "  Restarts the container (10 s grace period). In-memory state is lost.",
            },
            th.dim,
        )),
        Line::from(Span::styled("  The id and name are re-checked right before acting.", th.dim)),
        Line::from(""),
        Line::from(format!("  Type the container name and press Enter: {}_", d.typed)),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.bad)
        .title(" Confirm ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        r,
    );
}

fn render_ssh_consent(
    frame: &mut Frame,
    area: Rect,
    d: &crate::app::ConsentDialog,
    opts: &RenderOpts,
) {
    let th = &opts.theme;
    let r = centered(
        area,
        area.width.saturating_sub(4).min(100),
        20.min(area.height),
    );
    frame.render_widget(Clear, r);
    let lines = vec![
        Line::from(Span::styled("Allow systui to contact this host over SSH?", th.bad)),
        Line::from(""),
        Line::from(format!("  Host: {}   ({})", d.name, d.dest)),
        Line::from(""),
        Line::from("  It will run the system `ssh` client and execute this FIXED read-only script there:"),
        Line::from(Span::styled(format!("  {}", d.command), th.dim)),
        Line::from(""),
        Line::from("  - Uses your existing ssh keys / agent / ~/.ssh/config. systui stores no credentials and never asks for a password."),
        Line::from("  - The host key must already be trusted (StrictHostKeyChecking=yes); systui never accepts a new key."),
        Line::from("  - No forwarding. Polls only while the Services tab is open."),
        Line::from("  - Consent lasts until you quit systui; it is never saved."),
        Line::from(""),
        Line::from(format!("  Type the host name ({}) and press Enter: {}_", d.name, d.typed)),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(opts.border_set())
        .border_style(th.bad)
        .title(" Consent: remote read-only check ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        r,
    );
}

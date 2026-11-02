//! Storage: capacity of mounted volumes (not disk activity).

use crate::app::{App, StorageSort};
use crate::domain::Volume;
use crate::ui::layout::SizeTier;
use crate::ui::widgets::{bar, pct_style};
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    widgets::{Cell, Paragraph, Row, Table},
    Frame,
};

pub fn sorted(vols: &[Volume], by: StorageSort) -> Vec<&Volume> {
    let mut v: Vec<&Volume> = vols.iter().collect();
    match by {
        StorageSort::Used => v.sort_by(|a, b| {
            b.used_pct()
                .partial_cmp(&a.used_pct())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.mount.cmp(&b.mount))
        }),
        StorageSort::Free => {
            v.sort_by(|a, b| a.available.cmp(&b.available).then(a.mount.cmp(&b.mount)))
        }
        StorageSort::Mount => v.sort_by(|a, b| a.mount.cmp(&b.mount)),
    }
    v
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
    if snap.volumes.is_empty() {
        let note = snap
            .diagnostics
            .iter()
            .find(|d| d.name == "storage")
            .and_then(|d| d.detail.clone())
            .unwrap_or_else(|| "No mounted volumes were reported.".into());
        frame.render_widget(Paragraph::new(note).style(th.warn), area);
        return;
    }
    let label = match app.storage.sort {
        StorageSort::Used => "used % (high first)",
        StorageSort::Free => "free space (low first)",
        StorageSort::Mount => "mount point",
    };
    let parts = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);
    let block = panel(
        format!(
            "Volumes ({})  sort: {label}  [capacity, not I/O activity]",
            snap.volumes.len()
        ),
        true,
        opts,
    );
    let inner = block.inner(parts[0]);
    frame.render_widget(block, parts[0]);
    let compact = t == SizeTier::Compact;
    let rows: Vec<Row> = sorted(&snap.volumes, app.storage.sort)
        .into_iter()
        .map(|v| {
            let pct = v.used_pct();
            let mut c = vec![Cell::from(v.mount.clone()), Cell::from(v.fs.clone())];
            c.push(Cell::from(format::bytes(v.total)));
            if !compact {
                c.push(Cell::from(format::bytes(v.used())));
            }
            c.push(Cell::from(format::bytes(v.available)));
            c.push(Cell::from(format!("{pct:.0}%")).style(pct_style(th, pct)));
            if !compact {
                c.push(Cell::from(bar(pct / 100.0, 10, opts.unicode)).style(pct_style(th, pct)));
            }
            Row::new(c)
        })
        .collect();
    let (widths, header): (Vec<Constraint>, Vec<&str>) = if compact {
        (
            vec![
                Constraint::Min(8),
                Constraint::Length(7),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(5),
            ],
            vec!["MOUNT", "FS", "TOTAL", "FREE", "USE"],
        )
    } else {
        (
            vec![
                Constraint::Min(8),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(5),
                Constraint::Length(10),
            ],
            vec!["MOUNT", "FS", "TOTAL", "USED", "FREE", "USE", ""],
        )
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(th.dim))
        .row_highlight_style(th.selected)
        .style(th.text);
    frame.render_stateful_widget(table, inner, &mut app.storage.table);
    let note = snap
        .diagnostics
        .iter()
        .find(|d| d.name == "storage")
        .and_then(|d| d.detail.clone())
        .unwrap_or_default();
    frame.render_widget(Paragraph::new(format!(" {note}")).style(th.dim), parts[1]);
}

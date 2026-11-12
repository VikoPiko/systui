//! AI tab: what local inference servers report (loaded models, memory split,
//! throughput where exposed), plus GPU/host pressure and the engine processes.
//! Every number states its source; anything a server does not expose is `—`/absent.

use crate::app::{App, ModView};
use crate::config::AiKind;
use crate::modules::ai::{self, LoadedModel, ServerInfo, ServerReport, ServerState};
use crate::modules::ModStatus;
use crate::ui::layout::SizeTier;
use crate::ui::widgets::{bar, pct_style, spark};
use crate::ui::{panel, RenderOpts};
use crate::util::format;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A model that stays loaded "forever" (Ollama `keep_alive = -1`) reports a far-future expiry.
const FOREVER_SECS: i64 = 365 * 86_400;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App, opts: &RenderOpts, t: SizeTier) {
    let block = panel(
        "Local AI (read-only, loopback only)".to_string(),
        true,
        opts,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = build_lines(app, opts, inner.width as usize, t == SizeTier::Compact);
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    app.ai.scroll = app.ai.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.ai.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

fn trunc(s: &str, max: usize, unicode: bool) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let ell = if unicode { "…" } else { "..." };
    let keep = max.saturating_sub(ell.chars().count());
    format!("{}{ell}", s.chars().take(keep).collect::<String>())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn stale(v: &ModView<impl Sized>) -> bool {
    v.received_at
        .is_some_and(|t| Instant::now().saturating_duration_since(t).as_secs() > 2 * 3 + 12)
}

fn section(title: &str, opts: &RenderOpts) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), opts.theme.accent))
}

pub fn build_lines(
    app: &App,
    opts: &RenderOpts,
    width: usize,
    compact: bool,
) -> Vec<Line<'static>> {
    let th = &opts.theme;
    let mut out: Vec<Line<'static>> = Vec::new();
    let w = width.max(20);
    let mut inst: Vec<Line<'static>> = Vec::new();

    if !app.config.modules.ai {
        out.push(Line::from(Span::styled(
            "disabled (nothing is probed). Enable with [modules] ai = true",
            th.dim,
        )));
        return out;
    }

    // ---- servers -------------------------------------------------------------
    let v = &app.ai.view;
    match &v.snap {
        None => out.push(Line::from(Span::styled("checking local servers…", th.dim))),
        Some(s) => {
            match &s.status {
                ModStatus::Ok if stale(v) => {
                    out.push(Line::from(Span::styled("STALE: no recent update", th.bad)))
                }
                ModStatus::Ok | ModStatus::Pending | ModStatus::Disabled => {}
                ModStatus::Unavailable(m) => out.push(Line::from(Span::styled(
                    format!("unavailable: {m}"),
                    th.warn,
                ))),
                ModStatus::Error(m) => out.push(Line::from(Span::styled(
                    format!("error: {m} (showing last good data)"),
                    th.bad,
                ))),
            }
            if let Some(d) = &s.data {
                let any_up = d
                    .servers
                    .iter()
                    .any(|r| matches!(r.state, ServerState::Up(_)));
                for r in &d.servers {
                    server_lines(r, opts, w, compact, &mut out, &mut inst);
                }
                if !any_up && !d.servers.is_empty() {
                    out.push(Line::from(Span::styled(
                        "No local AI server answered. Start Ollama / LM Studio / llama-server, or add [[ai.endpoints]] to the config.",
                        th.warn,
                    )));
                }
            }
        }
    }

    // ---- GPU -----------------------------------------------------------------
    out.push(Line::raw(""));
    out.push(section("GPU memory and load (NVIDIA, via NVML)", opts));
    gpu_lines(app, opts, w, &mut out);

    // ---- host + engine processes ------------------------------------------------
    out.push(Line::raw(""));
    out.push(section("Host and engine processes", opts));
    match app.snapshot.as_ref() {
        None => out.push(Line::from(Span::styled(
            "waiting for the first sample…",
            th.dim,
        ))),
        Some(s) => {
            match s.memory {
                Some(m) if m.total > 0 => {
                    let pct = m.used as f64 * 100.0 / m.total as f64;
                    out.push(Line::from(vec![
                        Span::raw(format!(
                            "host RAM {} / {}  ",
                            format::bytes(m.used),
                            format::bytes(m.total)
                        )),
                        Span::styled(format!("{pct:.0}%"), pct_style(th, pct)),
                        Span::raw(format!(" {}", bar(pct / 100.0, 10, opts.unicode))),
                    ]));
                }
                _ => out.push(Line::from("host RAM —")),
            }
            if !s.processes_fresh {
                out.push(Line::from(Span::styled(
                    "process list not refreshed yet",
                    th.dim,
                )));
            } else {
                let eng = ai::summarize_processes(&s.processes);
                if eng.is_empty() {
                    out.push(Line::from(Span::styled(
                        "no known engine process running (python-hosted engines such as vllm serve show as `python` and are not detected)",
                        th.dim,
                    )));
                }
                for e in eng {
                    let cpu = e.cpu_sum.map_or("—".to_string(), |c| format!("{c:.0}%"));
                    let unk = if e.rss_unknown > 0 {
                        format!(" (+{} unreadable)", e.rss_unknown)
                    } else {
                        String::new()
                    };
                    out.push(Line::from(format!(
                        "{:<10} {:>2} proc  RAM {}{unk}  CPU {cpu} of one core",
                        e.engine,
                        e.count,
                        format::bytes(e.rss_total),
                    )));
                }
            }
        }
    }
    out.push(Line::from(Span::styled(
        "Server-reported model sizes and host/GPU memory are different measurements: they are shown side by side, never summed.",
        th.dim,
    )));
    // Installed-model lists can be long, so they come last.
    if !inst.is_empty() {
        out.push(Line::raw(""));
        out.push(section(
            "Installed models (as each server reports them)",
            opts,
        ));
        out.extend(inst);
    }
    out
}

fn gpu_lines(app: &App, opts: &RenderOpts, w: usize, out: &mut Vec<Line<'static>>) {
    let th = &opts.theme;
    if !app.config.modules.gpu {
        out.push(Line::from(Span::styled(
            "disabled: [modules] gpu = false",
            th.dim,
        )));
        return;
    }
    let v = &app.dev.gpu;
    let Some(s) = &v.snap else {
        out.push(Line::from(Span::styled("checking…", th.dim)));
        return;
    };
    match &s.status {
        ModStatus::Unavailable(m) => {
            out.push(Line::from(Span::styled(
                format!("unavailable: {m}"),
                th.warn,
            )));
            return;
        }
        ModStatus::Error(m) => out.push(Line::from(Span::styled(format!("error: {m}"), th.bad))),
        ModStatus::Ok if stale(v) => {
            out.push(Line::from(Span::styled("STALE: no recent update", th.bad)))
        }
        _ => {}
    }
    let Some(d) = &s.data else {
        return;
    };
    if d.gpus.is_empty() {
        out.push(Line::from(Span::styled("NVML reports 0 GPUs", th.warn)));
    }
    let spark_w = w.saturating_sub(34).clamp(8, 40);
    for g in &d.gpus {
        out.push(Line::from(format!(
            "[{}] {}",
            g.index,
            g.name.as_deref().unwrap_or("unnamed GPU")
        )));
        let vram = g.mem_used.zip(g.mem_total).filter(|(_, t)| *t > 0);
        match vram {
            Some((u, t)) => {
                let pct = u as f64 * 100.0 / t as f64;
                let mut spans = vec![
                    Span::raw(format!(
                        "  VRAM {} / {}  ",
                        format::bytes(u),
                        format::bytes(t)
                    )),
                    Span::styled(format!("{pct:.0}%"), pct_style(th, pct)),
                    Span::raw(format!(" {}", bar(pct / 100.0, 10, opts.unicode))),
                ];
                if g.index == 0 && app.ai.vram.len() > 1 {
                    spans.push(Span::styled(
                        format!(
                            "  {}",
                            spark(&app.ai.vram.to_vec(), spark_w, Some(100.0), opts.unicode)
                        ),
                        th.graph,
                    ));
                }
                out.push(Line::from(spans));
            }
            None => out.push(Line::from("  VRAM —")),
        }
        let mut spans = vec![Span::raw("  util ")];
        match g.util_gpu_percent {
            Some(u) => {
                spans.push(Span::styled(format!("{u}%"), pct_style(th, f64::from(u))));
                if g.index == 0 && app.ai.gpu_util.len() > 1 {
                    spans.push(Span::styled(
                        format!(
                            "  {}",
                            spark(
                                &app.ai.gpu_util.to_vec(),
                                spark_w,
                                Some(100.0),
                                opts.unicode
                            )
                        ),
                        th.graph,
                    ));
                }
            }
            None => spans.push(Span::raw("—")),
        }
        let deg = if opts.unicode { "°C" } else { " C" };
        spans.push(Span::raw(format!(
            "   temp {}   power {}",
            g.temp_c.map_or("—".into(), |c| format!("{c}{deg}")),
            g.power_watts.map_or("—".into(), |p| format!("{p:.0} W")),
        )));
        out.push(Line::from(spans));
    }
    if d.more > 0 {
        out.push(Line::from(Span::styled(
            format!("+{} more GPU(s) not shown", d.more),
            th.dim,
        )));
    }
    out.push(Line::from(Span::styled(
        "Per-process GPU memory is not available; the trend covers GPU 0 since this tab opened.",
        th.dim,
    )));
}

fn server_lines(
    r: &ServerReport,
    opts: &RenderOpts,
    w: usize,
    compact: bool,
    out: &mut Vec<Line<'static>>,
    inst: &mut Vec<Line<'static>>,
) {
    let th = &opts.theme;
    let dot_up = if opts.unicode { "●" } else { "*" };
    let dot_off = if opts.unicode { "○" } else { "o" };
    let kind = match r.kind {
        AiKind::Ollama => "Ollama API",
        AiKind::Lmstudio => "LM Studio API",
        AiKind::Openai => "OpenAI-compatible API",
    };
    match &r.state {
        ServerState::NotRunning => out.push(Line::from(Span::styled(
            format!("{dot_off} {}  {}  not running", r.name, r.display),
            th.dim,
        ))),
        ServerState::Error(m) => out.push(Line::from(vec![
            Span::styled(format!("{dot_off} {}  {}  ", r.name, r.display), th.warn),
            Span::styled(format!("error: {m}"), th.warn),
        ])),
        ServerState::Up(i) => {
            out.push(Line::from(vec![
                Span::styled(format!("{dot_up} {}", r.name), th.good),
                Span::raw(format!(
                    "  {}  {kind}{}  {} ms",
                    r.display,
                    i.version
                        .as_ref()
                        .map_or(String::new(), |v| format!(" v{v}")),
                    i.latency_ms
                )),
            ]));
            up_lines(&r.name, i, opts, w, compact, out, inst);
        }
    }
}

fn up_lines(
    name: &str,
    i: &ServerInfo,
    opts: &RenderOpts,
    w: usize,
    compact: bool,
    out: &mut Vec<Line<'static>>,
    inst: &mut Vec<Line<'static>>,
) {
    let th = &opts.theme;
    let sep = opts.sep();
    out.push(Line::from(format!(
        "  {} ({}):",
        i.loaded_label,
        i.loaded.len()
    )));
    if i.loaded.is_empty() {
        out.push(Line::from(Span::styled(
            "    none loaded right now",
            th.dim,
        )));
    }
    for m in &i.loaded {
        out.push(Line::from(Span::styled(
            format!("    {}", trunc(&m.name, w.saturating_sub(6), opts.unicode)),
            th.text,
        )));
        let d = model_detail(m, sep);
        if !d.is_empty() {
            out.push(Line::from(Span::styled(format!("      {d}"), th.dim)));
        }
        if let Some(f) = m.gpu_fraction() {
            let (txt, style) = if f >= 0.999 {
                ("fully on GPU".to_string(), th.good)
            } else {
                (
                    format!(
                        "{:.0}% on GPU, {:.0}% on CPU/RAM (slower)",
                        f * 100.0,
                        100.0 - f * 100.0
                    ),
                    th.warn,
                )
            };
            out.push(Line::from(vec![
                Span::raw(format!("      {} ", bar(f, 10, opts.unicode))),
                Span::styled(txt, style),
            ]));
        }
    }
    if let Some(m) = &i.metrics {
        let f =
            |v: Option<f64>, unit: &str| v.map_or("—".to_string(), |x| format!("{x:.1}{unit}"));
        out.push(Line::from(format!(
            "  generation {} {sep} prompt {} (server averages)",
            f(m.gen_tps, " tok/s"),
            f(m.prompt_tps, " tok/s")
        )));
        out.push(Line::from(format!(
            "  requests running {} {sep} waiting {} {sep} KV cache {}",
            f(m.running, ""),
            f(m.waiting, ""),
            m.kv_cache
                .map_or("—".to_string(), |c| format!("{:.0}%", c * 100.0)),
        )));
    }
    if let Some(list) = &i.installed {
        let total: u64 = list.iter().filter_map(|m| m.size).sum();
        let sized = list.iter().filter(|m| m.size.is_some()).count();
        let total_txt = if sized == 0 {
            String::new()
        } else if sized < list.len() {
            format!(
                ", {} on disk for the {sized} with a known size",
                format::bytes(total)
            )
        } else {
            format!(", {} on disk", format::bytes(total))
        };
        inst.push(Line::from(format!(
            "{name}: {} installed{total_txt}",
            list.len()
        )));

        if !compact {
            for m in list {
                let mut parts = Vec::new();
                parts.extend(m.params.clone());
                parts.extend(m.quant.clone());
                parts.extend(m.max_context.map(|c| format!("max ctx {c}")));
                parts.extend(m.size.map(format::bytes));
                inst.push(Line::from(Span::styled(
                    format!(
                        "    {}  {}",
                        trunc(&m.name, w.saturating_sub(34).max(16), opts.unicode),
                        parts.join(&format!(" {sep} "))
                    ),
                    th.dim,
                )));
            }
        }
    }
    if let Some(n) = &i.note {
        out.push(Line::from(Span::styled(format!("  note: {n}"), th.dim)));
    }
}

fn model_detail(m: &LoadedModel, sep: &str) -> String {
    let mut p: Vec<String> = Vec::new();
    p.extend(m.family.clone());
    p.extend(m.params.clone());
    p.extend(m.quant.clone());
    p.extend(m.context.map(|c| format!("ctx {c}")));
    if let Some(s) = m.size {
        let mem = match m.size_vram {
            Some(v) => format!("{} total, {} in VRAM", format::bytes(s), format::bytes(v)),
            None => format::bytes(s),
        };
        p.push(mem);
    }
    if let Some(e) = m.expires_unix {
        let left = e - now_unix();
        p.push(if left > FOREVER_SECS {
            "stays loaded".to_string()
        } else if left <= 0 {
            "unloading".to_string()
        } else {
            format!("unloads in {}", format::duration(left as u64))
        });
    }
    p.join(&format!(" {sep} "))
}

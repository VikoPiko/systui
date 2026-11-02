//! Terminal lifecycle and the interactive event loop.

use crate::app::{App, Effect};
use crate::collectors::{Cmd, Collector};
use crate::modules::health::HealthHandle;
use crate::modules::ssh::{SshCmd, SshHandle};
use crate::modules::ModuleSet;
use crate::ui::{self, RenderOpts};
use ratatui::crossterm::event::{self, Event};
use std::io::IsTerminal;
use std::time::{Duration, Instant};

/// Restores the terminal on drop (normal exit, `?` early return, unwinding).
struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub fn stdio_is_tty() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

/// How often the loop wakes when idle to look for a new snapshot. Input is
/// handled immediately regardless (the poll returns as soon as a key arrives).
const IDLE_WAKE: Duration = Duration::from_millis(100);

/// Run the interactive UI until the app asks to quit.
///
/// `ratatui::try_init` enters raw mode + alternate screen and installs a panic
/// hook that restores the terminal before the panic message prints.
pub fn run(mut app: App, opts: RenderOpts) -> std::io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _guard = RestoreGuard;
    let collector = Collector::spawn(Duration::from_millis(app.config.ui.refresh_ms));
    let mut modules = ModuleSet::new(&app.config);
    let ssh = (!app.ssh_specs.is_empty()).then(|| {
        SshHandle::spawn(
            app.ssh_specs.clone(),
            std::sync::Arc::new(crate::ssh::real_runner),
        )
    });
    let (ports_tx, ports_rx) = std::sync::mpsc::channel();
    let (task_tx, task_rx) = std::sync::mpsc::channel::<crate::app::TaskResult>();
    // Health checks run continuously (alerts depend on them) but only if the user enabled
    // them AND configured at least one enabled check.
    let health = crate::health::specs_from_config(&app.config)
        .filter(|specs| specs.iter().any(|s| s.enabled))
        .map(HealthHandle::spawn);
    let mut dirty = true;
    let mut was_stale = false;
    while !app.should_quit {
        if dirty {
            terminal.draw(|f| ui::render(f, &mut app, &opts))?;
            dirty = false;
        }
        if event::poll(IDLE_WAKE)? {
            // Coalesce a burst of queued events into a single redraw.
            loop {
                match event::read()? {
                    Event::Key(k) => dirty |= app.handle_key(k),
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
                if app.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        let now = Instant::now();
        if let Some(snap) = collector.take_latest() {
            app.ingest(snap, now);
            dirty = true;
        }
        let stale = app.is_stale(now);
        if stale != was_stale {
            was_stale = stale;
            dirty = true;
        }
        while let Ok(r) = task_rx.try_recv() {
            app.ingest_task(r);
            dirty = true;
        }
        if let Ok(r) = ports_rx.try_recv() {
            app.ingest_ports(r);
            dirty = true;
        }
        if let Some(h) = &ssh {
            if let Some(snap) = h.take_latest() {
                app.ingest_ssh(snap);
                dirty = true;
            }
        }
        if let Some(h) = &health {
            if let Some(snap) = h.take_latest() {
                app.ingest_health(snap, now);
                dirty = true;
            }
        }
        if modules.started() {
            let u = modules.take();
            if u.docker.is_some() || u.wsl.is_some() || u.gpu.is_some() {
                app.ingest_modules(u, now);
                dirty = true;
            }
        }
        for e in app.take_effects() {
            match e {
                Effect::RefreshNow => collector.send(Cmd::RefreshNow),
                Effect::Watch(w) => collector.send(Cmd::Watch(w)),
                Effect::Needs(n) => collector.send(Cmd::Needs(n)),
                Effect::DevVisible(v) => modules.set_visible(v),
                Effect::ModuleRefresh(k, extra) => modules.refresh(k, extra),
                Effect::CollectListeners => {
                    // Short-lived thread: the OS tool may take seconds; the UI never waits.
                    let tx = ports_tx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(crate::ports::collect());
                    });
                }
                Effect::DockerLogs { id, lines } => {
                    let tx = task_tx.clone();
                    std::thread::spawn(move || {
                        let r = crate::modules::docker::logs(&id, lines);
                        let _ = tx.send(crate::app::TaskResult::Logs(id, r));
                    });
                }
                Effect::DockerAction { action, id, name } => {
                    let tx = task_tx.clone();
                    std::thread::spawn(move || {
                        let r = crate::actions::docker::run(action, &id, &name);
                        let _ = tx.send(crate::app::TaskResult::Action(r));
                    });
                }
                Effect::SshVisible(v) => {
                    if let Some(h) = &ssh {
                        h.send(SshCmd::Visible(v));
                    }
                }
                Effect::SshConsent(name) => {
                    if let Some(h) = &ssh {
                        h.send(SshCmd::Consent(name));
                    }
                }
                Effect::SshCheckNow => {
                    if let Some(h) = &ssh {
                        h.send(SshCmd::CheckNow);
                    }
                }
                Effect::HealthCheckNow => {
                    if let Some(h) = &health {
                        h.check_now();
                    }
                }
            }
        }
    }
    Ok(())
}

/// Heuristic: modern Windows terminals and UTF-8 locales render box drawing.
pub fn detect_unicode() -> bool {
    if cfg!(windows) {
        return true;
    }
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .map(|v| v.to_ascii_lowercase().contains("utf"))
        .unwrap_or(false)
}

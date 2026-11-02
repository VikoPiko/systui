//! Non-interactive subcommands: `doctor` and `snapshot`. They never enter raw
//! mode or the alternate screen.

use crate::collectors::system::SystemCollector;
use crate::config::{default_config_path, Config};
use crate::domain::{CollectorState, Snapshot};
use crate::export::{build, Redactor};
use crate::terminal::stdio_is_tty;
use crate::util::format;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Like `println!` but a closed pipe (`systui doctor | head`) is not a panic.
macro_rules! say {
    ($($a:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($a)*);
    }};
}

/// Two samples so CPU %, process CPU and network rates have a baseline.
pub fn sample_twice() -> Result<Snapshot, String> {
    let mut c = SystemCollector::new()?;
    c.sample();
    std::thread::sleep(Duration::from_millis(600));
    Ok(c.sample())
}

/// Reject remote-looking targets; only plain local paths are accepted.
pub fn validate_output_path(p: &Path) -> Result<(), String> {
    let s = p.to_string_lossy();
    if s.starts_with("\\\\") || s.starts_with("//") || s.contains("://") {
        return Err(format!("--output must be a local file path, got `{s}`"));
    }
    if p.is_dir() {
        return Err(format!("--output `{s}` is a directory"));
    }
    Ok(())
}

/// Write `text` to `path`; refuses to overwrite unless `force`.
pub fn write_output(path: &Path, text: &str, force: bool) -> Result<(), String> {
    validate_output_path(path)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if force {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    let mut f = opts.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "`{}` already exists; pass --force to overwrite it",
                path.display()
            )
        } else {
            format!("cannot write `{}`: {e}", path.display())
        }
    })?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write `{}`: {e}", path.display()))
}

pub fn snapshot(
    json: bool,
    output: Option<PathBuf>,
    force: bool,
    top: usize,
) -> Result<(), String> {
    // Validate the destination before doing any work.
    if let Some(p) = &output {
        validate_output_path(p)?;
        if !force && p.exists() {
            return Err(format!(
                "`{}` already exists; pass --force to overwrite it",
                p.display()
            ));
        }
    }
    let snap = sample_twice()?;
    let doc = build(&snap, top.min(1000), &Redactor::from_env());
    let text = if json {
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())? + "\n"
    } else {
        human_summary(&snap)
    };
    match output {
        Some(p) => {
            write_output(&p, &text, force)?;
            eprintln!("wrote {}", p.display());
            Ok(())
        }
        None => {
            // Ignore a closed pipe (e.g. `| head`) instead of panicking.
            let _ = std::io::stdout().write_all(text.as_bytes());
            Ok(())
        }
    }
}

fn human_summary(s: &Snapshot) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "systui snapshot  {}\n",
        crate::export::utc_string(
            s.sampled_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        )
    ));
    o.push_str(&format!(
        "CPU    {} ({} logical)\n",
        format::percent(s.cpu.usage.map(f64::from)),
        s.cpu.logical
    ));
    match &s.memory {
        Some(m) => o.push_str(&format!(
            "RAM    {} / {}\n",
            format::bytes(m.used),
            format::bytes(m.total)
        )),
        None => o.push_str("RAM    unavailable\n"),
    }
    for v in &s.volumes {
        o.push_str(&format!(
            "DISK   {}  {:.0}% used, {} free\n",
            v.mount,
            v.used_pct(),
            format::bytes(v.available)
        ));
    }
    if let Some((rx, tx)) = s.net_totals() {
        o.push_str(&format!(
            "NET    down {}  up {}\n",
            format::rate(rx),
            format::rate(tx)
        ));
    }
    o.push_str(&format!("PROCS  {}\n", s.processes.len()));
    o.push_str("Use --json for the full, machine-readable form.\n");
    o
}

/// Returns `Ok(true)` when every core collector works.
pub fn doctor(config_override: Option<&Path>) -> Result<bool, String> {
    let mut ok = true;
    let line = |label: &str, value: String| say!("{label:<21}{value}");
    say!(
        "systui {}  ({} / {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    say!();

    // Config
    let (path, explicit) = match config_override {
        Some(p) => (Some(p.to_path_buf()), true),
        None => (default_config_path(), false),
    };
    match &path {
        Some(p) => {
            line("config path", p.display().to_string());
            match Config::load(p, explicit) {
                Ok(_) if p.exists() => line("config", "found and valid".into()),
                Ok(_) => line("config", "not present (built-in defaults in use)".into()),
                Err(e) => {
                    ok = false;
                    line("config", format!("ERROR: {e}"));
                }
            }
        }
        None => line(
            "config path",
            "could not determine a config directory".into(),
        ),
    }

    // Terminal
    line("stdin/stdout tty", stdio_is_tty().to_string());
    line(
        "NO_COLOR",
        if std::env::var_os("NO_COLOR").is_some() {
            "set (colors off)".into()
        } else {
            "not set".into()
        },
    );
    line(
        "unicode (auto)",
        crate::terminal::detect_unicode().to_string(),
    );
    say!();

    // Collectors
    match sample_twice() {
        Ok(s) => {
            for d in &s.diagnostics {
                let st = match d.state {
                    CollectorState::Ok => "ok",
                    CollectorState::WarmingUp => "warming up",
                    CollectorState::Unsupported => "UNSUPPORTED",
                    CollectorState::Error => "ERROR",
                };
                if matches!(d.state, CollectorState::Error | CollectorState::Unsupported)
                    && matches!(d.name, "cpu" | "memory" | "processes")
                {
                    ok = false;
                }
                let extra = d
                    .detail
                    .as_deref()
                    .map(|x| format!(" - {x}"))
                    .unwrap_or_default();
                line(&format!("collector {}", d.name), format!("{st}{extra}"));
            }
            line("processes", s.processes.len().to_string());
            let restricted = s.processes.iter().filter(|p| p.restricted).count();
            if restricted > 0 {
                line(
                    "restricted procs",
                    format!(
                        "{restricted} (OS denied access; shown as `denied`, systui never elevates)"
                    ),
                );
            }
            line("interfaces", s.network.len().to_string());
            line(
                "sensors",
                match (&s.sensors.battery, s.sensors.temps.len()) {
                    (None, 0) => format!(
                        "no battery; temperatures: {}",
                        s.sensors.temps_note.as_deref().unwrap_or("none")
                    ),
                    (b, n) => format!(
                        "battery {}; {} temperature sensor(s)",
                        b.as_ref().map_or("none".to_string(), |b| format!(
                            "{}% ({})",
                            b.percent.map_or("?".to_string(), |p| p.to_string()),
                            b.state.label()
                        )),
                        n
                    ),
                },
            );
            line("volumes", s.volumes.len().to_string());
            line(
                "sysinfo CPU",
                format!("{} logical, {}", s.cpu.logical, s.cpu.brand),
            );
        }
        Err(e) => {
            ok = false;
            line("collectors", format!("ERROR: {e}"));
        }
    }
    say!();
    // Optional integrations: doctor is an explicit request, so it probes each one
    // (bounded by timeouts) and also says whether the config enables it.
    let cfg = path
        .as_deref()
        .and_then(|p| Config::load(p, explicit).ok())
        .unwrap_or_default();
    let on = |b: bool| {
        if b {
            "enabled"
        } else {
            "disabled in config (not used)"
        }
    };
    let show = |r: Result<String, crate::modules::ModStatus>| match r {
        Ok(s) => format!("available - {s}"),
        Err(crate::modules::ModStatus::Unavailable(m)) => format!("unavailable - {m}"),
        Err(crate::modules::ModStatus::Error(m)) => format!("ERROR - {m}"),
        Err(other) => format!("{other:?}"),
    };
    line(
        "docker",
        format!(
            "{}; {}",
            on(cfg.modules.docker),
            show(crate::modules::docker::probe().map(|v| format!("engine {v}")))
        ),
    );
    line(
        "wsl",
        format!(
            "{}; {}",
            on(cfg.modules.wsl),
            show(
                crate::modules::wsl::poll(&[])
                    .map(|d| format!("{} distribution(s)", d.distros.len()))
            )
        ),
    );
    line(
        "nvidia gpu",
        format!(
            "{}; {}",
            on(cfg.modules.gpu),
            show(crate::modules::gpu::probe().map(|(drv, n)| format!(
                "driver {}, {n} GPU(s)",
                drv.unwrap_or_else(|| "?".into())
            )))
        ),
    );
    // M6 features. Nothing here contacts a remote host, writes a recording or touches a container.
    let ssh_client = match crate::util::exec::run("ssh", &["-V"], std::time::Duration::from_secs(5))
    {
        Ok(o) => format!(
            "available - {}",
            crate::util::text::sanitize(String::from_utf8_lossy(&o.stderr).trim())
        ),
        Err(crate::util::exec::ExecError::NotFound(_)) => "not found on PATH".to_string(),
        Err(e) => format!("unusable - {e}"),
    };
    line(
        "ssh checks",
        format!(
            "{}; {} host(s) in the allowlist; client: {ssh_client}; consent is asked per session, in the UI",
            if cfg.modules.ssh { "enabled" } else { "disabled in config (no host is contacted)" },
            cfg.ssh.hosts.len()
        ),
    );
    line(
        "docker extras",
        format!(
            "logs: {} lines; stop/restart: {}",
            cfg.docker.log_lines,
            if cfg.docker.allow_actions {
                "ALLOWED (each asks for the container name)"
            } else {
                "off"
            }
        ),
    );
    line(
        "recording",
        if cfg.recording.enabled {
            format!(
                "allowed (start with Ctrl+R) -> {}; every {}s, keep {} files / {} MB / {} days",
                cfg.recording
                    .resolved_dir()
                    .map_or("?".to_string(), |d| d.display().to_string()),
                cfg.recording.interval_seconds,
                cfg.recording.max_files,
                cfg.recording.max_total_mb,
                cfg.recording.max_age_days
            )
        } else {
            "off (nothing is ever recorded)".into()
        },
    );
    {
        let s = crate::sensors::read();
        line(
            "power/thermal",
            match (&s.battery, s.temps.len()) {
                (None, 0) => format!(
                    "no battery; temperatures: {}",
                    s.temps_note.as_deref().unwrap_or("none")
                ),
                (b, n) => format!(
                    "battery {}; {n} temperature sensor(s)",
                    b.as_ref().map_or("none".to_string(), |b| format!(
                        "{}% ({})",
                        b.percent.map_or("?".to_string(), |p| p.to_string()),
                        b.state.label()
                    ))
                ),
            },
        );
    }
    // Health checks are only described here, never executed by doctor.
    let h = &cfg.health;
    let enabled_checks = h.checks.iter().filter(|c| c.enabled).count();
    line(
        "health checks",
        if cfg.modules.health {
            format!(
                "enabled; {} configured, {} enabled; targets: {}",
                h.checks.len(),
                enabled_checks,
                if h.allow_lan {
                    "loopback + private LAN"
                } else {
                    "loopback only"
                }
            )
        } else {
            "disabled in config (nothing is checked)".into()
        },
    );
    line(
        "alerts",
        if cfg.alerts.enabled {
            format!(
                "on: CPU>={}% RAM>={}% disk free<{}% for {}s; services after {} failures",
                cfg.alerts.cpu_percent,
                cfg.alerts.memory_percent,
                cfg.alerts.disk_free_percent_below,
                cfg.alerts.sustain_seconds,
                cfg.alerts.service_failures
            )
        } else {
            "off".into()
        },
    );
    say!();
    say!(
        "{}",
        if ok {
            "doctor: all core collectors available"
        } else {
            "doctor: PROBLEMS found (see above)"
        }
    );
    Ok(ok)
}

/// Write `bytes` to stdout, ignoring a closed pipe (`systui man | head`).
fn write_stdout(bytes: &[u8]) {
    let _ = std::io::stdout().write_all(bytes);
}

/// Shell completion script for `shell`, generated from the live CLI definition.
pub fn completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut crate::cli::Cli::command(), "systui", &mut buf);
    write_stdout(&buf);
}

/// The manual page in roff format, generated from the live CLI definition.
pub fn man() -> Result<(), String> {
    use clap::CommandFactory;
    let mut buf = Vec::new();
    clap_mangen::Man::new(crate::cli::Cli::command())
        .render(&mut buf)
        .map_err(|e| format!("cannot render the man page: {e}"))?;
    write_stdout(&buf);
    Ok(())
}

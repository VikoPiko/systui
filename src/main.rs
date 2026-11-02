use clap::Parser;
use std::process::ExitCode;
use systui::app::App;
use systui::cli::{Cli, Command};
use systui::commands;
use systui::config::{default_config_path, Config, UnicodeMode};
use systui::terminal;
use systui::ui::{theme::Theme, RenderOpts};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(msg) => {
            eprintln!("systui: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(cli: &Cli) -> Result<Config, String> {
    let mut cfg = match &cli.config {
        Some(p) => Config::load(p, true),
        None => match default_config_path() {
            Some(p) => Config::load(&p, false),
            None => Ok(Config::default()),
        },
    }
    .map_err(|e| e.to_string())?;
    cfg.apply_overrides(cli.refresh_ms)
        .map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// `Ok(false)` = ran fine but reports a problem (exit code 1, no extra message).
fn run(cli: Cli) -> Result<bool, String> {
    match &cli.command {
        // doctor reports config problems itself instead of aborting early.
        Some(Command::Doctor) => return commands::doctor(cli.config.as_deref()),
        Some(Command::Snapshot {
            json,
            output,
            force,
            top,
        }) => {
            commands::snapshot(*json, output.clone(), *force, *top)?;
            return Ok(true);
        }
        None => {}
    }
    let cfg = load_config(&cli)?;
    if !terminal::stdio_is_tty() {
        return Err(
            "interactive mode needs a terminal (stdin/stdout is not a TTY); \
                    use `systui snapshot --json` for scripting"
                .into(),
        );
    }
    let no_color = cli.no_color || std::env::var_os("NO_COLOR").is_some();
    let opts = RenderOpts {
        theme: Theme::resolve(cfg.ui.theme, no_color),
        unicode: match cfg.ui.unicode {
            UnicodeMode::Always => true,
            UnicodeMode::Never => false,
            UnicodeMode::Auto => terminal::detect_unicode(),
        },
    };
    terminal::run(App::new(cfg), opts).map_err(|e| format!("terminal error: {e}"))?;
    Ok(true)
}

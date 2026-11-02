//! Command-line interface definition.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "systui",
    version,
    about = "Fast, keyboard-first terminal system monitor",
    long_about = "SysTUI is a read-only, native terminal system monitor.\n\
                  Run without arguments to open the interactive UI."
)]
pub struct Cli {
    /// Override the user config file path.
    #[arg(long, value_name = "PATH", global = true)]
    pub config: Option<PathBuf>,

    /// Refresh interval in milliseconds (500-5000).
    #[arg(long, value_name = "MS", global = true)]
    pub refresh_ms: Option<u64>,

    /// Disable colors (text contrast is retained). `NO_COLOR` is also honored.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check collector availability and print resolved paths.
    Doctor,
    /// Print a shell completion script (bash, zsh, fish, powershell, elvish) to stdout.
    Completions { shell: clap_complete::Shell },
    /// Print the manual page (roff) to stdout.
    Man,
    /// Print a one-shot, read-only system summary.
    Snapshot {
        /// Emit JSON (currently the only format).
        #[arg(long)]
        json: bool,
        /// Write to this file instead of stdout.
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Allow overwriting an existing --output file.
        #[arg(long)]
        force: bool,
        /// How many top processes (by CPU) to include.
        #[arg(long, value_name = "N", default_value_t = 25)]
        top: usize,
    },
}

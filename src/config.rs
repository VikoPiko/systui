//! User configuration: safe defaults, validation, precedence.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MIN_REFRESH_MS: u64 = 500;
pub const MAX_REFRESH_MS: u64 = 5000;
pub const MIN_HISTORY: usize = 30;
pub const MAX_HISTORY: usize = 300;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid setting `{key}`: {message}")]
    Invalid { key: &'static str, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    Dark,
    Light,
    Mono,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnicodeMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartView {
    Overview,
    Processes,
    Network,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Cpu,
    Memory,
    Name,
    Pid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuMode {
    /// 100% = one logical CPU; may exceed 100%.
    PerCoreEquivalent,
    /// Share of total host CPU capacity (0-100%).
    HostShare,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: ThemeName,
    pub unicode: UnicodeMode,
    pub refresh_ms: u64,
    pub history_samples: usize,
    pub start_view: StartView,
    pub show_clock: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProcessConfig {
    pub default_sort: SortKey,
    pub cpu_mode: CpuMode,
    pub confirm_terminate: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ModulesConfig {
    /// NVIDIA GPU panel; detected at runtime, hidden if unsupported.
    pub gpu: bool,
    /// Docker CLI integration (opt-in).
    pub docker: bool,
    /// WSL distro list (Windows only, opt-in).
    pub wsl: bool,
    /// Local health checks / Services tab (opt-in).
    pub health: bool,
    /// Read-only SSH checks of allowlisted hosts (opt-in, needs per-session consent).
    pub ssh: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DeveloperConfig {
    pub process_families: Vec<String>,
    /// Extra whole-word name tokens per family, e.g. `wsl = ["vmmemwsl"]`.
    pub aliases: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct WslConfig {
    /// Explicit, user-approved `.vhdx` files whose host file size is shown.
    pub tracked_vhdx_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckKind {
    Tcp,
    Http,
    Process,
    Service,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CheckConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: CheckKind,
    pub host: Option<String>,
    pub port: Option<u16>,
    /// `http://` only (no TLS support); host must be local.
    pub url: Option<String>,
    pub process: Option<String>,
    pub service: Option<String>,
    pub enabled: bool,
    pub interval_seconds: u64,
    pub timeout_ms: u64,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: CheckKind::Tcp,
            host: None,
            port: None,
            url: None,
            process: None,
            service: None,
            enabled: true,
            interval_seconds: 30,
            timeout_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HealthConfig {
    /// Also allow private-LAN addresses (10/8, 172.16/12, 192.168/16, fc00::/7).
    /// Loopback is always allowed; public addresses and hostnames never are.
    pub allow_lan: bool,
    pub checks: Vec<CheckConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AlertsConfig {
    pub enabled: bool,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub disk_free_percent_below: f64,
    /// A CPU/RAM/disk condition must hold this long before an alert is raised.
    pub sustain_seconds: u64,
    /// An alert clears only after the value is this many points back inside the limit...
    pub hysteresis_points: f64,
    /// ...for this long.
    pub clear_seconds: u64,
    /// After clearing, the same alert cannot re-raise for this long.
    pub cooldown_seconds: u64,
    /// Consecutive failed checks before a service alert.
    pub service_failures: u32,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cpu_percent: 90.0,
            memory_percent: 90.0,
            disk_free_percent_below: 10.0,
            sustain_seconds: 15,
            hysteresis_points: 5.0,
            clear_seconds: 5,
            cooldown_seconds: 30,
            service_failures: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// Allows recording to be started. It never starts by itself.
    pub enabled: bool,
    /// Absolute directory; default is `<per-user data dir>/systui/recordings`.
    pub directory: Option<String>,
    pub interval_seconds: u64,
    /// Top processes (by CPU) included in each line.
    pub top_processes: usize,
    pub max_file_mb: u64,
    pub max_files: usize,
    pub max_total_mb: u64,
    pub max_age_days: u64,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: None,
            interval_seconds: 5,
            top_processes: 10,
            max_file_mb: 10,
            max_files: 10,
            max_total_mb: 100,
            max_age_days: 7,
        }
    }
}

impl RecordingConfig {
    pub fn resolved_dir(&self) -> Option<PathBuf> {
        match &self.directory {
            Some(d) => Some(PathBuf::from(d)),
            None => directories::ProjectDirs::from("", "", "systui")
                .map(|p| p.data_local_dir().join("recordings")),
        }
    }

    pub fn limits(&self) -> crate::recorder::Limits {
        crate::recorder::Limits {
            max_file_bytes: self.max_file_mb * 1024 * 1024,
            max_files: self.max_files,
            max_total_bytes: self.max_total_mb * 1024 * 1024,
            max_age: std::time::Duration::from_secs(self.max_age_days * 86_400),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DockerConfig {
    /// Lines shown by the log preview (`l` on a container).
    pub log_lines: usize,
    /// Allow stop/restart from the Dev tab (each one still needs typed confirmation).
    pub allow_actions: bool,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            log_lines: 100,
            allow_actions: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SshHostConfig {
    /// Label shown in the UI and typed to give consent.
    pub name: String,
    /// Hostname, IP, or a `Host` alias from your `~/.ssh/config`.
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub interval_seconds: u64,
    /// systemd units to report with `systemctl is-active`.
    pub units: Vec<String>,
    pub enabled: bool,
}

impl Default for SshHostConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            user: None,
            port: None,
            interval_seconds: 60,
            units: Vec::new(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    pub hosts: Vec<SshHostConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub processes: ProcessConfig,
    pub modules: ModulesConfig,
    pub developer: DeveloperConfig,
    pub wsl: WslConfig,
    pub health: HealthConfig,
    pub alerts: AlertsConfig,
    pub recording: RecordingConfig,
    pub docker: DockerConfig,
    pub ssh: SshConfig,
}

impl Default for ModulesConfig {
    fn default() -> Self {
        Self {
            gpu: true,
            docker: false,
            wsl: false,
            health: false,
            ssh: false,
        }
    }
}

impl Default for DeveloperConfig {
    fn default() -> Self {
        let names = [
            "dotnet", "node", "pnpm", "npm", "cargo", "rustc", "git", "docker", "wsl",
        ];
        let mut aliases = HashMap::new();
        aliases.insert(
            "wsl".to_string(),
            ["wslservice", "wslhost", "wslrelay", "vmmemwsl"]
                .map(String::from)
                .to_vec(),
        );
        Self {
            process_families: names.map(String::from).to_vec(),
            aliases,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: ThemeName::Dark,
            unicode: UnicodeMode::Auto,
            refresh_ms: 1000,
            history_samples: 300,
            start_view: StartView::Overview,
            show_clock: true,
        }
    }
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            default_sort: SortKey::Cpu,
            cpu_mode: CpuMode::PerCoreEquivalent,
            confirm_terminate: true,
        }
    }
}

impl Config {
    /// Parse and validate TOML text. Unknown keys are ignored.
    pub fn from_toml(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.message().to_string(),
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Load from `path`, or defaults when the file is absent.
    pub fn load(path: &Path, must_exist: bool) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !must_exist => {
                Ok(Self::default())
            }
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_refresh(self.ui.refresh_ms)?;
        self.validate_dev()?;
        self.validate_health()?;
        self.validate_alerts()?;
        self.validate_recording()?;
        self.validate_ssh()?;
        if !(10..=500).contains(&self.docker.log_lines) {
            return Err(ConfigError::Invalid {
                key: "docker.log_lines",
                message: format!("{} is out of range; use 10-500", self.docker.log_lines),
            });
        }
        if !(MIN_HISTORY..=MAX_HISTORY).contains(&self.ui.history_samples) {
            return Err(ConfigError::Invalid {
                key: "ui.history_samples",
                message: format!(
                    "{} is out of range; use {MIN_HISTORY}-{MAX_HISTORY}",
                    self.ui.history_samples
                ),
            });
        }
        Ok(())
    }

    fn validate_ssh(&self) -> Result<(), ConfigError> {
        let bad = |m: String| {
            Err(ConfigError::Invalid {
                key: "ssh.hosts",
                message: m,
            })
        };
        if self.ssh.hosts.len() > 8 {
            return bad("at most 8 hosts".into());
        }
        let mut names = std::collections::HashSet::new();
        for h in &self.ssh.hosts {
            let spec = match crate::ssh::HostSpec::from_config(h) {
                Ok(s) => s,
                Err(e) => return bad(format!("`{}`: {e}", h.name)),
            };
            if !names.insert(spec.name.to_lowercase()) {
                return bad(format!("duplicate host name `{}`", spec.name));
            }
        }
        Ok(())
    }

    fn validate_recording(&self) -> Result<(), ConfigError> {
        let r = &self.recording;
        let bad = |key: &'static str, message: &str| {
            Err(ConfigError::Invalid {
                key,
                message: message.to_string(),
            })
        };
        if !(1..=3600).contains(&r.interval_seconds) {
            return bad("recording.interval_seconds", "use 1-3600");
        }
        if r.top_processes > 50 {
            return bad("recording.top_processes", "use 0-50");
        }
        if !(1..=1024).contains(&r.max_file_mb) {
            return bad("recording.max_file_mb", "use 1-1024");
        }
        if !(1..=1000).contains(&r.max_files) {
            return bad("recording.max_files", "use 1-1000");
        }
        if !(1..=10_240).contains(&r.max_total_mb) {
            return bad("recording.max_total_mb", "use 1-10240");
        }
        if !(1..=3650).contains(&r.max_age_days) {
            return bad("recording.max_age_days", "use 1-3650");
        }
        if let Some(d) = &r.directory {
            let p = Path::new(d);
            if !p.is_absolute() || d.contains("..") {
                return bad(
                    "recording.directory",
                    "must be an absolute path without `..`",
                );
            }
        }
        Ok(())
    }

    fn validate_alerts(&self) -> Result<(), ConfigError> {
        let a = &self.alerts;
        let pct = |v: f64| v.is_finite() && (1.0..=100.0).contains(&v);
        let bad = |key: &'static str, message: String| Err(ConfigError::Invalid { key, message });
        if !pct(a.cpu_percent) {
            return bad(
                "alerts.cpu_percent",
                format!("{} is out of range; use 1-100", a.cpu_percent),
            );
        }
        if !pct(a.memory_percent) {
            return bad(
                "alerts.memory_percent",
                format!("{} is out of range; use 1-100", a.memory_percent),
            );
        }
        if !pct(a.disk_free_percent_below) {
            return bad(
                "alerts.disk_free_percent_below",
                format!("{} is out of range; use 1-100", a.disk_free_percent_below),
            );
        }
        if !(a.hysteresis_points.is_finite() && (0.0..=50.0).contains(&a.hysteresis_points)) {
            return bad("alerts.hysteresis_points", "use 0-50".into());
        }
        if !(1..=3600).contains(&a.sustain_seconds) {
            return bad("alerts.sustain_seconds", "use 1-3600".into());
        }
        if !(1..=3600).contains(&a.clear_seconds) {
            return bad("alerts.clear_seconds", "use 1-3600".into());
        }
        if a.cooldown_seconds > 86_400 {
            return bad("alerts.cooldown_seconds", "use 0-86400".into());
        }
        if !(1..=100).contains(&a.service_failures) {
            return bad("alerts.service_failures", "use 1-100".into());
        }
        Ok(())
    }

    fn validate_health(&self) -> Result<(), ConfigError> {
        let h = &self.health;
        let bad = |message: String| {
            Err(ConfigError::Invalid {
                key: "health.checks",
                message,
            })
        };
        if h.checks.len() > 32 {
            return bad("at most 32 checks".into());
        }
        let mut names = std::collections::HashSet::new();
        for c in &h.checks {
            let n = c.name.trim();
            if n.is_empty() || n.chars().count() > 40 || n.chars().any(char::is_control) {
                return bad(format!(
                    "check name `{}` must be 1-40 printable characters",
                    c.name
                ));
            }
            if !names.insert(n.to_lowercase()) {
                return bad(format!("duplicate check name `{n}`"));
            }
            if !(5..=3600).contains(&c.interval_seconds) {
                return bad(format!("`{n}`: interval_seconds must be 5-3600"));
            }
            if !(100..=10_000).contains(&c.timeout_ms) || c.timeout_ms >= c.interval_seconds * 1000
            {
                return bad(format!(
                    "`{n}`: timeout_ms must be 100-10000 and below the interval"
                ));
            }
            // Only enabled checks need to be well-formed targets (the shipped example is disabled),
            // but a malformed one is still reported so typos are caught early.
            if let Err(e) = crate::health::CheckSpec::from_config(c, h.allow_lan) {
                return bad(format!("`{n}`: {e}"));
            }
        }
        Ok(())
    }

    fn validate_dev(&self) -> Result<(), ConfigError> {
        let f = &self.developer.process_families;
        if f.len() > 32
            || f.iter()
                .any(|s| s.trim().is_empty() || s.chars().count() > 40)
        {
            return Err(ConfigError::Invalid {
                key: "developer.process_families",
                message: "use at most 32 non-empty names of up to 40 characters".into(),
            });
        }
        let p = &self.wsl.tracked_vhdx_paths;
        if p.len() > 16 {
            return Err(ConfigError::Invalid {
                key: "wsl.tracked_vhdx_paths",
                message: "at most 16 paths".into(),
            });
        }
        for path in p {
            let pb = Path::new(path);
            let ok_ext = pb
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("vhdx"));
            if !ok_ext || !pb.is_absolute() || path.contains("..") {
                return Err(ConfigError::Invalid {
                    key: "wsl.tracked_vhdx_paths",
                    message: format!("`{path}` must be an absolute path to a .vhdx file (no `..`)"),
                });
            }
        }
        Ok(())
    }

    /// Apply command-line overrides (they take precedence over the file).
    pub fn apply_overrides(&mut self, refresh_ms: Option<u64>) -> Result<(), ConfigError> {
        if let Some(ms) = refresh_ms {
            validate_refresh(ms)?;
            self.ui.refresh_ms = ms;
        }
        Ok(())
    }
}

fn validate_refresh(ms: u64) -> Result<(), ConfigError> {
    if (MIN_REFRESH_MS..=MAX_REFRESH_MS).contains(&ms) {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            key: "ui.refresh_ms",
            message: format!("{ms} is out of range; use {MIN_REFRESH_MS}-{MAX_REFRESH_MS}"),
        })
    }
}

/// Default per-user config path (`<config dir>/systui/config.toml`).
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "systui").map(|d| d.config_dir().join("config.toml"))
}

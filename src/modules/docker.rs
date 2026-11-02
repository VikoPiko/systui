//! Read-only Docker integration through the `docker` CLI (fixed argv, timeouts,
//! bounded output). Never starts Docker, never changes anything.
//!
//! Output formats are requested with explicit Go templates (tab separated) so
//! labels and environment never reach us.

use super::ModStatus;
use crate::util::exec::{self, ExecError};
use crate::util::text::sanitize;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

pub const CADENCE: Duration = Duration::from_secs(5);
const T_LIST: Duration = Duration::from_secs(5);
const T_STATS: Duration = Duration::from_secs(10);
const T_DF: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// Compose project (the single label `com.docker.compose.project`), or empty.
    pub project: String,
    /// running / exited / paused / created / restarting / dead
    pub state: String,
    /// Docker's human status, e.g. "Up 3 hours" or "Exited (1) 2 minutes ago".
    pub status: String,
    /// `None` for stopped containers or when stats are unavailable.
    pub cpu_percent: Option<f32>,
    /// Docker's own text, e.g. "45.5MiB / 7.6GiB".
    pub mem_usage: Option<String>,
    pub mem_percent: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DfRow {
    pub kind: String,
    pub total: String,
    pub active: String,
    pub size: String,
    pub reclaimable: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DockerData {
    pub version: Option<String>,
    pub containers: Vec<Container>,
    /// Only present after an explicit refresh (`R`); not polled.
    pub df: Option<Vec<DfRow>>,
    pub df_at: Option<SystemTime>,
    /// Non-fatal problem, e.g. stats timed out.
    pub warning: Option<String>,
    /// Output lines that did not parse.
    pub skipped_lines: usize,
}

/// `docker stats` style "12.34%" -> 12.34. `--` and garbage -> None.
pub fn parse_percent(s: &str) -> Option<f32> {
    s.trim()
        .strip_suffix('%')?
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// `{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Label "com.docker.compose.project"}}`
pub fn parse_ps(text: &str) -> (Vec<Container>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 6 || f[0].trim().is_empty() {
            skipped += 1;
            continue;
        }
        out.push(Container {
            id: sanitize(f[0].trim()),
            name: sanitize(f[1].trim()),
            image: sanitize(f[2].trim()),
            state: sanitize(f[3].trim()).to_lowercase(),
            status: sanitize(f[4].trim()),
            project: sanitize(f[5].trim()),
            cpu_percent: None,
            mem_usage: None,
            mem_percent: None,
        });
    }
    (out, skipped)
}

/// `{{.ID}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.MemPerc}}` keyed by container id.
/// (cpu %, memory text, memory %) per container id.
pub type Stat = (Option<f32>, Option<String>, Option<f32>);

pub fn parse_stats(text: &str) -> HashMap<String, Stat> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 4 || f[0].trim().is_empty() {
            continue;
        }
        let usage = sanitize(f[2].trim());
        m.insert(
            sanitize(f[0].trim()),
            (
                parse_percent(f[1]),
                (!usage.is_empty() && usage != "--").then_some(usage),
                parse_percent(f[3]),
            ),
        );
    }
    m
}

/// `{{.Type}}\t{{.TotalCount}}\t{{.Active}}\t{{.Size}}\t{{.Reclaimable}}`
pub fn parse_df(text: &str) -> Vec<DfRow> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() == 5).then(|| DfRow {
                kind: sanitize(f[0].trim()),
                total: sanitize(f[1].trim()),
                active: sanitize(f[2].trim()),
                size: sanitize(f[3].trim()),
                reclaimable: sanitize(f[4].trim()),
            })
        })
        .collect()
}

pub fn merge_stats(containers: &mut [Container], stats: &HashMap<String, Stat>) {
    for c in containers.iter_mut().filter(|c| c.state == "running") {
        if let Some((cpu, mem, pct)) = stats.get(&c.id) {
            c.cpu_percent = *cpu;
            c.mem_usage = mem.clone();
            c.mem_percent = *pct;
        }
    }
}

fn classify(e: ExecError) -> ModStatus {
    match e {
        ExecError::NotFound(_) => ModStatus::Unavailable("`docker` CLI not found on PATH".into()),
        ExecError::Failed { stderr, .. } => {
            // Typically "Cannot connect to the Docker daemon..." / pipe not found.
            let first = stderr.lines().next().unwrap_or("").to_string();
            ModStatus::Unavailable(format!("engine not reachable: {first}"))
        }
        ExecError::Timeout(..) => {
            ModStatus::Error("docker did not answer in time (timeout)".into())
        }
        other => ModStatus::Error(other.to_string()),
    }
}

fn text(o: &exec::Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// One poll. `with_df` also runs `docker system df` (explicit refresh only).
/// `prev_df` is carried over so the df table stays visible between polls.
pub fn poll(
    with_df: bool,
    version: &mut Option<String>,
    prev_df: &mut Option<(Vec<DfRow>, SystemTime)>,
) -> Result<DockerData, ModStatus> {
    let ps = exec::run(
        "docker",
        &[
            "ps",
            "-a",
            "--format",
            "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Label \"com.docker.compose.project\"}}",
        ],
        T_LIST,
    )
    .map_err(classify)?;
    let (mut containers, skipped) = parse_ps(&text(&ps));
    let mut warning = ps
        .truncated
        .then(|| "container list truncated (output cap)".to_string());

    if version.is_none() {
        if let Ok(v) = exec::run(
            "docker",
            &["version", "--format", "{{.Server.Version}}"],
            T_LIST,
        ) {
            *version = Some(sanitize(text(&v).trim()));
        }
    }
    // Group compose projects together; the Dev tab's selection index follows this order.
    containers.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    if containers.iter().any(|c| c.state == "running") {
        match exec::run(
            "docker",
            &[
                "stats",
                "--no-stream",
                "--format",
                "{{.ID}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.MemPerc}}",
            ],
            T_STATS,
        ) {
            Ok(o) => merge_stats(&mut containers, &parse_stats(&text(&o))),
            Err(e) => warning = Some(format!("stats unavailable: {e}")),
        }
    }
    if with_df {
        match exec::run(
            "docker",
            &[
                "system",
                "df",
                "--format",
                "{{.Type}}\t{{.TotalCount}}\t{{.Active}}\t{{.Size}}\t{{.Reclaimable}}",
            ],
            T_DF,
        ) {
            Ok(o) => *prev_df = Some((parse_df(&text(&o)), SystemTime::now())),
            Err(e) => warning = Some(format!("system df failed: {e}")),
        }
    }
    Ok(DockerData {
        version: version.clone(),
        containers,
        df: prev_df.as_ref().map(|d| d.0.clone()),
        df_at: prev_df.as_ref().map(|d| d.1),
        warning,
        skipped_lines: skipped,
    })
}

/// A container id as Docker prints it: 12-64 lowercase hex characters. Anything else is
/// refused before it can reach a command line (an id starting with `-` would be an option).
pub fn valid_container_id(id: &str) -> bool {
    (12..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPreview {
    /// Newest last, ANSI-stripped and sanitized, one entry per log line.
    pub lines: Vec<String>,
    /// More output existed than the capture limit.
    pub truncated: bool,
}

const MAX_LOG_LINE: usize = 400;

/// Merge Docker's stdout and stderr streams (both carry `--timestamps` prefixes) into one
/// chronological preview of at most `max` lines.
pub fn parse_logs(stdout: &str, stderr: &str, max: usize, truncated: bool) -> LogPreview {
    let mut all: Vec<(String, String)> = Vec::new();
    for raw in stdout.lines().chain(stderr.lines()) {
        let clean = sanitize(&crate::util::text::strip_ansi(raw));
        if clean.trim().is_empty() {
            continue;
        }
        // RFC3339Nano timestamps sort lexicographically; keep the first token as the key.
        let key = clean.split(' ').next().unwrap_or("").to_string();
        let mut line = clean;
        if line.chars().count() > MAX_LOG_LINE {
            line = line.chars().take(MAX_LOG_LINE).collect::<String>() + "…";
        }
        all.push((key, line));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0)); // stable: ties keep stream order
    let skip = all.len().saturating_sub(max);
    LogPreview {
        lines: all.into_iter().skip(skip).map(|(_, l)| l).collect(),
        truncated,
    }
}

/// Read-only: the last `n` log lines of one container.
pub fn logs(id: &str, n: usize) -> Result<LogPreview, String> {
    if !valid_container_id(id) {
        return Err("refusing to read logs: not a valid container id".into());
    }
    let tail = n.clamp(1, 1000).to_string();
    let o = exec::run(
        "docker",
        &["logs", "--tail", &tail, "--timestamps", id],
        T_LIST,
    )
    .map_err(|e| match classify(e) {
        ModStatus::Unavailable(m) | ModStatus::Error(m) => m,
        other => format!("{other:?}"),
    })?;
    Ok(parse_logs(
        &String::from_utf8_lossy(&o.stdout),
        &String::from_utf8_lossy(&o.stderr),
        n,
        o.truncated,
    ))
}

/// Cheap availability check (used by `doctor`): the engine version, or why not.
pub fn probe() -> Result<String, ModStatus> {
    exec::run(
        "docker",
        &["version", "--format", "{{.Server.Version}}"],
        T_LIST,
    )
    .map(|o| sanitize(text(&o).trim()))
    .map_err(classify)
}

pub fn spawn() -> super::ModuleHandle<DockerData> {
    let mut version = None;
    let mut df = None;
    super::ModuleHandle::spawn("docker", CADENCE, move |extra| {
        poll(extra, &mut version, &mut df)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "1a2b3c4d5e6f\tshop-web-1\tshop/web:dev\trunning\tUp 3 hours\tshop\n\
                      2b3c4d5e6f70\tgateway\tshop/gateway:dev\trunning\tUp 3 hours (healthy)\t\n\
                      0badc0ffee00\told-job\tbusybox\texited\tExited (1) 2 days ago\t\n";

    #[test]
    fn parses_ps() {
        let (c, skipped) = parse_ps(PS);
        assert_eq!(skipped, 0);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].name, "shop-web-1");
        assert_eq!(c[0].project, "shop");
        assert_eq!(c[1].project, "", "no compose label => empty project");
        assert_eq!(c[2].state, "exited");
        assert!(c[2].cpu_percent.is_none());
    }

    #[test]
    fn malformed_lines_are_counted_not_guessed() {
        let (c, skipped) = parse_ps("garbage\n\nonly\ttwo\n");
        assert!(c.is_empty());
        assert_eq!(skipped, 2);
    }

    #[test]
    fn hostile_names_are_sanitized() {
        let (c, _) = parse_ps("abc\t\x1b[31mevil\x07\timg\trunning\tUp\t\n");
        assert!(!c[0].name.contains('\x1b') && !c[0].name.contains('\x07'));
    }

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("12.34%"), Some(12.34));
        assert_eq!(parse_percent(" 0.00% "), Some(0.0));
        assert_eq!(parse_percent("--"), None);
        assert_eq!(parse_percent("abc%"), None);
        assert_eq!(parse_percent("-3%"), None);
        assert_eq!(parse_percent("NaN%"), None);
    }

    #[test]
    fn stats_merge_only_into_running_containers() {
        let (mut c, _) = parse_ps(PS);
        let st = parse_stats(
            "1a2b3c4d5e6f\t1.50%\t45.5MiB / 7.6GiB\t0.58%\n\
             0badc0ffee00\t9.00%\t1MiB / 2GiB\t0.1%\n\
             2b3c4d5e6f70\t--\t--\t--\n",
        );
        merge_stats(&mut c, &st);
        assert_eq!(c[0].cpu_percent, Some(1.5));
        assert_eq!(c[0].mem_usage.as_deref(), Some("45.5MiB / 7.6GiB"));
        assert_eq!(c[0].mem_percent, Some(0.58));
        assert_eq!(c[1].cpu_percent, None, "`--` is unknown, not 0");
        assert_eq!(c[1].mem_usage, None);
        assert_eq!(
            c[2].cpu_percent, None,
            "stopped container ignores stray stats"
        );
    }

    #[test]
    fn container_id_validation() {
        assert!(valid_container_id("1a2b3c4d5e6f"));
        assert!(valid_container_id(&"a".repeat(64)));
        for bad in [
            "",
            "short",
            "-abcdef123456",
            "ABCDEF123456",
            "d22ed356ec9g",
            "1a2b3c4d5e6f;rm",
            "d22ed356 ec93",
            &"a".repeat(65),
        ] {
            assert!(!valid_container_id(bad), "{bad}");
        }
    }

    #[test]
    fn log_streams_merge_chronologically_and_are_cleaned() {
        let out = "2026-11-02T10:00:01.000000000Z listening on :8080\n2026-11-02T10:00:03.000000000Z \u{1b}[32mrequest ok\u{1b}[0m\n";
        let err = "2026-11-02T10:00:02.000000000Z warn: slow query\n\n";
        let p = parse_logs(out, err, 10, false);
        assert_eq!(p.lines.len(), 3);
        assert!(
            p.lines[0].contains("listening")
                && p.lines[1].contains("slow query")
                && p.lines[2].contains("request ok")
        );
        assert!(p.lines.iter().all(|l| !l.contains('\u{1b}')));
    }

    #[test]
    fn log_preview_keeps_the_newest_n_and_truncates_long_lines() {
        let out: String = (0..50)
            .map(|i| format!("2026-11-02T10:00:{i:02}.000000000Z line{i}\n"))
            .collect();
        let p = parse_logs(&out, "", 5, true);
        assert_eq!(p.lines.len(), 5);
        assert!(p.lines[0].ends_with("line45") && p.lines[4].ends_with("line49"));
        assert!(p.truncated);
        let long = format!("2026-11-02T10:00:00.000000000Z {}\n", "x".repeat(5000));
        let p = parse_logs(&long, "", 5, false);
        assert!(p.lines[0].chars().count() <= MAX_LOG_LINE + 1 && p.lines[0].ends_with('…'));
    }

    #[test]
    fn hostile_log_text_cannot_reach_the_terminal() {
        let p = parse_logs("t \u{1b}]0;pwn\u{7}\u{1b}[2J\u{8}end\n", "", 5, false);
        assert!(!p.lines[0].chars().any(|c| c.is_control()), "{:?}", p.lines);
    }

    #[test]
    fn logs_refuses_bad_ids_without_running_anything() {
        assert!(logs("--help", 10)
            .unwrap_err()
            .contains("valid container id"));
        assert!(logs("; calc", 10).is_err());
    }

    #[test]
    fn df_rows() {
        let rows = parse_df(
            "Images\t12\t5\t6.2GB\t3.1GB (50%)\nBuild Cache\t88\t0\t4.4GB\t4.4GB\nbroken line\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, "Images");
        assert_eq!(rows[1].reclaimable, "4.4GB");
    }

    #[test]
    fn exec_errors_map_to_honest_statuses() {
        assert!(
            matches!(classify(ExecError::NotFound("docker".into())), ModStatus::Unavailable(m) if m.contains("PATH"))
        );
        assert!(matches!(
            classify(ExecError::Failed { program: "docker".into(), code: "1".into(), stderr: "Cannot connect".into() }),
            ModStatus::Unavailable(m) if m.contains("not reachable")
        ));
        assert!(matches!(
            classify(ExecError::Timeout("docker".into(), 5000)),
            ModStatus::Error(_)
        ));
    }
}

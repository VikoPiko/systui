//! Read-only remote health over the system `ssh` client.
//!
//! Safety model:
//! * only hosts listed in the config (an allowlist) can ever be contacted, and only
//!   after the user explicitly consents in the UI for that session;
//! * authentication is whatever your `ssh` already does (keys / agent / `~/.ssh/config`);
//!   systui stores no credentials and never prompts: `BatchMode=yes`;
//! * host keys are never auto-accepted: `StrictHostKeyChecking=yes`;
//! * the remote command is a FIXED read-only script (plus `systemctl is-active` for unit
//!   names validated against a strict character set); no user text is ever executed;
//! * no forwarding of any kind.

use crate::config::SshHostConfig;
use crate::util::exec::{ExecError, Output};
use crate::util::text::sanitize;
use std::time::Duration;

/// Separates the sections of the remote script's output.
pub const MARKER: &str = "#SYSTUI#";
pub const CONNECT_TIMEOUT_S: u32 = 5;
pub const RUN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    pub name: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub interval: Duration,
    pub units: Vec<String>,
    pub enabled: bool,
}

fn token_ok(s: &str, extra: &[char], max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

impl HostSpec {
    pub fn from_config(c: &SshHostConfig) -> Result<Self, String> {
        let name = c.name.trim();
        if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
            return Err(format!(
                "host name `{}` must be 1-40 printable characters",
                c.name
            ));
        }
        let host = c.host.trim();
        // A leading `-` would be parsed by ssh as an option (e.g. -oProxyCommand=...).
        if !token_ok(host, &['.', '-', '_', ':'], 253) {
            return Err(format!(
                "host `{host}` is invalid: letters, digits, . - _ : only, and it must not start with `-`"
            ));
        }
        if let Some(u) = &c.user {
            if !token_ok(u, &['.', '-', '_'], 32) {
                return Err(format!(
                    "user `{u}` is invalid: letters, digits, . - _ only, not starting with `-`"
                ));
            }
        }
        if c.port == Some(0) {
            return Err("port must be 1-65535".into());
        }
        if !(15..=3600).contains(&c.interval_seconds) {
            return Err("interval_seconds must be 15-3600".into());
        }
        if c.units.len() > 8 {
            return Err("at most 8 units per host".into());
        }
        for u in &c.units {
            if !token_ok(u, &['.', '-', '_', '@'], 64) {
                return Err(format!(
                    "unit `{u}` is invalid: letters, digits, . - _ @ only"
                ));
            }
        }
        Ok(Self {
            name: name.to_string(),
            host: host.to_string(),
            user: c.user.clone(),
            port: c.port,
            interval: Duration::from_secs(c.interval_seconds),
            units: c.units.clone(),
            enabled: c.enabled,
        })
    }

    /// `user@host:port` for display (no secrets are involved).
    pub fn dest(&self) -> String {
        format!(
            "{}{}{}",
            self.user
                .as_ref()
                .map_or(String::new(), |u| format!("{u}@")),
            self.host,
            self.port.map_or(String::new(), |p| format!(":{p}"))
        )
    }

    /// The exact script run on the remote host. Everything in it is fixed text except the
    /// validated unit names.
    pub fn remote_command(&self) -> String {
        let m = MARKER;
        let mut s = format!(
            "cat /proc/loadavg; echo '{m}'; \
             grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo; echo '{m}'; \
             df -P -B1 / 2>/dev/null | tail -n +2; echo '{m}'; \
             cut -d' ' -f1 /proc/uptime"
        );
        if !self.units.is_empty() {
            s.push_str(&format!(
                "; echo '{m}'; systemctl is-active -- {} 2>/dev/null",
                self.units.join(" ")
            ));
        }
        s.push_str("; true");
        s
    }

    /// Full argv for the `ssh` program (without the program name).
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "-o",
            "BatchMode=yes",
            "-o",
            &format!("ConnectTimeout={CONNECT_TIMEOUT_S}"),
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "NumberOfPasswordPrompts=0",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ForwardX11=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "LogLevel=ERROR",
            "-T",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(p) = self.port {
            a.push("-p".into());
            a.push(p.to_string());
        }
        if let Some(u) = &self.user {
            a.push("-l".into());
            a.push(u.clone());
        }
        a.push("--".into());
        a.push(self.host.clone());
        a.push(self.remote_command());
        a
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteStatus {
    pub load: Option<(f32, f32, f32)>,
    pub mem_total: Option<u64>,
    pub mem_available: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_free: Option<u64>,
    /// Root filesystem: (total, available) bytes.
    pub disk_root: Option<(u64, u64)>,
    pub uptime_secs: Option<u64>,
    /// (unit, state exactly as systemd printed it)
    pub units: Vec<(String, String)>,
}

pub fn parse_loadavg(s: &str) -> Option<(f32, f32, f32)> {
    let mut it = s.split_whitespace();
    let v = |x: Option<&str>| {
        x?.parse::<f32>()
            .ok()
            .filter(|f| f.is_finite() && *f >= 0.0)
    };
    Some((v(it.next())?, v(it.next())?, v(it.next())?))
}

/// `/proc/meminfo` subset -> (total, available, swap_total, swap_free) in bytes.
pub fn parse_meminfo(s: &str) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
    let get = |key: &str| {
        s.lines().find_map(|l| {
            let rest = l.strip_prefix(key)?.strip_prefix(':')?;
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            kb.checked_mul(1024)
        })
    };
    (
        get("MemTotal"),
        get("MemAvailable"),
        get("SwapTotal"),
        get("SwapFree"),
    )
}

/// Body of `df -P -B1 /` without its header: the root line -> (total, available).
pub fn parse_df(s: &str) -> Option<(u64, u64)> {
    s.lines().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.len() < 6 || t[t.len() - 1] != "/" {
            return None;
        }
        let total: u64 = t[1].parse().ok()?;
        let avail: u64 = t[3].parse().ok()?;
        (total > 0 && avail <= total).then_some((total, avail))
    })
}

pub fn parse_uptime(s: &str) -> Option<u64> {
    let f: f64 = s.split_whitespace().next()?.parse().ok()?;
    (f.is_finite() && f >= 0.0).then_some(f as u64)
}

/// Parse the whole script output. A completely unrecognisable reply means the remote is not
/// a Linux host with /proc (or something else answered), which is an error rather than data.
pub fn parse_output(text: &str, units: &[String]) -> Result<RemoteStatus, String> {
    let sections: Vec<&str> = text.split(MARKER).collect();
    let sec = |i: usize| sections.get(i).map_or("", |s| s.trim_matches(['\r', '\n']));
    let load = parse_loadavg(sec(0));
    let (mem_total, mem_available, swap_total, swap_free) = parse_meminfo(sec(1));
    if load.is_none() && mem_total.is_none() {
        return Err(
            "unexpected reply: the remote does not look like a Linux host with /proc".into(),
        );
    }
    let unit_states: Vec<String> = sec(4).lines().map(|l| sanitize(l.trim())).collect();
    Ok(RemoteStatus {
        load,
        mem_total,
        mem_available,
        swap_total,
        swap_free,
        disk_root: parse_df(sec(2)),
        uptime_secs: parse_uptime(sec(3)),
        units: units
            .iter()
            .enumerate()
            .map(|(i, u)| {
                (
                    u.clone(),
                    unit_states
                        .get(i)
                        .cloned()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "unknown".into()),
                )
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Network-level: refused, timed out, no route, name not found.
    Unreachable(String),
    /// Reachable but not usable: auth, host key, ssh missing, bad reply.
    Error(String),
}

pub fn classify_ssh_error(stderr: &str) -> Failure {
    let first = sanitize(
        stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("ssh failed")
            .trim(),
    );
    let l = stderr.to_lowercase();
    if l.contains("host key verification failed")
        || l.contains("remote host identification has changed")
    {
        Failure::Error("host key not trusted (or changed): verify it once with `ssh` yourself; systui never accepts host keys".into())
    } else if l.contains("permission denied") || l.contains("no supported authentication") {
        Failure::Error("authentication failed: your ssh keys/agent were not accepted".into())
    } else if l.contains("connection refused")
        || l.contains("timed out")
        || l.contains("no route to host")
        || l.contains("could not resolve")
        || l.contains("network is unreachable")
        || l.contains("connection reset")
        || l.contains("connection closed")
    {
        Failure::Unreachable(first)
    } else {
        Failure::Error(first)
    }
}

/// Run one poll through `run` (the real `ssh` in production, a fake in tests).
pub fn poll<F>(spec: &HostSpec, run: F) -> Result<RemoteStatus, Failure>
where
    F: FnOnce(&[String], Duration) -> Result<Output, ExecError>,
{
    match run(&spec.args(), RUN_TIMEOUT) {
        Ok(o) => {
            parse_output(&String::from_utf8_lossy(&o.stdout), &spec.units).map_err(Failure::Error)
        }
        Err(ExecError::NotFound(_)) => Err(Failure::Error(
            "the `ssh` client was not found on PATH".into(),
        )),
        Err(ExecError::Timeout(..)) => Err(Failure::Unreachable("no answer within 15 s".into())),
        Err(ExecError::Failed { stderr, .. }) => Err(classify_ssh_error(&stderr)),
        Err(e) => Err(Failure::Error(sanitize(&e.to_string()))),
    }
}

/// The real thing: the system `ssh`.
pub fn real_runner(args: &[String], timeout: Duration) -> Result<Output, ExecError> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::util::exec::run("ssh", &refs, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(host: &str) -> SshHostConfig {
        SshHostConfig {
            name: "lab".into(),
            host: host.into(),
            ..Default::default()
        }
    }

    #[test]
    fn host_user_port_unit_validation_blocks_option_injection() {
        assert!(HostSpec::from_config(&cfg("192.168.1.50")).is_ok());
        assert!(HostSpec::from_config(&cfg("home-lab.example.org")).is_ok());
        assert!(HostSpec::from_config(&cfg("fe80::1")).is_ok());
        for bad in [
            "-oProxyCommand=calc",
            "-p",
            "",
            "host name",
            "host;rm",
            "a$(b)",
            "h`x`",
            "host\nx",
            &"a".repeat(300),
        ] {
            assert!(HostSpec::from_config(&cfg(bad)).is_err(), "{bad:?}");
        }
        let mut c = cfg("h");
        c.user = Some("-oProxyCommand=x".into());
        assert!(HostSpec::from_config(&c).is_err());
        c.user = Some("deploy user".into());
        assert!(HostSpec::from_config(&c).is_err());
        c.user = Some("viko".into());
        c.port = Some(0);
        assert!(HostSpec::from_config(&c).is_err());
        c.port = Some(2222);
        c.interval_seconds = 5;
        assert!(
            HostSpec::from_config(&c).is_err(),
            "too frequent for a remote host"
        );
        c.interval_seconds = 60;
        c.units = vec!["docker".into(), "bad unit; reboot".into()];
        assert!(HostSpec::from_config(&c).unwrap_err().contains("unit"));
        c.units = vec!["--now".into()];
        assert!(
            HostSpec::from_config(&c).is_err(),
            "units cannot start with a dash either"
        );
        c.units = (0..9).map(|i| format!("u{i}")).collect();
        assert!(HostSpec::from_config(&c).is_err());
    }

    #[test]
    fn argv_is_hardened_and_the_host_comes_after_double_dash() {
        let mut c = cfg("10.0.0.5");
        c.user = Some("viko".into());
        c.port = Some(2222);
        c.units = vec!["docker".into(), "nginx".into()];
        let s = HostSpec::from_config(&c).unwrap();
        let a = s.args();
        for must in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "NumberOfPasswordPrompts=0",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "ControlMaster=no",
            "ControlPath=none",
            "-T",
        ] {
            assert!(a.iter().any(|x| x == must), "missing {must}: {a:?}");
        }
        let dd = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(a[dd + 1], "10.0.0.5");
        assert_eq!(
            dd + 3,
            a.len(),
            "exactly one remote command argument after the host"
        );
        assert!(
            a.iter().position(|x| x == "-p").unwrap() < dd
                && a.iter().position(|x| x == "-l").unwrap() < dd
        );
        assert!(!a
            .iter()
            .any(|x| x.contains("StrictHostKeyChecking=no") || x == "-A" || x == "-i"));
        assert_eq!(s.dest(), "viko@10.0.0.5:2222");
    }

    #[test]
    fn the_remote_script_is_fixed_read_only_text() {
        let mut c = cfg("h");
        let none = HostSpec::from_config(&c).unwrap().remote_command();
        for w in [
            "/proc/loadavg",
            "/proc/meminfo",
            "df -P -B1 /",
            "/proc/uptime",
        ] {
            assert!(none.contains(w), "{w}");
        }
        assert!(!none.contains("systemctl"), "no units => no systemctl");
        for forbidden in [
            "rm ", "sudo", "kill", "|sh", "curl", "wget", "reboot", "shutdown", "chmod", "mv ",
            "dd ",
        ] {
            assert!(
                !none.contains(forbidden),
                "script must not contain `{forbidden}`: {none}"
            );
        }
        assert!(
            none.ends_with("; true"),
            "exit status 0 so partial output is still delivered"
        );
        c.units = vec!["docker".into(), "ssh@x".into()];
        let with = HostSpec::from_config(&c).unwrap().remote_command();
        assert!(with.contains("systemctl is-active -- docker ssh@x 2>/dev/null"));
    }

    const OUT: &str = "0.52 0.58 0.59 1/467 12345\n#SYSTUI#\n\
MemTotal:       16318112 kB\nMemAvailable:   11034560 kB\nSwapTotal:       2097148 kB\nSwapFree:        2097148 kB\n#SYSTUI#\n\
/dev/sda2 105089261568 42319249408 57352310784 43% /\n#SYSTUI#\n\
98765.43\n#SYSTUI#\nactive\ninactive\n";

    #[test]
    fn full_reply_is_parsed() {
        let r = parse_output(OUT, &["docker".into(), "nginx".into()]).unwrap();
        assert_eq!(r.load, Some((0.52, 0.58, 0.59)));
        assert_eq!(r.mem_total, Some(16318112 * 1024));
        assert_eq!(r.mem_available, Some(11034560 * 1024));
        assert_eq!(r.swap_total, Some(2097148 * 1024));
        assert_eq!(r.disk_root, Some((105089261568, 57352310784)));
        assert_eq!(r.uptime_secs, Some(98765));
        assert_eq!(
            r.units,
            [
                ("docker".to_string(), "active".to_string()),
                ("nginx".to_string(), "inactive".to_string())
            ]
        );
    }

    #[test]
    fn partial_replies_leave_gaps_as_none_not_zero() {
        // no df (busybox), no units, short uptime section
        let out = "1.00 2.00 3.00 1/1 1\n#SYSTUI#\nMemTotal: 1000 kB\n#SYSTUI#\n\n#SYSTUI#\n";
        let r = parse_output(out, &[]).unwrap();
        assert_eq!(r.load, Some((1.0, 2.0, 3.0)));
        assert_eq!((r.mem_total, r.mem_available), (Some(1_024_000), None));
        assert_eq!((r.disk_root, r.uptime_secs), (None, None));
        let r = parse_output(OUT, &["a".into(), "b".into(), "c".into()]).unwrap();
        assert_eq!(
            r.units[2],
            ("c".to_string(), "unknown".to_string()),
            "missing state is unknown, not 'active'"
        );
    }

    #[test]
    fn non_linux_or_garbage_replies_are_errors() {
        assert!(parse_output("", &[]).unwrap_err().contains("Linux"));
        assert!(parse_output("cat: /proc/loadavg: No such file\n#SYSTUI#\n", &[]).is_err());
        assert!(parse_output("Welcome to Windows!\r\n", &[]).is_err());
    }

    #[test]
    fn individual_parsers_reject_nonsense() {
        assert_eq!(parse_loadavg("abc"), None);
        assert_eq!(parse_loadavg("1.0 2.0"), None);
        assert_eq!(parse_loadavg("-1 0 0"), None);
        assert_eq!(
            parse_df("tmpfs 10 5 5 50% /run"),
            None,
            "only the root filesystem"
        );
        assert_eq!(
            parse_df("/dev/x 100 50 500 50% /"),
            None,
            "available > total is rejected"
        );
        assert_eq!(parse_df("/dev/x 0 0 0 0% /"), None);
        assert_eq!(parse_uptime("nan"), None);
        assert_eq!(parse_meminfo("MemTotal: x kB"), (None, None, None, None));
    }

    #[test]
    fn hostile_unit_states_are_sanitised() {
        let out = "0.1 0.1 0.1 1/1 1\n#SYSTUI#\nMemTotal: 1 kB\n#SYSTUI#\n#SYSTUI#\n1.0\n#SYSTUI#\n\u{1b}[31mactive\u{7}\n";
        let r = parse_output(out, &["u".into()]).unwrap();
        assert!(!r.units[0].1.chars().any(char::is_control), "{:?}", r.units);
    }

    #[test]
    fn ssh_errors_are_classified_honestly() {
        use Failure::*;
        assert!(
            matches!(classify_ssh_error("Host key verification failed.\r\n"), Error(m) if m.contains("never accepts"))
        );
        assert!(matches!(
            classify_ssh_error("@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@"),
            Error(_)
        ));
        assert!(
            matches!(classify_ssh_error("viko@h: Permission denied (publickey)."), Error(m) if m.contains("authentication"))
        );
        assert!(matches!(
            classify_ssh_error("ssh: connect to host h port 22: Connection refused"),
            Unreachable(_)
        ));
        assert!(matches!(
            classify_ssh_error("ssh: connect to host h port 22: Connection timed out"),
            Unreachable(_)
        ));
        assert!(matches!(
            classify_ssh_error("ssh: Could not resolve hostname nope: No such host is known."),
            Unreachable(_)
        ));
        assert!(matches!(classify_ssh_error("something odd"), Error(m) if m == "something odd"));
        assert!(matches!(classify_ssh_error(""), Error(_)));
    }

    #[test]
    fn poll_maps_runner_results() {
        let s = HostSpec::from_config(&cfg("h")).unwrap();
        let ok = poll(&s, |_, _| {
            Ok(Output {
                stdout: OUT.as_bytes().to_vec(),
                stderr: vec![],
                truncated: false,
            })
        });
        assert!(ok.unwrap().load.is_some());
        let nf = poll(&s, |_, _| Err(ExecError::NotFound("ssh".into())));
        assert!(matches!(nf, Err(Failure::Error(m)) if m.contains("not found")));
        let to = poll(&s, |_, _| Err(ExecError::Timeout("ssh".into(), 15000)));
        assert!(matches!(to, Err(Failure::Unreachable(_))));
        let fail = poll(&s, |_, _| {
            Err(ExecError::Failed {
                program: "ssh".into(),
                code: "255".into(),
                stderr: "Permission denied (publickey).".into(),
            })
        });
        assert!(matches!(fail, Err(Failure::Error(m)) if m.contains("authentication")));
        let junk = poll(&s, |_, _| {
            Ok(Output {
                stdout: b"hello".to_vec(),
                stderr: vec![],
                truncated: false,
            })
        });
        assert!(matches!(junk, Err(Failure::Error(_))));
    }

    #[test]
    fn the_runner_receives_the_hardened_argv_and_a_deadline() {
        let s = HostSpec::from_config(&cfg("h")).unwrap();
        let mut seen = None;
        let _ = poll(&s, |a, t| {
            seen = Some((a.to_vec(), t));
            Err(ExecError::NotFound("ssh".into()))
        });
        let (a, t) = seen.unwrap();
        assert!(a.contains(&"BatchMode=yes".to_string()));
        assert_eq!(t, RUN_TIMEOUT);
    }

    /// Real `ssh` against a port nobody listens on: must fail fast and cleanly, never hang or prompt.
    #[test]
    fn real_ssh_to_a_closed_local_port_fails_fast_without_prompting() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut c = cfg("127.0.0.1");
        c.port = Some(port);
        let s = HostSpec::from_config(&c).unwrap();
        let t = std::time::Instant::now();
        let r = poll(&s, real_runner);
        assert!(t.elapsed() < Duration::from_secs(12), "{:?}", t.elapsed());
        match r {
            Err(Failure::Unreachable(m)) => {
                assert!(m.to_lowercase().contains("refused") || !m.is_empty(), "{m}")
            }
            Err(Failure::Error(m)) if m.contains("not found") => {
                eprintln!("no ssh client installed: skipped")
            }
            other => panic!("expected Unreachable, got {other:?}"),
        }
    }
}

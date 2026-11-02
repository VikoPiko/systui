//! Local-only health checks. A check only ever talks to loopback addresses (or
//! private-LAN addresses when `allow_lan` is set), never resolves arbitrary
//! hostnames and never sends credentials. HTTP checks read the status line only.
//!
//! A TCP connect proves a port is open, NOT that the application is healthy; the
//! statuses keep those apart.

use crate::config::{CheckConfig, CheckKind};
use crate::util::exec::{self, ExecError};
use crate::util::text::sanitize;
use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Tcp {
        host: String,
        port: u16,
    },
    Http {
        host: String,
        port: u16,
        path_and_query: String,
        display: String,
    },
    Process(String),
    Service(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSpec {
    pub name: String,
    pub kind: CheckKind,
    pub target: Target,
    pub enabled: bool,
    pub interval: Duration,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Never checked yet (or disabled).
    Unknown,
    /// HTTP 2xx/3xx, process running, service active.
    Healthy,
    /// TCP connected. Says nothing about the application behind the port.
    PortOpen,
    /// The server answered, but not with a success status.
    HttpResponded(u16),
    NotReachable,
    /// Process absent / service not active.
    NotRunning,
    /// The check itself could not be performed (bad service name, tool missing…).
    Error,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Unknown => "unknown".into(),
            Status::Healthy => "healthy".into(),
            Status::PortOpen => "port open".into(),
            Status::HttpResponded(c) => format!("http {c}"),
            Status::NotReachable => "not reachable".into(),
            Status::NotRunning => "not running".into(),
            Status::Error => "check error".into(),
        }
    }

    /// Counts toward "unhealthy after N failures". `PortOpen` is not a failure.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Status::HttpResponded(_) | Status::NotReachable | Status::NotRunning | Status::Error
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub latency: Option<Duration>,
    pub message: Option<String>,
}

/// Only IP literals and `localhost` are accepted: no DNS lookups of arbitrary names.
pub fn host_allowed(host: &str, allow_lan: bool) -> Result<(), String> {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    let ip: IpAddr = h.parse().map_err(|_| {
        format!("host `{h}` is not allowed: use `localhost` or an IP address (names are never resolved)")
    })?;
    let ok = match ip {
        IpAddr::V4(v) => v.is_loopback() || (allow_lan && (v.is_private() || v.is_link_local())),
        IpAddr::V6(v) => {
            v.is_loopback()
                || (allow_lan
                    && ((v.segments()[0] & 0xfe00) == 0xfc00
                        || (v.segments()[0] & 0xffc0) == 0xfe80))
        }
    };
    if ok {
        Ok(())
    } else if ip.is_unspecified() {
        Err(format!("host `{h}` is not a destination; use 127.0.0.1"))
    } else {
        Err(format!(
            "host `{h}` is not local; checks are loopback-only{}",
            if allow_lan {
                " (private LAN addresses are allowed, public ones never)"
            } else {
                " (set [health] allow_lan = true for private LAN addresses)"
            }
        ))
    }
}

/// -> (host, port, path+query, display). Display never contains credentials or the query.
pub fn parse_http_url(url: &str, allow_lan: bool) -> Result<(String, u16, String, String), String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Err(
            "https is not supported (no TLS); use an http:// local endpoint or a tcp check".into(),
        );
    }
    let rest = lower
        .strip_prefix("http://")
        .map(|_| &url["http://".len()..])
        .ok_or("url must start with http://")?;
    if rest.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("url must not contain spaces or control characters".into());
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    if authority.contains('@') {
        return Err("credentials in the url are not supported".into());
    }
    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let end = stripped.find(']').ok_or("unterminated [ipv6] address")?;
        let after = &stripped[end + 1..];
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse::<u16>().map_err(|_| "invalid port")?,
            None if after.is_empty() => 80,
            None => return Err("invalid url authority".into()),
        };
        (stripped[..end].to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| "invalid port")?),
            None => (authority.to_string(), 80),
        }
    };
    if host.is_empty() {
        return Err("url has no host".into());
    }
    host_allowed(&host, allow_lan)?;
    let tail = tail.split('#').next().unwrap_or("");
    let pq = if tail.is_empty() || tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    if pq.len() > 512 {
        return Err("url path is too long (max 512)".into());
    }
    let path_only = pq.split('?').next().unwrap_or("/");
    let shown_host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let display = format!(
        "http://{shown_host}:{port}{path_only}{}",
        if pq.contains('?') { "?…" } else { "" }
    );
    Ok((host, port, pq, display))
}

/// The configured checks as specs, or `None` when health checks are not enabled.
/// (Config validation already guarantees every check parses.)
pub fn specs_from_config(cfg: &crate::config::Config) -> Option<Vec<CheckSpec>> {
    if !cfg.modules.health {
        return None;
    }
    let specs: Vec<CheckSpec> = cfg
        .health
        .checks
        .iter()
        .filter_map(|c| CheckSpec::from_config(c, cfg.health.allow_lan).ok())
        .collect();
    (!specs.is_empty()).then_some(specs)
}

fn valid_service_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | '-'))
}

impl CheckSpec {
    pub fn from_config(c: &CheckConfig, allow_lan: bool) -> Result<Self, String> {
        let need = |v: &Option<String>, what: &str| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{what} is required for type `{:?}`", c.kind).to_lowercase())
        };
        let target = match c.kind {
            CheckKind::Tcp => {
                let host = need(&c.host, "host")?;
                host_allowed(&host, allow_lan)?;
                let port = c
                    .port
                    .filter(|p| *p != 0)
                    .ok_or("port is required (1-65535)")?;
                Target::Tcp { host, port }
            }
            CheckKind::Http => {
                let url = need(&c.url, "url")?;
                let (host, port, path_and_query, display) = parse_http_url(&url, allow_lan)?;
                Target::Http {
                    host,
                    port,
                    path_and_query,
                    display,
                }
            }
            CheckKind::Process => Target::Process(need(&c.process, "process")?),
            CheckKind::Service => {
                let s = need(&c.service, "service")?;
                if !valid_service_name(&s) {
                    return Err("service name may only contain letters, digits and _ . @ -".into());
                }
                Target::Service(s)
            }
        };
        Ok(Self {
            name: c.name.trim().to_string(),
            kind: c.kind,
            target,
            enabled: c.enabled,
            interval: Duration::from_secs(c.interval_seconds),
            timeout: Duration::from_millis(c.timeout_ms),
        })
    }

    /// Safe-to-display target description.
    pub fn describe(&self) -> String {
        match &self.target {
            Target::Tcp { host, port } => format!("tcp {host}:{port}"),
            Target::Http { display, .. } => display.clone(),
            Target::Process(p) => format!("process {p}"),
            Target::Service(s) => format!("service {s}"),
        }
    }
}

fn io_msg(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => "connection refused".into(),
        TimedOut | WouldBlock => "timed out".into(),
        other => sanitize(&format!("{other}")),
    }
}

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, std::io::Error> {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    let addr = (h, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address"))?;
    TcpStream::connect_timeout(&addr, timeout)
}

fn run_tcp(host: &str, port: u16, timeout: Duration) -> Outcome {
    let t = Instant::now();
    match connect(host, port, timeout) {
        Ok(_) => Outcome {
            status: Status::PortOpen,
            latency: Some(t.elapsed()),
            message: None,
        },
        Err(e) => Outcome {
            status: Status::NotReachable,
            latency: None,
            message: Some(io_msg(&e)),
        },
    }
}

/// Parse "HTTP/1.x NNN ..." from the first bytes of a response.
pub fn parse_status_line(buf: &[u8]) -> Option<u16> {
    let line = buf.split(|b| *b == b'\n').next()?;
    let s = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    let rest = s.strip_prefix("HTTP/1.")?;
    let mut it = rest.splitn(3, ' ');
    let _minor = it.next()?;
    let code: u16 = it.next()?.parse().ok()?;
    (100..=599).contains(&code).then_some(code)
}

fn run_http(host: &str, port: u16, pq: &str, timeout: Duration) -> Outcome {
    let start = Instant::now();
    let deadline = start + timeout;
    let mut stream = match connect(host, port, timeout) {
        Ok(s) => s,
        Err(e) => {
            return Outcome {
                status: Status::NotReachable,
                latency: None,
                message: Some(io_msg(&e)),
            }
        }
    };
    let left = |d: Instant| {
        d.saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    };
    let _ = stream.set_write_timeout(Some(left(deadline)));
    let req = format!(
        "GET {pq} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: systui-healthcheck\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    if let Err(e) = stream.write_all(req.as_bytes()) {
        return Outcome {
            status: Status::NotReachable,
            latency: None,
            message: Some(io_msg(&e)),
        };
    }
    let mut buf = [0u8; 512];
    let mut got = 0usize;
    loop {
        if Instant::now() >= deadline {
            return Outcome {
                status: Status::NotReachable,
                latency: None,
                message: Some("timed out waiting for a response".into()),
            };
        }
        let _ = stream.set_read_timeout(Some(left(deadline)));
        match stream.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => {
                got += n;
                if buf[..got].contains(&b'\n') || got == buf.len() {
                    break;
                }
            }
            Err(e) => {
                return Outcome {
                    status: Status::NotReachable,
                    latency: None,
                    message: Some(io_msg(&e)),
                }
            }
        }
    }
    let latency = Some(start.elapsed());
    match parse_status_line(&buf[..got]) {
        Some(c) if (200..400).contains(&c) => Outcome {
            status: Status::Healthy,
            latency,
            message: None,
        },
        Some(c) => Outcome {
            status: Status::HttpResponded(c),
            latency,
            message: Some(format!("server answered with status {c}")),
        },
        None => Outcome {
            status: Status::Error,
            latency,
            message: Some("port answered but did not speak HTTP".into()),
        },
    }
}

fn run_process(name: &str) -> Outcome {
    let t = Instant::now();
    let Ok(mut sys) = sysinfo::System::new() else {
        return Outcome {
            status: Status::Error,
            latency: None,
            message: Some("cannot read the process list".into()),
        };
    };
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
    );
    let want = name.to_ascii_lowercase();
    let want = want.strip_suffix(".exe").unwrap_or(&want).to_string();
    let found = sys.processes().values().any(|p| {
        if p.thread_kind() == Some(sysinfo::ThreadKind::Userland) {
            return false; // a thread is not a process
        }
        let comm = p.name().to_string_lossy().to_string();
        // Linux truncates comm to 15 chars: recover the real name (exe/argv), and also accept a
        // configured name whose first 15 characters equal the truncated comm.
        let full = crate::collectors::system::resolve_long_name(p.pid().as_u32(), &comm, p.exe());
        [comm, full].iter().any(|n| {
            let n = n.to_ascii_lowercase();
            let n = n.strip_suffix(".exe").unwrap_or(&n);
            n == want
                || (cfg!(target_os = "linux")
                    && n.chars().count() == crate::collectors::system::COMM_MAX
                    && want.chars().count() > crate::collectors::system::COMM_MAX
                    && want.starts_with(n))
        })
    });
    if found {
        Outcome {
            status: Status::Healthy,
            latency: Some(t.elapsed()),
            message: Some("process is running".into()),
        }
    } else {
        Outcome {
            status: Status::NotRunning,
            latency: None,
            message: Some("no such process".into()),
        }
    }
}

/// Windows `sc query` / systemd `systemctl show`: read-only.
fn run_service(name: &str, timeout: Duration) -> Outcome {
    let t = Instant::now();
    let r = if cfg!(windows) {
        exec::run("sc.exe", &["query", name], timeout)
    } else {
        exec::run(
            "systemctl",
            &["show", "-p", "ActiveState", "--value", name],
            timeout,
        )
    };
    match r {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let active = if cfg!(windows) {
                let st = text
                    .lines()
                    .find(|l| l.trim_start().starts_with("STATE"))
                    .unwrap_or("");
                (
                    st.contains("RUNNING"),
                    st.split_whitespace().last().unwrap_or("?").to_string(),
                )
            } else {
                let v = text.trim().to_string();
                (v == "active", v)
            };
            if active.0 {
                Outcome {
                    status: Status::Healthy,
                    latency: Some(t.elapsed()),
                    message: Some("service is running".into()),
                }
            } else {
                Outcome {
                    status: Status::NotRunning,
                    latency: None,
                    message: Some(format!(
                        "service state: {}",
                        sanitize(&active.1).to_lowercase()
                    )),
                }
            }
        }
        Err(ExecError::NotFound(p)) => Outcome {
            status: Status::Error,
            latency: None,
            message: Some(format!("`{p}` is not available")),
        },
        Err(ExecError::Timeout(..)) => Outcome {
            status: Status::Error,
            latency: None,
            message: Some("service query timed out".into()),
        },
        Err(e) => Outcome {
            status: Status::Error,
            latency: None,
            message: Some(sanitize(&e.to_string())),
        },
    }
}

pub fn run_check(spec: &CheckSpec) -> Outcome {
    match &spec.target {
        Target::Tcp { host, port } => run_tcp(host, *port, spec.timeout),
        Target::Http {
            host,
            port,
            path_and_query,
            ..
        } => run_http(host, *port, path_and_query, spec.timeout),
        Target::Process(p) => run_process(p),
        Target::Service(s) => run_service(s, spec.timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    #[cfg(target_os = "linux")]
    use std::path::Path;

    #[test]
    fn hosts_are_loopback_only_by_default() {
        for ok in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.9.9.9",
            "::1",
            "[::1]",
        ] {
            assert!(host_allowed(ok, false).is_ok(), "{ok}");
        }
        for bad in [
            "example.com",
            "8.8.8.8",
            "192.168.1.5",
            "10.0.0.1",
            "0.0.0.0",
            "evil.localhost.com",
            "169.254.1.1",
            "2001:db8::1",
            "",
        ] {
            assert!(host_allowed(bad, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn lan_requires_explicit_opt_in_and_public_is_never_allowed() {
        for ok in [
            "192.168.1.5",
            "10.1.2.3",
            "172.16.0.9",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(host_allowed(ok, true).is_ok(), "{ok}");
        }
        for bad in [
            "8.8.8.8",
            "1.1.1.1",
            "example.com",
            "2001:4860:4860::8888",
            "172.32.0.1",
        ] {
            assert!(host_allowed(bad, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn url_parsing_and_display_redaction() {
        let (h, p, pq, d) = parse_http_url("http://127.0.0.1:7141/health", false).unwrap();
        assert_eq!(
            (h.as_str(), p, pq.as_str(), d.as_str()),
            ("127.0.0.1", 7141, "/health", "http://127.0.0.1:7141/health")
        );
        let (_, p, pq, d) = parse_http_url("http://localhost/x?token=SECRET&a=1", false).unwrap();
        assert_eq!((p, pq.as_str()), (80, "/x?token=SECRET&a=1"));
        assert_eq!(d, "http://localhost:80/x?…");
        assert!(!d.contains("SECRET"), "query never displayed");
        let (_, _, pq, _) = parse_http_url("http://localhost:9000", false).unwrap();
        assert_eq!(pq, "/");
        let (h, p, _, d) = parse_http_url("http://[::1]:8080/h", false).unwrap();
        assert_eq!(
            (h.as_str(), p, d.as_str()),
            ("::1", 8080, "http://[::1]:8080/h")
        );
        let (_, _, pq, _) = parse_http_url("http://localhost?a=1", false).unwrap();
        assert_eq!(pq, "/?a=1");
    }

    #[test]
    fn bad_urls_are_rejected_with_reasons() {
        let e = |u: &str| parse_http_url(u, false).unwrap_err();
        assert!(e("https://localhost/").contains("TLS"));
        assert!(e("ftp://localhost/").contains("http://"));
        assert!(e("http://user:pw@localhost/").contains("credentials"));
        assert!(e("http://example.com/health").contains("not allowed"));
        assert!(e("http://localhost:99999/").contains("port"));
        assert!(e("http://localhost:abc/").contains("port"));
        assert!(e("http:///x").contains("host"));
        assert!(e("http://localhost/a b").contains("spaces"));
        assert!(
            e("http://localhost/a\r\nHost: evil").contains("control")
                || e("http://localhost/a\r\nHost: evil").contains("spaces")
        );
        assert!(e(&format!("http://localhost/{}", "a".repeat(600))).contains("too long"));
    }

    #[test]
    fn status_line_parsing() {
        assert_eq!(
            parse_status_line(b"HTTP/1.1 200 OK\r\nServer: x\r\n"),
            Some(200)
        );
        assert_eq!(
            parse_status_line(b"HTTP/1.0 503 Service Unavailable\r\n"),
            Some(503)
        );
        assert_eq!(parse_status_line(b"HTTP/1.1 200\r\n"), Some(200));
        assert_eq!(parse_status_line(b"SSH-2.0-OpenSSH_9.0\r\n"), None);
        assert_eq!(parse_status_line(b"HTTP/1.1 abc\r\n"), None);
        assert_eq!(parse_status_line(b"HTTP/1.1 99 x\r\n"), None);
        assert_eq!(parse_status_line(b""), None);
        assert_eq!(parse_status_line(&[0xff, 0xfe, 0x00]), None);
    }

    fn cfg(kind: CheckKind) -> CheckConfig {
        CheckConfig {
            name: "t".into(),
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn spec_validation_per_type() {
        let mut c = cfg(CheckKind::Tcp);
        assert!(CheckSpec::from_config(&c, false)
            .unwrap_err()
            .contains("host"));
        c.host = Some("127.0.0.1".into());
        assert!(CheckSpec::from_config(&c, false)
            .unwrap_err()
            .contains("port"));
        c.port = Some(80);
        assert_eq!(
            CheckSpec::from_config(&c, false).unwrap().describe(),
            "tcp 127.0.0.1:80"
        );
        c.host = Some("example.com".into());
        assert!(CheckSpec::from_config(&c, false).is_err());

        let mut c = cfg(CheckKind::Service);
        c.service = Some("evil; calc".into());
        assert!(CheckSpec::from_config(&c, false).is_err());
        c.service = Some("nginx".into());
        assert!(CheckSpec::from_config(&c, false).is_ok());

        let mut c = cfg(CheckKind::Process);
        assert!(CheckSpec::from_config(&c, false).is_err());
        c.process = Some("node".into());
        assert_eq!(
            CheckSpec::from_config(&c, false).unwrap().describe(),
            "process node"
        );
    }

    fn serve_once(reply: &'static [u8]) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().take(5) {
                let mut s = s.unwrap();
                let mut b = [0u8; 1024];
                let _ = s.read(&mut b);
                let _ = s.write_all(reply);
            }
        });
        port
    }

    #[test]
    fn tcp_open_is_port_open_and_not_healthy() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let o = run_tcp("127.0.0.1", port, Duration::from_secs(1));
        assert_eq!(o.status, Status::PortOpen);
        assert!(!o.status.is_failure());
        assert!(o.latency.is_some());
    }

    #[test]
    fn closed_port_is_not_reachable() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        }; // listener dropped: port is closed
        let o = run_tcp("127.0.0.1", port, Duration::from_secs(1));
        assert_eq!(o.status, Status::NotReachable);
        assert!(o.status.is_failure());
        assert!(o.message.is_some());
    }

    #[test]
    fn http_statuses_are_distinguished() {
        let ok = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let o = run_http("127.0.0.1", ok, "/health", Duration::from_secs(2));
        assert_eq!(o.status, Status::Healthy);
        let redirect = serve_once(b"HTTP/1.1 302 Found\r\nLocation: /x\r\n\r\n");
        assert_eq!(
            run_http("127.0.0.1", redirect, "/", Duration::from_secs(2)).status,
            Status::Healthy
        );
        let bad = serve_once(b"HTTP/1.1 503 Service Unavailable\r\n\r\n");
        let o = run_http("127.0.0.1", bad, "/", Duration::from_secs(2));
        assert_eq!(o.status, Status::HttpResponded(503));
        assert!(
            o.status.is_failure(),
            "a 503 is a failure, unlike a bare open port"
        );
        let notfound = serve_once(b"HTTP/1.1 404 Not Found\r\n\r\n");
        assert_eq!(
            run_http("127.0.0.1", notfound, "/", Duration::from_secs(2)).status,
            Status::HttpResponded(404)
        );
        let ssh = serve_once(b"SSH-2.0-OpenSSH_9.0\r\n");
        let o = run_http("127.0.0.1", ssh, "/", Duration::from_secs(2));
        assert_eq!(o.status, Status::Error);
        assert!(o.message.unwrap().contains("did not speak HTTP"));
    }

    #[test]
    fn silent_server_times_out_promptly() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let _keep = std::thread::spawn(move || {
            let _held: Vec<_> = l.incoming().take(1).collect(); // accept, never reply
            std::thread::sleep(Duration::from_secs(3));
        });
        let t = Instant::now();
        let o = run_http("127.0.0.1", port, "/", Duration::from_millis(400));
        assert_eq!(o.status, Status::NotReachable);
        assert!(
            t.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t.elapsed()
        );
    }

    #[test]
    fn request_is_a_plain_get_with_no_credentials() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = vec![0u8; 2048];
            let n = s.read(&mut b).unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
            String::from_utf8_lossy(&b[..n]).to_string()
        });
        run_http("127.0.0.1", port, "/health?x=1", Duration::from_secs(2));
        let req = h.join().unwrap();
        assert!(req.starts_with("GET /health?x=1 HTTP/1.1\r\n"), "{req}");
        assert!(
            !req.to_lowercase().contains("authorization") && !req.to_lowercase().contains("cookie")
        );
    }

    #[test]
    fn process_check_finds_this_test_binary_and_rejects_a_missing_one() {
        let me = std::env::current_exe().unwrap();
        let name = me.file_stem().unwrap().to_string_lossy().to_string();
        assert_eq!(run_process(&name).status, Status::Healthy);
        assert_eq!(
            run_process(&name.to_uppercase()).status,
            Status::Healthy,
            "case-insensitive"
        );
        let o = run_process("definitely-not-running-xyz");
        assert_eq!(o.status, Status::NotRunning);
    }

    #[cfg(windows)]
    #[test]
    fn windows_service_query_distinguishes_running_and_missing() {
        // EventLog always runs on Windows.
        assert_eq!(
            run_service("EventLog", Duration::from_secs(10)).status,
            Status::Healthy
        );
        let o = run_service("definitely-no-such-service-xyz", Duration::from_secs(10));
        assert_eq!(o.status, Status::Error);
    }

    /// Real systemd (only when this machine actually booted with systemd as PID 1).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_systemctl_service_check_distinguishes_active_inactive_and_unknown() {
        if !Path::new("/run/systemd/system").exists() {
            eprintln!("systemd is not running here: skipped");
            return;
        }
        let t = Duration::from_secs(10);
        // systemd-journald is always running under systemd
        assert_eq!(run_service("systemd-journald", t).status, Status::Healthy);
        // a unit that does not exist reads as "inactive": reported as not running, never as healthy
        let o = run_service("definitely-no-such-unit-xyz", t);
        assert_eq!(o.status, Status::NotRunning, "{o:?}");
        assert!(o.message.unwrap().contains("inactive"));
    }

    #[test]
    fn status_labels_and_failure_classes() {
        assert_eq!(Status::HttpResponded(500).label(), "http 500");
        assert!(
            !Status::Healthy.is_failure()
                && !Status::Unknown.is_failure()
                && !Status::PortOpen.is_failure()
        );
        assert!(Status::NotRunning.is_failure() && Status::Error.is_failure());
    }
}

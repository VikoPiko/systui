//! Listening sockets and their owning process, collected ON DEMAND only (the `L`
//! key in the Network tab) with the OS's own read-only tools:
//! `netstat -ano` (Windows) / `ss -H -ltnup` (Linux). This is not a packet
//! sniffer: it lists sockets the OS reports as listening/bound, nothing more.

use crate::util::exec::{self, ExecError};
use crate::util::text::sanitize;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(8);
/// `netstat -ano` also lists every established connection and can be large.
const NETSTAT_CAP: usize = 4 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub proto: &'static str,
    /// Local address as printed by the OS, e.g. `127.0.0.1`, `[::]`, `*`.
    pub addr: String,
    pub port: u16,
    /// Owning process id. `None` when the OS does not say (e.g. no permission on Linux).
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listeners {
    pub items: Vec<Listener>,
    /// The tool's output hit the size cap, so the list may be incomplete.
    pub truncated: bool,
}

/// Split `host:port` / `[v6]:port` / `*:port` at the LAST colon.
fn split_endpoint(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    let port: u16 = p.parse().ok()?;
    let host = sanitize(h);
    (!host.is_empty() && host.len() <= 64).then_some((host, port))
}

/// Windows `netstat -ano`. TCP rows count as listening when the foreign endpoint is
/// port 0 (the state word is localized, so it is not used). Every UDP row is a bound socket.
pub fn parse_netstat(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(&first) = t.first() else { continue };
        let first = first.to_ascii_uppercase();
        match first.as_str() {
            "TCP" if t.len() >= 5 => {
                let (Some((addr, port)), Some((_, fport))) =
                    (split_endpoint(t[1]), split_endpoint(t[2]))
                else {
                    continue;
                };
                if fport != 0 {
                    continue;
                }
                out.push(Listener {
                    proto: "tcp",
                    addr,
                    port,
                    pid: t[t.len() - 1].parse().ok(),
                });
            }
            "UDP" if t.len() >= 4 => {
                let Some((addr, port)) = split_endpoint(t[1]) else {
                    continue;
                };
                out.push(Listener {
                    proto: "udp",
                    addr,
                    port,
                    pid: t[t.len() - 1].parse().ok(),
                });
            }
            _ => {}
        }
    }
    out
}

/// Linux `ss -H -ltnup`: `tcp LISTEN 0 4096 127.0.0.1:631 0.0.0.0:* users:(("cupsd",pid=812,fd=7))`
pub fn parse_ss(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() < 5 {
            continue;
        }
        let proto = match t[0] {
            "tcp" => "tcp",
            "udp" => "udp",
            _ => continue,
        };
        let Some((addr, port)) = split_endpoint(t[4]) else {
            continue;
        };
        let pid = line
            .split("pid=")
            .nth(1)
            .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|d| d.parse().ok());
        out.push(Listener {
            proto,
            addr,
            port,
            pid,
        });
    }
    out
}

/// Sort by port then protocol, drop exact duplicates.
fn finish(mut v: Vec<Listener>) -> Vec<Listener> {
    v.sort_by(|a, b| (a.port, a.proto, &a.addr, a.pid).cmp(&(b.port, b.proto, &b.addr, b.pid)));
    v.dedup();
    v
}

pub fn collect() -> Result<Listeners, String> {
    let explain = |program: &str, e: ExecError| match e {
        ExecError::NotFound(_) => format!("`{program}` is not available on this machine"),
        ExecError::Timeout(..) => format!("`{program}` did not finish in time"),
        other => other.to_string(),
    };
    if cfg!(windows) {
        let o = exec::run_capped("netstat.exe", &["-ano"], TIMEOUT, NETSTAT_CAP)
            .map_err(|e| explain("netstat", e))?;
        Ok(Listeners {
            items: finish(parse_netstat(&String::from_utf8_lossy(&o.stdout))),
            truncated: o.truncated,
        })
    } else if cfg!(target_os = "linux") {
        let o = exec::run("ss", &["-H", "-ltnup"], TIMEOUT).map_err(|e| explain("ss", e))?;
        Ok(Listeners {
            items: finish(parse_ss(&String::from_utf8_lossy(&o.stdout))),
            truncated: o.truncated,
        })
    } else {
        Err("listening-port view is only implemented for Windows and Linux".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETSTAT: &str = "\r\n\
Active Connections\r\n\
\r\n\
  Proto  Local Address          Foreign Address        State           PID\r\n\
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1204\r\n\
  TCP    127.0.0.1:5432         0.0.0.0:0              ABH\u{d6}REN         7788\r\n\
  TCP    192.168.1.5:50123      52.1.2.3:443           ESTABLISHED     9001\r\n\
  TCP    [::]:445               [::]:0                 LISTENING       4\r\n\
  TCP    [::1]:7141             [::]:0                 LISTENING       4242\r\n\
  UDP    0.0.0.0:123            *:*                                    2468\r\n\
  UDP    [::]:5353              *:*                                    1980\r\n\
  garbage line\r\n\
  TCP    notanendpoint          0.0.0.0:0              LISTENING       5\r\n";

    #[test]
    fn netstat_parsing_keeps_listeners_and_udp_drops_established_and_junk() {
        let v = parse_netstat(NETSTAT);
        let ports: Vec<(&str, u16, Option<u32>)> =
            v.iter().map(|l| (l.proto, l.port, l.pid)).collect();
        assert_eq!(
            ports,
            [
                ("tcp", 135, Some(1204)),
                ("tcp", 5432, Some(7788)), // localized state word still recognised
                ("tcp", 445, Some(4)),
                ("tcp", 7141, Some(4242)),
                ("udp", 123, Some(2468)),
                ("udp", 5353, Some(1980)),
            ]
        );
        assert!(
            !v.iter().any(|l| l.port == 50123),
            "established connection excluded"
        );
        assert_eq!(v[2].addr, "[::]");
        assert_eq!(v[3].addr, "[::1]");
    }

    const SS: &str = "tcp   LISTEN 0 4096 127.0.0.1:631   0.0.0.0:*  users:((\"cupsd\",pid=812,fd=7))\n\
tcp   LISTEN 0 128  0.0.0.0:22      0.0.0.0:*  users:((\"sshd\",pid=700,fd=3),(\"sshd\",pid=701,fd=3))\n\
tcp   LISTEN 0 511  [::]:8080       [::]:*\n\
udp   UNCONN 0 0    *:5353          *:*        users:((\"avahi-daemon\",pid=655,fd=12))\n\
nl    UNCONN 0 0    0:0             *:*\n\
short\n";

    #[test]
    fn ss_parsing_extracts_pid_and_tolerates_missing_users() {
        let v = parse_ss(SS);
        assert_eq!(v.len(), 4);
        assert_eq!((v[0].port, v[0].pid), (631, Some(812)));
        assert_eq!((v[1].port, v[1].pid), (22, Some(700)), "first owner");
        assert_eq!(
            (v[2].port, v[2].pid, v[2].addr.as_str()),
            (8080, None, "[::]")
        );
        assert_eq!(
            (v[3].proto, v[3].addr.as_str(), v[3].port),
            ("udp", "*", 5353)
        );
    }

    #[test]
    fn endpoint_splitting_and_sanitising() {
        assert_eq!(
            split_endpoint("127.0.0.1:80"),
            Some(("127.0.0.1".into(), 80))
        );
        assert_eq!(split_endpoint("[::1]:443"), Some(("[::1]".into(), 443)));
        assert_eq!(split_endpoint("*:0"), Some(("*".into(), 0)));
        assert_eq!(split_endpoint("nocolon"), None);
        assert_eq!(split_endpoint("a:99999"), None);
        assert_eq!(split_endpoint(":80"), None);
        assert!(!split_endpoint("ev\x1bil:80").unwrap().0.contains('\x1b'));
    }

    #[test]
    fn finish_sorts_and_dedups() {
        let l = |p, port| Listener {
            proto: p,
            addr: "0.0.0.0".into(),
            port,
            pid: Some(1),
        };
        let v = finish(vec![l("udp", 53), l("tcp", 80), l("tcp", 22), l("tcp", 80)]);
        assert_eq!(
            v.iter().map(|x| (x.proto, x.port)).collect::<Vec<_>>(),
            [("tcp", 22), ("udp", 53), ("tcp", 80)]
        );
    }

    /// Real OS: open a socket in this process and make sure the OS tool reports us as its owner.
    #[test]
    fn real_listener_is_found_with_this_process_as_owner() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        match collect() {
            Ok(r) => {
                let hit = r.items.iter().find(|x| x.proto == "tcp" && x.port == port);
                let hit = hit.unwrap_or_else(|| {
                    panic!("port {port} not listed (truncated={})", r.truncated)
                });
                assert_eq!(hit.pid, Some(std::process::id()), "{hit:?}");
                assert!(hit.addr == "127.0.0.1");
            }
            Err(e) => {
                // Acceptable only when the tool is missing (e.g. minimal Linux container).
                #[cfg(windows)]
                panic!("netstat must work on Windows: {e}");
                #[cfg(not(windows))]
                assert!(
                    e.contains("not available") || e.contains("only implemented"),
                    "{e}"
                );
            }
        }
    }
}

//! Minimal blocking HTTP/1.1 GET for LOCAL endpoints (plain http, no TLS, no redirects).
//! Used by the AI module to read status JSON from a local inference server.
//! The caller must already have restricted the host to loopback.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Longest we wait for a loopback TCP connect to be accepted.
const CONNECT_LIMIT: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Nothing accepted the connection: refused/reset, or (Windows retries SYNs to a closed
    /// loopback port instead of failing fast) the connect itself timed out.
    Refused,
    TimedOut,
    /// Connected but the reply was not usable HTTP, or exceeded the size cap.
    Bad(String),
}

impl HttpError {
    pub fn describe(&self) -> String {
        match self {
            HttpError::Refused => "connection refused".into(),
            HttpError::TimedOut => "timed out".into(),
            HttpError::Bad(m) => m.clone(),
        }
    }
}

fn classify(e: &std::io::Error) -> HttpError {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused | ConnectionReset | ConnectionAborted | NotFound => HttpError::Refused,
        TimedOut | WouldBlock => HttpError::TimedOut,
        k => HttpError::Bad(format!("{k}")),
    }
}

/// GET `path` and return `(status, body)`. The whole exchange is bounded by `timeout`
/// and the body by `max_body` bytes (larger replies are an error, never truncated silently).
pub fn get(
    host: &str,
    port: u16,
    path: &str,
    timeout: Duration,
    max_body: usize,
) -> Result<(u16, Vec<u8>), HttpError> {
    let deadline = Instant::now() + timeout;
    let left = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1))
    };
    let h = host.trim_start_matches('[').trim_end_matches(']');
    let addr = (h, port)
        .to_socket_addrs()
        .map_err(|e| classify(&e))?
        .next()
        .ok_or(HttpError::Refused)?;
    // A live loopback listener accepts at once; a slow connect means nobody is listening.
    let connect_for = timeout.min(CONNECT_LIMIT);
    let mut s = TcpStream::connect_timeout(&addr, connect_for).map_err(|e| match classify(&e) {
        HttpError::TimedOut => HttpError::Refused,
        other => other,
    })?;
    let _ = s.set_write_timeout(Some(left()));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: systui\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| classify(&e))?;
    let cap = max_body.saturating_add(16 * 1024); // headers + body
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return Err(HttpError::TimedOut);
        }
        let _ = s.set_read_timeout(Some(left()));
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > cap {
                    return Err(HttpError::Bad("response too large".into()));
                }
            }
            Err(e) => return Err(classify(&e)),
        }
    }
    parse_response(&buf, max_body)
}

/// Split a full `Connection: close` response into status and (de-chunked) body.
pub fn parse_response(buf: &[u8], max_body: usize) -> Result<(u16, Vec<u8>), HttpError> {
    let bad = |m: &str| HttpError::Bad(m.to_string());
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("incomplete HTTP response"))?;
    let head = std::str::from_utf8(&buf[..split]).map_err(|_| bad("non-text HTTP headers"))?;
    let mut lines = head.split("\r\n");
    let status = crate::health::parse_status_line(lines.next().unwrap_or("").as_bytes())
        .ok_or_else(|| bad("not an HTTP response"))?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let body = &buf[split + 4..];
    let out = if chunked {
        dechunk(body).ok_or_else(|| bad("malformed chunked body"))?
    } else {
        body.to_vec()
    };
    if out.len() > max_body {
        return Err(bad("response too large"));
    }
    Ok((status, out))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let size_txt = std::str::from_utf8(&b[..eol]).ok()?;
        let size_txt = size_txt.split(';').next()?.trim();
        let size = usize::from_str_radix(size_txt, 16).ok()?;
        b = &b[eol + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn serve_once(reply: &'static [u8]) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = l.accept() {
                let mut b = [0u8; 1024];
                let _ = c.read(&mut b);
                let _ = c.write_all(reply);
            }
        });
        port
    }

    #[test]
    fn plain_and_chunked_bodies() {
        let p = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        let (s, b) = get("127.0.0.1", p, "/", Duration::from_secs(2), 1024).unwrap();
        assert_eq!((s, b.as_slice()), (200, &b"{}"[..]));
        let p = serve_once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        );
        let (_, b) = get("127.0.0.1", p, "/", Duration::from_secs(2), 1024).unwrap();
        assert_eq!(b, b"abcde");
    }

    #[test]
    fn status_codes_are_passed_through() {
        let p = serve_once(b"HTTP/1.1 404 Not Found\r\n\r\nnope");
        let (s, _) = get("127.0.0.1", p, "/x", Duration::from_secs(2), 1024).unwrap();
        assert_eq!(s, 404);
    }

    #[test]
    fn oversized_and_garbage_replies_are_errors() {
        let p = serve_once(b"HTTP/1.1 200 OK\r\n\r\n0123456789");
        assert!(matches!(
            get("127.0.0.1", p, "/", Duration::from_secs(2), 4),
            Err(HttpError::Bad(_))
        ));
        let p = serve_once(b"SSH-2.0-OpenSSH\r\n\r\n");
        assert!(matches!(
            get("127.0.0.1", p, "/", Duration::from_secs(2), 64),
            Err(HttpError::Bad(_))
        ));
        assert!(parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
            9
        )
        .is_err());
    }

    #[test]
    fn closed_port_is_refused_and_silent_server_times_out() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert_eq!(
            get("127.0.0.1", port, "/", Duration::from_secs(1), 64),
            Err(HttpError::Refused)
        );
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let _keep = std::thread::spawn(move || {
            let c = l.accept();
            std::thread::sleep(Duration::from_secs(2));
            drop(c);
        });
        assert_eq!(
            get("127.0.0.1", port, "/", Duration::from_millis(300), 64),
            Err(HttpError::TimedOut)
        );
    }
}

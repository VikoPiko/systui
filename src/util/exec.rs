//! Run an external program safely: fixed program + argument array (never a
//! shell), hard deadline, bounded output. Output is untrusted data.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Cap on captured stdout and on captured stderr (each).
pub const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("`{0}` was not found")]
    NotFound(String),
    #[error("could not start `{0}`: {1}")]
    Spawn(String, String),
    #[error("`{0}` did not finish within {1} ms and was killed")]
    Timeout(String, u128),
    #[error("`{program}` failed ({code}): {stderr}")]
    Failed {
        program: String,
        code: String,
        stderr: String,
    },
}

#[derive(Debug)]
pub struct Output {
    pub stdout: Vec<u8>,
    /// Captured (capped) stderr; only meaningful on success for tools that log there.
    pub stderr: Vec<u8>,
    /// True if stdout hit the cap and was cut.
    pub truncated: bool,
}

/// Output being collected by a reader thread. Shared so the caller can take whatever
/// has arrived even if the pipe never reaches EOF (a grandchild may keep it open).
#[derive(Default)]
struct Capture {
    data: Vec<u8>,
    truncated: bool,
    done: bool,
}

type Shared = Arc<Mutex<Capture>>;

fn spawn_reader<R: Read + Send + 'static>(mut r: R, cap: usize) -> Shared {
    let shared: Shared = Arc::default();
    let out = shared.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let Ok(mut c) = out.lock() else { return };
                    if c.data.len() < cap {
                        let take = n.min(cap - c.data.len());
                        c.data.extend_from_slice(&buf[..take]);
                        c.truncated |= take < n;
                    } else {
                        c.truncated = true; // keep draining so the child never blocks
                    }
                }
            }
        }
        if let Ok(mut c) = out.lock() {
            c.done = true;
        }
    });
    shared
}

/// Wait up to `grace` for the reader to hit EOF, then take what has arrived.
fn take_capture(c: &Shared, grace: Duration) -> (Vec<u8>, bool) {
    let until = Instant::now() + grace;
    loop {
        if let Ok(mut g) = c.lock() {
            if g.done || Instant::now() >= until {
                return (std::mem::take(&mut g.data), g.truncated);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// After the child exited, how long to wait for its output pipes to close. A
/// grandchild that inherited them (a daemonised helper) must not stall us.
const PIPE_GRACE: Duration = Duration::from_millis(1500);

/// Run `program args...`, killing it after `timeout`. Succeeds only on exit code 0.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Output, ExecError> {
    run_capped(program, args, timeout, MAX_OUTPUT)
}

/// Like [`run`] with an explicit stdout cap (still bounded; stderr keeps the default cap).
pub fn run_capped(
    program: &str,
    args: &[&str],
    timeout: Duration,
    stdout_cap: usize,
) -> Result<Output, ExecError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: never flash a console over the TUI
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // own group, so a timeout can kill the whole tree
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ExecError::NotFound(program.to_string())
        } else {
            ExecError::Spawn(program.to_string(), e.to_string())
        }
    })?;
    let (Some(child_out), Some(child_err)) = (child.stdout.take(), child.stderr.take()) else {
        kill_tree(&mut child);
        let _ = child.wait();
        return Err(ExecError::Spawn(
            program.to_string(),
            "could not capture the program's output".into(),
        ));
    };
    let out = spawn_reader(child_out, stdout_cap);
    let err = spawn_reader(child_err, MAX_OUTPUT);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                kill_tree(&mut child);
                let _ = child.wait();
                // Give the pipes a moment to close; never wait on stragglers.
                let _ = take_capture(&out, Duration::from_millis(300));
                let _ = take_capture(&err, Duration::from_millis(100));
                return Err(ExecError::Timeout(program.to_string(), timeout.as_millis()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(ExecError::Spawn(program.to_string(), e.to_string())),
        }
    };
    let (stdout, truncated) = take_capture(&out, PIPE_GRACE);
    let (stderr, _) = take_capture(&err, PIPE_GRACE);
    if status.success() {
        Ok(Output {
            stdout,
            stderr,
            truncated,
        })
    } else {
        let msg = crate::util::text::sanitize(decode_maybe_utf16(&stderr).trim());
        Err(ExecError::Failed {
            program: program.to_string(),
            code: status.code().map_or("signal".into(), |c| c.to_string()),
            stderr: msg.chars().take(300).collect(),
        })
    }
}

/// Kill the child and, on Unix, everything in its process group.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // SAFETY: killpg only sends a signal; the pid is our own child's, which is also the
        // id of the process group we created for it with `process_group(0)`.
        unsafe {
            libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Decode `wsl.exe` style output: UTF-16LE (with or without BOM); falls back to UTF-8.
pub fn decode_maybe_utf16(bytes: &[u8]) -> String {
    let looks16 = bytes.len() >= 2
        && (bytes[..2] == [0xFF, 0xFE]
            || bytes.iter().skip(1).step_by(2).take(16).all(|&b| b == 0));
    if looks16 {
        let body = if bytes[..2] == [0xFF, 0xFE] {
            &bytes[2..]
        } else {
            bytes
        };
        let (pairs, _odd_trailing_byte) = body.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            (
                "powershell",
                vec!["-NoProfile".into(), "-Command".into(), script.into()],
            )
        } else {
            ("sh", vec!["-c".into(), script.into()])
        }
    }

    fn run_sh(script: &str, t: Duration) -> Result<Output, ExecError> {
        let (p, a) = sh(script);
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        run(p, &a, t)
    }

    #[test]
    fn captures_output() {
        let o = run_sh("echo hello", Duration::from_secs(20)).unwrap();
        assert!(String::from_utf8_lossy(&o.stdout).contains("hello"));
        assert!(!o.truncated);
    }

    #[test]
    fn hung_child_is_killed_at_the_deadline() {
        let script = if cfg!(windows) {
            "Start-Sleep -Seconds 60"
        } else {
            "sleep 60"
        };
        let t = Instant::now();
        let e = run_sh(script, Duration::from_millis(700)).unwrap_err();
        assert!(matches!(e, ExecError::Timeout(..)), "{e:?}");
        assert!(
            t.elapsed() < Duration::from_secs(15),
            "returned promptly: {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn nonzero_exit_is_an_error_with_stderr() {
        let script = if cfg!(windows) {
            "[Console]::Error.WriteLine('boom'); exit 3"
        } else {
            "echo boom >&2; exit 3"
        };
        match run_sh(script, Duration::from_secs(20)).unwrap_err() {
            ExecError::Failed { code, stderr, .. } => {
                assert_eq!(code, "3");
                assert!(stderr.contains("boom"));
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn missing_program_is_not_found() {
        assert!(matches!(
            run(
                "definitely-not-a-real-program-xyz",
                &[],
                Duration::from_secs(1)
            ),
            Err(ExecError::NotFound(_))
        ));
    }

    #[test]
    fn huge_output_is_capped_not_buffered() {
        let script = if cfg!(windows) {
            "1..40000 | ForEach-Object { 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' }"
        } else {
            "yes xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx | head -n 40000"
        };
        let o = run_sh(script, Duration::from_secs(60)).unwrap();
        assert!(o.stdout.len() <= MAX_OUTPUT);
        assert!(o.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn args_are_not_interpreted_by_a_shell() {
        // A metacharacter-laden argument must reach the program verbatim.
        let (p, pre): (&str, Vec<&str>) = if cfg!(windows) {
            ("cmd", vec!["/C", "echo"])
        } else {
            ("echo", vec![])
        };
        let mut args = pre;
        args.push("a&b;c|d");
        let o = run(p, &args, Duration::from_secs(10)).unwrap();
        assert!(String::from_utf8_lossy(&o.stdout).contains("a&b;c|d"));
    }

    #[test]
    fn utf16_decoding() {
        let s = "  NAME  STATE\r\n* Ubuntu  Running\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode_maybe_utf16(&bytes), s);
        let no_bom: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(decode_maybe_utf16(&no_bom), s);
        assert_eq!(decode_maybe_utf16(b"plain utf8"), "plain utf8");
    }
}

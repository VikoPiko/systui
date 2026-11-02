//! Windows-host WSL view (read-only): distro list via `wsl.exe --list --verbose`
//! and the HOST file size of explicitly configured `.vhdx` files.
//!
//! Never starts a distribution, never scans the disk for VHDX files, never
//! compacts/shuts down anything. A `.vhdx` file size is the space the virtual
//! disk occupies on the host, NOT how full the guest filesystem is.

use super::ModStatus;
use crate::util::exec::{self, decode_maybe_utf16, ExecError};
use crate::util::text::sanitize;
use std::time::Duration;

pub const CADENCE: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Distro {
    pub name: String,
    /// Shown exactly as `wsl.exe` printed it (may be localized).
    pub state: String,
    pub version: u8,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VhdxSize {
    Bytes(u64),
    NotFound,
    Denied,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vhdx {
    pub path: String,
    pub size: VhdxSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslData {
    pub distros: Vec<Distro>,
    pub vhdx: Vec<Vhdx>,
    pub skipped_lines: usize,
}

/// Parse `wsl --list --verbose`. The header is localized, so it is skipped by
/// position; each row is read from the right: `[*] NAME ... STATE VERSION`.
pub fn parse_list(text: &str) -> (Vec<Distro>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '\u{feff}' || c == '\0' || c == '\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    for line in lines.iter().skip(1) {
        let t = line.trim_start();
        let (default, rest) = match t.strip_prefix('*') {
            Some(r) => (true, r),
            None => (false, t),
        };
        let toks: Vec<&str> = rest.split_whitespace().collect();
        if toks.len() < 3 {
            skipped += 1;
            continue;
        }
        let Ok(version) = toks[toks.len() - 1].parse::<u8>() else {
            skipped += 1;
            continue;
        };
        out.push(Distro {
            name: sanitize(&toks[..toks.len() - 2].join(" ")),
            state: sanitize(toks[toks.len() - 2]),
            version,
            default,
        });
    }
    (out, skipped)
}

pub fn vhdx_info(path: &str) -> Vhdx {
    let size = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => VhdxSize::Bytes(m.len()),
        Ok(_) => VhdxSize::Error("not a regular file".into()),
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => VhdxSize::NotFound,
            std::io::ErrorKind::PermissionDenied => VhdxSize::Denied,
            _ => VhdxSize::Error(e.to_string()),
        },
    };
    Vhdx {
        path: sanitize(path),
        size,
    }
}

pub fn poll(vhdx_paths: &[String]) -> Result<WslData, ModStatus> {
    if !cfg!(windows) {
        return Err(ModStatus::Unavailable(
            "the WSL host view is Windows-only (this is a Linux/macOS build; guest metrics are P2)"
                .into(),
        ));
    }
    let o = exec::run("wsl.exe", &["--list", "--verbose"], TIMEOUT).map_err(|e| match e {
        ExecError::NotFound(_) => {
            ModStatus::Unavailable("`wsl.exe` not found: WSL is not installed".into())
        }
        ExecError::Failed { stderr, .. } => ModStatus::Unavailable(format!(
            "WSL not usable: {}",
            stderr.lines().next().unwrap_or("")
        )),
        ExecError::Timeout(..) => {
            ModStatus::Error("wsl.exe did not answer in time (timeout)".into())
        }
        other => ModStatus::Error(other.to_string()),
    })?;
    let (distros, skipped_lines) = parse_list(&decode_maybe_utf16(&o.stdout));
    Ok(WslData {
        distros,
        vhdx: vhdx_paths.iter().map(|p| vhdx_info(p)).collect(),
        skipped_lines,
    })
}

pub fn spawn(vhdx_paths: Vec<String>) -> super::ModuleHandle<WslData> {
    super::ModuleHandle::spawn("wsl", CADENCE, move |_| poll(&vhdx_paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "  NAME              STATE           VERSION\r\n\
                        * Ubuntu            Running         2\r\n\
                        \x20 docker-desktop    Stopped         2\r\n\
                        \x20 Legacy            Running         1\r\n";

    #[test]
    fn parses_distros() {
        let (d, skipped) = parse_list(LIST);
        assert_eq!(skipped, 0);
        assert_eq!(d.len(), 3);
        assert_eq!(
            d[0],
            Distro {
                name: "Ubuntu".into(),
                state: "Running".into(),
                version: 2,
                default: true
            }
        );
        assert_eq!(d[1].state, "Stopped");
        assert!(!d[1].default);
        assert_eq!(d[2].version, 1);
    }

    #[test]
    fn localized_header_and_states_are_tolerated() {
        let (d, _) = parse_list("  NOM   ÉTAT   VERSION\r\n* Debian  En cours  2\r\n");
        // "En cours" has a space: name is read from the left of STATE+VERSION only,
        // so the name absorbs it. We keep the raw tokens rather than guess.
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].version, 2);
    }

    #[test]
    fn header_only_and_garbage() {
        assert!(parse_list("  NAME  STATE  VERSION\r\n").0.is_empty());
        let (d, s) = parse_list("hdr\nno version here\nUbuntu Running x\n");
        assert!(d.is_empty());
        assert_eq!(s, 2);
        assert!(parse_list("").0.is_empty());
    }

    #[test]
    fn decodes_real_utf16_output() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(LIST.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let (d, _) = parse_list(&decode_maybe_utf16(&bytes));
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn vhdx_sizes_and_failures() {
        let dir = std::env::temp_dir().join(format!("systui-vhdx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("ext4.vhdx");
        std::fs::write(&f, vec![0u8; 12345]).unwrap();
        assert_eq!(vhdx_info(&f.to_string_lossy()).size, VhdxSize::Bytes(12345));
        assert_eq!(
            vhdx_info(&dir.join("nope.vhdx").to_string_lossy()).size,
            VhdxSize::NotFound
        );
        assert!(matches!(
            vhdx_info(&dir.to_string_lossy()).size,
            VhdxSize::Error(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn unavailable_off_windows() {
        assert!(matches!(poll(&[]), Err(ModStatus::Unavailable(_))));
    }
}

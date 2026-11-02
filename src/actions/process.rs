//! Gated process operations. This is the ONLY module allowed to terminate
//! processes. Every termination re-reads the live process and re-validates its
//! identity immediately before signalling.

use crate::domain::ProcessId;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    #[error("refusing to terminate systui itself")]
    SelfProtected,
    #[error("protected system process: {0}")]
    Protected(String),
    #[error("process no longer exists")]
    NotFound,
    #[error(
        "PID {0} now belongs to a different process (identity changed); nothing was signalled"
    )]
    IdentityMismatch(u32),
    #[error("graceful termination is not supported on this platform")]
    GracefulUnsupported,
    #[error("could not signal the process (access denied or it just exited)")]
    Failed,
    #[error("cannot inspect process: {0}")]
    Inspect(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Risk {
    Normal,
    /// Allowed, but the dialog warns prominently.
    Caution(&'static str),
    /// Refused outright.
    Protected(&'static str),
}

/// Best-effort classification; this is NOT a guarantee of safety.
pub fn classify(pid: u32, name: &str) -> Risk {
    let n = name.to_ascii_lowercase();
    if pid == 0 || (cfg!(unix) && pid == 1) || (cfg!(windows) && pid == 4) {
        return Risk::Protected("core OS process");
    }
    const PROTECTED: &[&str] = &[
        "system",
        "system idle process",
        "registry",
        "memory compression",
        "smss.exe",
        "csrss.exe",
        "wininit.exe",
        "winlogon.exe",
        "lsass.exe",
        "services.exe",
        "init",
        "systemd",
        "kthreadd",
    ];
    if PROTECTED.contains(&n.as_str()) {
        return Risk::Protected("critical OS process; ending it can crash or lock the machine");
    }
    const CAUTION: &[&str] = &[
        "svchost.exe",
        "explorer.exe",
        "dwm.exe",
        "sshd",
        "dbus-daemon",
        "gnome-shell",
        "xorg",
        "login",
    ];
    if CAUTION.contains(&n.as_str()) {
        return Risk::Caution("system component; ending it may disrupt your session or services");
    }
    Risk::Normal
}

pub trait ProcessActions {
    /// Whether a graceful (non-forced) termination exists on this platform.
    fn can_graceful(&self) -> bool;
    /// Re-validate `target` against the live OS and terminate it.
    fn terminate(
        &self,
        target: &ProcessId,
        expected_name: &str,
        force: bool,
    ) -> Result<(), ActionError>;
}

/// Real implementation backed by a short-lived `sysinfo::System`.
pub struct OsActions;

impl ProcessActions for OsActions {
    fn can_graceful(&self) -> bool {
        cfg!(unix)
    }

    fn terminate(
        &self,
        target: &ProcessId,
        expected_name: &str,
        force: bool,
    ) -> Result<(), ActionError> {
        if target.pid == std::process::id() {
            return Err(ActionError::SelfProtected);
        }
        let pid = Pid::from_u32(target.pid);
        let mut sys = System::new().map_err(|e| ActionError::Inspect(e.to_string()))?;
        let kind = ProcessRefreshKind::nothing();
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
        let live = sys.process(pid).ok_or(ActionError::NotFound)?;

        let live_start = (live.start_time() != 0).then_some(live.start_time());
        let live_name = live.name().to_string_lossy().to_string();
        let same = match (target.started_at, live_start) {
            (Some(a), Some(b)) => a == b,
            // Missing start time on either side: fall back to name equality.
            _ => live_name.eq_ignore_ascii_case(expected_name),
        };
        if !same || !live_name.eq_ignore_ascii_case(expected_name) {
            return Err(ActionError::IdentityMismatch(target.pid));
        }
        if let Risk::Protected(why) = classify(target.pid, &live_name) {
            return Err(ActionError::Protected(why.to_string()));
        }
        let sent = if force {
            live.kill()
        } else {
            live.kill_with(Signal::Term)
                .ok_or(ActionError::GracefulUnsupported)?
        };
        if sent {
            return Ok(());
        }
        // Distinguish "already gone" from "denied".
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
        if sys.process(pid).is_none() {
            Err(ActionError::NotFound)
        } else {
            Err(ActionError::Failed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(matches!(classify(0, "x"), Risk::Protected(_)));
        assert!(matches!(classify(500, "LSASS.EXE"), Risk::Protected(_)));
        assert!(matches!(classify(500, "csrss.exe"), Risk::Protected(_)));
        assert!(matches!(classify(500, "svchost.exe"), Risk::Caution(_)));
        assert_eq!(classify(500, "notepad.exe"), Risk::Normal);
    }

    #[test]
    fn refuses_self() {
        let me = ProcessId {
            pid: std::process::id(),
            started_at: None,
        };
        assert_eq!(
            OsActions.terminate(&me, "systui", true),
            Err(ActionError::SelfProtected)
        );
    }
}

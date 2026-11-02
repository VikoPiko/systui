//! Gated container operations: the ONLY place that runs a mutating Docker command,
//! and the only commands it can run are `docker stop` and `docker restart` of one
//! container identified by id. No `rm`, `kill`, `exec`, `prune`, nothing else.
//!
//! Every call re-reads the container from Docker immediately before acting and
//! refuses if the id/name no longer match what the user confirmed.

use crate::modules::docker::valid_container_id;
use crate::util::exec::{self, ExecError};
use crate::util::text::sanitize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerAction {
    Stop,
    Restart,
}

impl DockerAction {
    pub fn verb(self) -> &'static str {
        match self {
            DockerAction::Stop => "stop",
            DockerAction::Restart => "restart",
        }
    }
}

const T_INSPECT: Duration = Duration::from_secs(8);
/// `docker stop/restart` wait for the container's grace period (we pass 10 s).
const T_ACT: Duration = Duration::from_secs(60);

/// Check the `docker ps -a --filter id=…` output (`ID\tName\tState` lines) against what
/// the user confirmed.
pub fn check_target(
    ps_out: &str,
    id: &str,
    expected_name: &str,
    action: DockerAction,
) -> Result<(), String> {
    let rows: Vec<Vec<&str>> = ps_out
        .lines()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|f| {
            f.len() == 3 && !f[0].is_empty() && (id.starts_with(f[0]) || f[0].starts_with(id))
        })
        .collect();
    let [row] = rows.as_slice() else {
        return Err(if rows.is_empty() {
            "the container no longer exists".into()
        } else {
            "the id matches more than one container; nothing was changed".into()
        });
    };
    if row[1] != expected_name {
        return Err(format!(
            "container {} is now named `{}`, not `{expected_name}`; nothing was changed",
            &id[..12.min(id.len())],
            sanitize(row[1])
        ));
    }
    if action == DockerAction::Stop && row[2] != "running" {
        return Err(format!(
            "`{expected_name}` is not running (state: {}); nothing to stop",
            sanitize(row[2])
        ));
    }
    Ok(())
}

fn explain(e: ExecError) -> String {
    match e {
        ExecError::NotFound(_) => "`docker` CLI not found on PATH".into(),
        ExecError::Timeout(..) => {
            "docker did not answer in time; the operation may still complete in the engine".into()
        }
        ExecError::Failed { stderr, .. } => sanitize(&stderr),
        other => sanitize(&other.to_string()),
    }
}

/// Stop or restart one container after re-validating its identity.
pub fn run(action: DockerAction, id: &str, expected_name: &str) -> Result<String, String> {
    if !valid_container_id(id) {
        return Err("refusing: not a valid container id".into());
    }
    let ps = exec::run(
        "docker",
        &[
            "ps",
            "-a",
            "--filter",
            &format!("id={id}"),
            "--format",
            "{{.ID}}\t{{.Names}}\t{{.State}}",
        ],
        T_INSPECT,
    )
    .map_err(explain)?;
    check_target(
        &String::from_utf8_lossy(&ps.stdout),
        id,
        expected_name,
        action,
    )?;
    exec::run("docker", &[action.verb(), "--time", "10", id], T_ACT).map_err(explain)?;
    Ok(format!(
        "{} {expected_name}",
        match action {
            DockerAction::Stop => "stopped",
            DockerAction::Restart => "restarted",
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1a2b3c4d5e6f";

    #[test]
    fn matching_running_container_is_accepted() {
        let out = format!("{ID}\tweb-1\trunning\n");
        assert!(check_target(&out, ID, "web-1", DockerAction::Stop).is_ok());
        assert!(check_target(&out, ID, "web-1", DockerAction::Restart).is_ok());
        // a full 64-char id also matches the short form Docker prints
        let full = format!("{ID}{}", "0".repeat(52));
        assert!(check_target(&out, &full, "web-1", DockerAction::Stop).is_ok());
    }

    #[test]
    fn renamed_or_replaced_container_is_refused() {
        let out = format!("{ID}\tsomething-else\trunning\n");
        let e = check_target(&out, ID, "web-1", DockerAction::Stop).unwrap_err();
        assert!(
            e.contains("now named") && e.contains("nothing was changed"),
            "{e}"
        );
    }

    #[test]
    fn vanished_container_is_refused() {
        assert!(check_target("", ID, "web-1", DockerAction::Restart)
            .unwrap_err()
            .contains("no longer exists"));
        let other = "aaaaaaaaaaaa\tweb-1\trunning\n";
        assert!(
            check_target(other, ID, "web-1", DockerAction::Stop).is_err(),
            "different id with the same name"
        );
    }

    #[test]
    fn stopping_a_stopped_container_is_refused_but_restart_is_allowed() {
        let out = format!("{ID}\tjob\texited\n");
        assert!(check_target(&out, ID, "job", DockerAction::Stop)
            .unwrap_err()
            .contains("not running"));
        assert!(check_target(&out, ID, "job", DockerAction::Restart).is_ok());
    }

    #[test]
    fn ambiguous_matches_are_refused() {
        let out = format!("{ID}\tweb-1\trunning\n{ID}\tweb-1\trunning\n");
        assert!(check_target(&out, ID, "web-1", DockerAction::Stop)
            .unwrap_err()
            .contains("more than one"));
    }

    #[test]
    fn bad_ids_never_reach_docker() {
        for bad in [
            "",
            "--help",
            "-f",
            "ABCDEF123456",
            "x; docker rm -f $(docker ps -aq)",
            "short",
        ] {
            assert!(
                run(DockerAction::Stop, bad, "n")
                    .unwrap_err()
                    .contains("not a valid container id"),
                "{bad}"
            );
        }
    }

    #[test]
    fn only_stop_and_restart_exist() {
        assert_eq!(DockerAction::Stop.verb(), "stop");
        assert_eq!(DockerAction::Restart.verb(), "restart");
    }
}

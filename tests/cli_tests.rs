//! Runs the real binary. These work in a non-interactive shell (no TTY).
use std::process::{Command, Stdio};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_systui"));
    c.stdin(Stdio::null());
    c
}

#[test]
fn version_and_help() {
    let o = bin().arg("--version").output().unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("systui "));
    let o = bin().arg("--help").output().unwrap();
    assert!(o.status.success());
    let t = String::from_utf8_lossy(&o.stdout);
    for w in [
        "doctor",
        "snapshot",
        "--config",
        "--refresh-ms",
        "--no-color",
    ] {
        assert!(t.contains(w), "help mentions {w}");
    }
    assert!(!t.contains('\x1b'), "no escape sequences in --help output");
}

#[test]
fn interactive_mode_without_tty_fails_with_actionable_message() {
    let o = bin().output().unwrap();
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(
        e.contains("terminal") && e.contains("snapshot --json"),
        "{e}"
    );
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains('\x1b'),
        "no alt-screen codes"
    );
}

#[test]
fn snapshot_json_is_valid_and_has_real_data() {
    let o = bin()
        .args(["snapshot", "--json", "--top", "3"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("valid JSON on stdout");
    assert_eq!(v["schema_version"], 1);
    assert!(
        v["cpu"]["usage_percent"].is_number(),
        "two samples => CPU available"
    );
    assert!(v["memory"]["total_bytes"].as_u64().unwrap() > 0);
    assert_eq!(v["top_processes"].as_array().unwrap().len(), 3);
    assert!(v["process_count"].as_u64().unwrap() > 5);
    assert!(!o.stdout.contains(&0x1b));
}

#[test]
fn snapshot_output_file_and_no_overwrite() {
    let dir = std::env::temp_dir().join(format!("systui-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("snap.json");
    let ok = bin()
        .args(["snapshot", "--json", "--output"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&std::fs::read(&f).unwrap()).unwrap();
    let again = bin()
        .args(["snapshot", "--json", "--output"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));
    let forced = bin()
        .args(["snapshot", "--json", "--force", "--output"])
        .arg(&f)
        .output()
        .unwrap();
    assert!(forced.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn doctor_reports_and_succeeds() {
    let o = bin().arg("doctor").output().unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{t}");
    for w in [
        "config path",
        "collector cpu",
        "collector memory",
        "collector processes",
        "docker",
    ] {
        assert!(t.contains(w), "doctor output has `{w}`:\n{t}");
    }
    assert!(
        t.contains("health checks") && t.contains("disabled in config"),
        "health state is reported:\n{t}"
    );
    assert!(t.contains("alerts"), "{t}");
    assert!(
        !t.contains("not implemented"),
        "every feature exists now:\n{t}"
    );
}

#[test]
fn doctor_flags_a_broken_config() {
    let dir = std::env::temp_dir().join(format!("systui-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("bad.toml");
    std::fs::write(&f, "[ui]\nrefresh_ms = 0\n").unwrap();
    let o = bin()
        .arg("doctor")
        .arg("--config")
        .arg(&f)
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("refresh_ms"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_refresh_override_is_rejected() {
    let o = bin().args(["--refresh-ms", "99999"]).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("500-5000"));
}

#[test]
fn closed_stdout_pipe_is_not_a_panic() {
    let mut child = bin()
        .arg("doctor")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // reader goes away immediately, like `| head -0`
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn doctor_columns_are_separated() {
    let o = bin().arg("doctor").output().unwrap();
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(!t.contains("ttyfalse") && !t.contains("ttytrue"), "{t}");
}

#[test]
fn completions_exist_for_every_shell_and_cover_the_real_cli() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let o = bin().args(["completions", shell]).output().unwrap();
        assert!(
            o.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let t = String::from_utf8_lossy(&o.stdout);
        assert!(t.len() > 200, "{shell}: suspiciously short");
        // generated from the live clap definition, so every subcommand and flag is present
        for w in [
            "doctor",
            "snapshot",
            "completions",
            "man",
            "refresh-ms",
            "no-color",
        ] {
            assert!(t.contains(w), "{shell} completion misses `{w}`");
        }
        assert!(!t.contains('\x1b'), "{shell}: no escape sequences");
    }
}

#[test]
fn unknown_shell_is_rejected_with_the_valid_choices() {
    let o = bin().args(["completions", "tcsh"]).output().unwrap();
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("bash") && e.contains("powershell"), "{e}");
}

#[test]
fn man_page_is_valid_roff_that_documents_the_commands() {
    let o = bin().arg("man").output().unwrap();
    assert!(o.status.success());
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(t.contains(".TH systui 1"), "{}", &t[..t.len().min(200)]);
    for w in ["SYNOPSIS", "doctor", "snapshot", r"refresh\-ms"] {
        assert!(t.contains(w), "man page misses `{w}`");
    }
    assert!(
        t.contains(env!("CARGO_PKG_VERSION")),
        "version is stamped in"
    );
}

#[test]
fn generators_survive_a_closed_pipe() {
    for args in [vec!["man"], vec!["completions", "bash"]] {
        let mut child = bin()
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        assert!(
            !String::from_utf8_lossy(&out.stderr).contains("panicked"),
            "{args:?}"
        );
    }
}

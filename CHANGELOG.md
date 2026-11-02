# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/). Verification status of every claim lives in
[TASKS.md](TASKS.md); design decisions in [DECISIONS.md](DECISIONS.md).

## [0.1.0] - unreleased

First feature-complete build (milestones M0–M6). Pre-1.0: the config format and the
`snapshot --json` schema (`schema_version: 1`) may still change.

### Added
- **Overview / Processes / Network / Storage** views with real telemetry: CPU (aggregate and
  per core), memory and swap, volumes, per-interface network rates, bounded histories. First
  samples say "warming up"; unreadable values say `denied` or `—`, never `0`.
- **Process explorer:** sort, filter, details, parent/child jump, tree view with subtree totals,
  optional per-process I/O, sanitized export (`x`), and guarded termination (type the PID; identity
  re-validated against PID reuse; critical processes and systui itself refused).
- **Command palette** (`Ctrl+K`, built-in commands only), help overlay, four terminal size tiers,
  dark/light/mono themes, `NO_COLOR`, ASCII fallback.
- **`systui doctor`** and **`systui snapshot --json`** (versioned, redacted, no overwrite without `--force`).
- **Dev tab:** process families (configurable, hedged heuristics), NVIDIA GPU via NVML, Docker
  (containers, stats, `system df` on request, compose project), WSL distro list and `.vhdx` file sizes.
- **Services tab:** local-only health checks (TCP, HTTP status, process, OS service) and low-noise
  alerts (sustain, hysteresis, cool-down); "What changed?" process diff (`w`); listening ports with
  owner (`L`).
- **Stretch features (all off by default):** battery/temperature/throttle sensors, session recording
  to JSONL with size and retention limits (`Ctrl+R`), Docker log preview and confirmed stop/restart,
  consent-based read-only SSH checks of allowlisted hosts.
- Installers (`scripts/install.ps1`, `scripts/install.sh`), a pseudo-terminal smoke test
  (`scripts/pty_smoke.py`), benchmarks (`examples/bench.rs`) and a screenshot generator
  (`examples/screenshot.rs`).

### Performance
- The collector refreshes only what the visible tab shows. On the Windows test machine (≈330
  processes, 1 s refresh): Storage/Services ≈ 0.5 %, Network ≈ 0.9 %, process views ≈ 2.7 % of one
  core (the OS process enumeration dominates; see [PERFORMANCE.md](PERFORMANCE.md)).

### Fixed (found while verifying on Linux)
- A timed-out external command could block for as long as a leftover child kept its output pipe
  open; the whole process group is now killed and output collection is time-bounded.
- Linux: process names are no longer cut to 15 characters; threads are no longer listed as
  processes; single-file bind mounts are no longer listed as volumes.
- Command palette ranking preferred scattered letters over an exact word match.

### Known limitations
- Tested on Windows 11 and on Ubuntu 24.04 under WSL2. Not tested on bare-metal Linux or macOS,
  against a real SSH server, or with real battery/thermal hardware.
- The default 1 s refresh costs ≈ 2.7 % of one core on the Windows test machine in process views.
- Minimum supported Rust version is set by dependencies (see `rust-version` in `Cargo.toml`).

### Packaging (unreleased)
- Per-user MSI (WiX 5.0.2), winget/Scoop/Chocolatey definitions, `.deb`, man page and shell completions
  (`systui man`, `systui completions <shell>`), Authenticode signing script, keyless sigstore signing and
  provenance attestations in the release workflow. Linux releases are built on Ubuntu 22.04 (glibc guard).

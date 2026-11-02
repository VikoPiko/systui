# SysTUI implementation tracker

> All entries start unchecked. Only check an item when implemented **and verified**. See [SPEC.md](SPEC.md) §16–17.

## M0 — Foundation
- [x] Initialize native Rust binary, `Cargo.lock`, build config and basic README. (`cargo build --release` OK)
- [x] Validate Ratatui/Crossterm/sysinfo version and MSRV compatibility. (ratatui 0.30.2 re-exports the only crossterm, 0.29.0; sysinfo 0.40.1; tested on rustc 1.99.1 only — declared MSRV 1.85 is unverified)
- [x] Implement CLI `systui`, `--help`, `--version` and config parsing. (`doctor`/`snapshot` land in M3)
- [x] Implement robust terminal initialization/restoration and exit handling. (`try_init` + Drop guard + panic hook; user ran the TUI on Windows and reported it works as expected and exits cleanly — manual, not automated)
- [x] Implement responsive tabs, selection/focus state, footer and keyboard navigation. (TestBackend-verified)
- [x] Add tests for layout and navigation. (23 tests)
- [x] Verify fmt, clippy, test and run initial app on available OS. (Windows 11; interactive run done by the user)

## M1 — Core metrics
- [x] Persistent sysinfo collector with CPU warm-up and targeted refresh. (one long-lived `System`/`Networks`/`Disks` on a worker thread; second sample at +500 ms; volumes refresh every 10 s)
- [x] CPU/RAM/uptime with correct units and states. (first CPU sample = `warming up`, never 0%)
- [x] Network interface counters and delta rates. (own `RateTracker`: first sample/reset => none)
- [x] Disks/volumes and their capacity/free data. (Network and Storage tabs also implemented, as they are P0 views)
- [x] Ring-buffer histories, real Overview, first-sample and stale states. (bounded `History`; `STALE` label after 3 intervals + 1 s)
- [x] Validate readings against OS utilities and test rate calculations. (Windows 11: RAM total exact, 6/6 volumes exact, logical CPUs and process count match PowerShell; CPU% within the range of 3 `Get-Counter` samples — different windows so not identical; uptime see DECISIONS ADR-013)
- [x] Linux validation. (Ubuntu 24.04 on WSL2, rustc 1.99.1: fmt + clippy `-D warnings` clean, 387 tests pass; real collector, memory, volumes, network, process list verified; found and fixed 15-char process-name truncation, userland threads listed as processes, file bind-mounts listed as volumes. NOT bare-metal Linux, so battery/temperature hardware is still untested.)

## M2 — Processes
- [x] Full process list with PID, identity, CPU, RSS, status. (identity = PID + start time; status is `—` on Windows because sysinfo hard-codes "Run" there; inaccessible processes show `denied`, not 0)
- [x] Sorting, filtering, selection preservation and details view. (identity-based selection; windowed rendering)
- [x] Parent/child navigation and static metadata lazily collected. (`p`/`c`; command line fetched only for the process whose details are open and hidden until `e`)
- [x] Confirmation-gated termination with PID reuse protection. (type PID + Enter; target captured at dialog open; identity re-read from the OS immediately before signalling; self/critical processes refused; exit confirmed from later snapshots)
- [x] Unit/integration tests + disposable-process termination smoke test. (real child spawned, found by the collector, force-terminated, verified gone; stale start time and wrong name rejected with child left alive)
- [ ] `x` sanitized process export — deferred to M3 with the JSON snapshot/redaction work.
- [x] Graceful (SIGTERM) path — exercised for real on Linux against a disposable child (the Windows build has no graceful option, by design).

## M3 — Polish and usable release
- [x] Ctrl+K command palette and help overlay. (fuzzy search over a fixed internal command list; shell-looking text matches nothing; `terminate` via palette still needs PID confirmation; 8 tests)
- [x] Wide, standard, compact, and too-small layouts. (tier tests + render tests at 120x40, 90x26, 70x20, 50x12 and degenerate sizes)
- [x] Dark/light/mono/no-color mode; safe Unicode fallback. (tests assert zero colors in mono/`NO_COLOR`, basic-ANSI-only palettes, ASCII mode contains no box/block glyphs)
- [x] `doctor`, scriptable `snapshot --json`, sanitized export. (real binary tests: valid JSON, stable key set, secrets/hostname/username absent, `null` for unavailable, no-overwrite without `--force`, UNC/URL rejected; `x` process export)
- [x] Windows per-user and Linux per-user installation scripts. (`install.ps1` and `cargo install --path . --locked --root <tmp>` run on Windows with the installed binary executed; `install.sh` now also run for real on Ubuntu 24.04 (WSL2): builds with `--locked`, installs mode 755, prints the PATH hint, installed binary runs; `cargo install --path . --locked --root <tmp>` works there too. macOS untested.)
- [x] Benchmarks documented in PERFORMANCE.md with honest observations. (**CPU target missed at default refresh: 2.81 % vs <1 %**; RSS, render, input and startup targets met on Windows)
- [x] README includes correct features, limitations, keybindings, installation.
- [ ] Final P0 tests and manual smoke test on available OS. (automated: all pass; manual: user confirmed M0–M2 UI works on Windows Terminal; palette, `x`, `doctor`/`snapshot` in a real terminal NOT yet manually checked; 5-minute run and Linux/SSH smoke NOT done)

## M4 — Developer extensions (only after M0–M3)
- [x] Developer process-family view. (configurable whole-word families + aliases, aggregates over KNOWN values only, hedged "possible …" flags, drill-down to a filtered Processes view; 16 unit/integration tests)
- [x] Read-only Docker container stats and on-demand `system df`. (CLI via fixed argv + timeout + output cap; explicit templates so labels/env never read; `system df` only on `R`; verified against the real engine: 71 containers / 32 running and all four `df` rows match `docker` itself)
- [x] Windows WSL distro status and explicitly configured VHDX sizes. (UTF-16 output decoded; verified against `wsl -l -v` on this machine: Ubuntu + docker-desktop; VHDX size path unit-tested with a temp file; **not** verified against a real `.vhdx` file; no discovery, no state-changing commands)
- [x] Optional NVIDIA GPU utilization/VRAM/temperature. (`nvml-wrapper` 0.13, loaded lazily at runtime; verified on a GTX 1080 Ti: name, util, VRAM, temp, power, fan; unreported fields shown `n/a`; multi-GPU/per-process still P2)
- [x] Module-disabled/unavailable/error states and timeout tests. (render tests for disabled / checking / unavailable / error-with-stale-data / STALE / ok; exec timeout kills a hung child; hidden Dev tab => zero polls; modules start lazily)
- [ ] Linux behavior of Docker/GPU/WSL-unavailable paths — NOT RUN (no Linux host). The "WSL is Windows-only" path is unit-tested but only compiled on Windows here.
- [ ] Manual check of the Dev tab in a real terminal — NOT done by me (automated TestBackend only); please try `5`, `Tab`, `Enter`, `R`.

## M5 — Operations
- [x] Local health checks with timeouts and explicit config. (tcp / http-status / process / OS-service; loopback-only targets enforced at config load, `allow_lan` for private ranges, public IPs and hostnames always rejected, no TLS, no credentials, query never displayed; statuses `healthy` / `port open` / `http NNN` / `not reachable` / `not running` / `unknown` / `STALE` kept distinct; verified end-to-end against a **disposable local HTTP server** that is flipped 200 -> 503 -> 200; disabled checks proven never to connect)
- [x] Low-noise alert thresholds and hysteresis. (CPU/RAM sustained, disk free %, service after N failures; clear margin + clear time + cool-down; missing data never raises or clears; defaults match the spec: 90 % / 90 % / <10 % free / 15 s / 3 failures; **off by default**, no OS notifications; 14 engine tests with injected time)
- [x] Session-local process changes and snapshot deltas. (`w` overlay: started / exited / CPU and memory movers vs ~5 s, 30 s, 2 min; bounded ring of 40 snapshots, never written to disk; PID-reuse aware; unknown values never diffed as 0)
- [x] Process tree/aggregation, per-process I/O and listener information where feasible. (`t` tree with collapse/expand and `Σ` subtree totals, cycle/self-parent/PID-reuse safe; `i` per-process I/O columns, warm-up baseline, `denied` for unreadable; `L` listening sockets + owner via `netstat -ano` / `ss`, on demand only; verified against the real OS: this test process is reported as the owner of its own listening socket)
- [x] CPU optimisation requested after M4: collector refreshes only what the visible tab shows. (Storage 0.51 %, Services 0.55 %, Network 0.90 % vs ~2.7 % before; **Overview/Processes/Dev unchanged at ~2.7 %**, see PERFORMANCE.md for why)
- [ ] Alert when a monitored container exits unexpectedly — NOT done: Docker data is only polled while the Dev tab is open, so it cannot watch in the background without changing that design.
- [x] Linux behavior of `ss`, `systemctl` and the unavailable paths. (WSL2: `ss` reports this test process as owner of its own listening socket; `systemctl show` distinguishes an active unit from an unknown one; WSL-only module reports itself unavailable. The `ss` parser is still also unit-tested on sample text.)
- [ ] Manual check of the new UI in a real terminal (tree, `w`, `L`, `i`, Services, alert badge) — NOT done by me; automated TestBackend only.

## M6 — Stretch
- [x] Consent-based remote read-only SSH checks. (system `ssh` only; allowlist in config; per-host, per-session typed consent that is never persisted; worker contacts nothing without consent AND the Services tab being open; `BatchMode=yes`, `StrictHostKeyChecking=yes`, no forwarding/multiplexing, `--` before the host, host/user/unit validation blocks option injection; one FIXED read-only remote script; verified: worker-level gating tests, hardened-argv test, fake-ssh pipeline, and the **real `ssh` client refusing a closed local port fast with no prompt**. Verified against a **real SSH server: NO** — none was available, so the parse of a real remote reply is tested on canned Linux output only)
- [x] Docker logs preview, confirm-before-action workflow. (`l` logs: stdout+stderr merged chronologically, ANSI stripped, never saved/exported; `S`/`X` stop/restart behind `allow_actions = false` default + typed container name + id/name re-validation, only those two verbs exist; compose project column; verified against real Docker: logs of an existing container, wrong-name refusal leaves a real container running, and a full stop → double-stop refusal → restart cycle on a **disposable container** that was created and removed by the test, no leftovers)
- [x] Optional session recording with retention limits. (`[recording] enabled` only allows it; Ctrl+R starts/stops; JSONL of the redacted snapshot document; bounded queue + writer thread; rotation by size; retention by count, total size and age touching only `systui-*.jsonl`; stale-process lines withhold stale values; real-collector run verified to contain no username or hostname)
- [x] Device battery/temperature/thermal metrics, when supported. (battery + temps + throttle counters; Linux via sysfs, Windows battery via `GetSystemPowerStatus`; absent hardware is absent; verified on this desktop: no battery, temperatures reported unavailable with the reason. **Linux and a real battery were NOT tested**: sysfs parsing is tested against a fake tree, the Windows state mapping against table-driven inputs)
- [x] Fixed after M5 on the way: palette ranking bug ("record" matched "Reverse process sort order" first), SSH MEM/DISK cells were clipped, palette "Go to Dev workspace" entry (M4) was missing.
- [ ] Multi-GPU selector/details — NOT done (up to 4 GPUs are listed read-only).
- [ ] Saved workspace/bookmark presets — NOT done (config is hand-edited only).
- [ ] Richer process-family heuristics — NOT done.
- [ ] Linux behavior of M6 code (partly verified; see what is still open). (Docker logs/stop/restart on a disposable container, recording, and NVML via WSL GPU passthrough all pass on Linux; the sysfs sensor reader correctly reports "no sensors" under WSL2 and is otherwise tested against a fake sysfs tree. **Still NOT tested: ssh against a real SSH server (none available: no sshd and no sudo), and real battery/thermal hardware.**)
- [ ] Manual check of M6 in a real terminal by a human — NOT done. (On Linux the TUI WAS driven through a real pseudo-terminal by `scripts/pty_smoke.py`, 33 checks, incl. a storm of keys, palette, tree, What-changed, resizes down to 1x1, tty restore; the Windows console has not been scripted.)

## Release readiness
- [x] `LICENSE` (MIT). The copyright holder is the placeholder "The systui authors": **replace it with your name/organisation**.
- [x] `CHANGELOG.md` (Keep a Changelog format, 0.1.0 unreleased); version stays 0.1.0 until you decide to tag.
- [x] README screenshots: real UI, real data from the Windows dev machine, reproducible with `cargo run --release --example screenshot` (host name replaced by `demo-host`; process/interface names are the real ones, so review before publishing; the Dev/Services tabs are not captured because they show container names).
- [x] Minimum supported Rust verified by test, not assumed: the declared 1.85 was wrong; `sysinfo` 0.40 needs 1.95. Verified on Linux: 1.95 builds all targets and passes the unit tests; 1.94 is rejected. `Cargo.toml` now says `rust-version = "1.95"`.
- [x] Dependency audit: `cargo audit` (RustSec database, 1,402 advisories) over all 142 locked crates: no vulnerabilities. Only duplicate versions are build-time proc-macro crates (`darling`, `syn`, `thiserror-impl`).
- [x] Code review pass: no `unwrap`/`expect` left on runtime paths (the 7 that existed were removed); `Command::new` only in `util/exec.rs`; exactly 2 `unsafe` blocks, both with a SAFETY comment; one dead helper and one vestigial abstraction (`Tab::unavailable_reason`) removed; stale footer hints (`1-4 view`) fixed.
- [x] CI workflow (`.github/workflows/ci.yml`: fmt, clippy `-D warnings`, tests on Ubuntu + Windows, pseudo-terminal smoke test on Ubuntu, MSRV check) and release workflow (`release.yml`: tag `v*` builds Linux + Windows archives with SHA-256 and attaches them to a GitHub release). **Both parse as YAML but have NOT been executed** (they need a GitHub repository). Expect to fix small things on the first real run.
- [x] A real-world safety finding: a live-system Docker test used the first running container as its target. It was removed (the disposable-container test already covers refusal); real Docker tests may now only read existing containers and only mutate containers they created themselves.
- [ ] No git commit exists yet (nothing has been committed or pushed); no repository URL in `Cargo.toml` (`publish = false` until you choose one).
- [ ] Not done: code signing, an MSI/winget/Homebrew package, bare-metal-Linux and macOS testing, glibc portability testing of the release binary (the release workflow builds on `ubuntu-latest`).

## Verification log
- **Linux (WSL2 Ubuntu 24.04, kernel 6.6, rustc 1.99.1)**: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` clean; `cargo test` = 387 pass, 0 fail; `scripts/pty_smoke.py` = 33/33 checks pass; release binary built with `cargo build --release`. Windows after the same fixes: 384 pass, 0 fail.
- Release pass: Windows `cargo test --locked` = 383 pass, 0 fail (one risky real-Docker test removed); `cargo fmt`/`clippy --locked -D warnings` clean with the corrected `rust-version = "1.95"` (which enabled a newer clippy lint, fixed).
- M0 (Windows 11, rustc 1.99.1): `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (23 pass). Manually ran `--version`, `--help`, non-TTY refusal (exit 1), `--refresh-ms 0` rejection (exit 1). Interactive TTY run NOT yet done.
- M1+M2 (Windows 11, rustc 1.99.1): fmt/clippy `-D warnings` clean; `cargo test` = 107 tests pass (unit 12, config 10, navigation 13, app 21, process view 12, rate 6, render 18, real-OS collector+actions 14, live pipeline 1). Interactive TTY run still NOT done.
- M3 (Windows 11, rustc 1.99.1): fmt/clippy `-D warnings` clean; `cargo test` = 139 pass, 0 fail (adds palette 7, theme 4, snapshot schema 6, real-binary CLI 9, plus unit tests). Benchmarks: see PERFORMANCE.md.
- M4 (Windows 11, rustc 1.99.1): fmt/clippy `-D warnings` clean; `cargo test` = 193 pass, 0 fail (adds dev 22, modules unit 11, exec 6, live Docker/WSL/GPU 2, family/palette updates). Real adapters cross-checked against `docker`, `docker system df`, `wsl -l -v`, `nvidia-smi`. Overhead: PERFORMANCE.md.
- M5 (Windows 11, rustc 1.99.1): fmt/clippy `-D warnings` clean; `cargo test` = 283 pass, 0 fail (adds health 14, alerts 14, changes 9, ports 5, health/alerts integration 17 incl. a disposable-server end-to-end, needs 11, M5 UI 18). Real OS checks: netstat owner lookup of this test process, Windows `sc query` service check, real collector resume/warm-up/I-O baselines. Per-tab CPU: PERFORMANCE.md.
- M6 (Windows 11, rustc 1.99.1): fmt/clippy `-D warnings` clean; `cargo test` = 383 pass, 0 fail (143 unit + 21 suites). New: sensors 6, recorder 11, ssh 12+7, docker logs/actions 12+16, recording 13, ssh integration 17. Real-system: Docker logs/stop/restart on a disposable container, real `ssh` refusal, recording from the live collector. CPU re-check after M6: Overview 2.73 %, Storage 0.47–0.70 % (run-to-run noise), unchanged.

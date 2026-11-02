# systui

A fast, read-only, keyboard-first **terminal system monitor** written in Rust. One native
executable, no daemon, no telemetry, and no network access of its own. The only connections it ever makes are the ones you configure and (for SSH)
explicitly consent to: loopback health checks, and read-only SSH to hosts on your allowlist.

```
systui            # open the interactive UI
systui doctor     # check collectors, show resolved config path
systui snapshot --json
```

> Status: **v0.1 (milestones M0–M6 implemented)**. Tested on Windows 11 (native) and Ubuntu 24.04 (under WSL2:
> full test suite and a scripted pseudo-terminal run). Not yet tested on bare-metal Linux, macOS, or with a real SSH server. See [TASKS.md](TASKS.md) for exactly what is
> verified, [PERFORMANCE.md](PERFORMANCE.md) for measurements and [SPEC.md](SPEC.md) for the plan.

## Screenshots

Plain-text captures of the real UI, rendered by `cargo run --release --example screenshot` with live data from the
Windows 11 development machine (the host name is replaced by `demo-host`; everything else is as collected; colours are
not shown). The terminal version has colours, a highlighted selection and bars that change colour with load.

**Overview (118Ã34)**
```text
╭ systui v0.1.0 ────────────────────────────────────────────────────────────────────────────────────────── live 0.1s ╮
│  1 Overview   2 Processes   3 Network   4 Storage   5 Dev   6 Services                                             │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│╭ Resources ───────────────────────────────────────────────────────────────────────────────────────────────────────╮│
││CPU   98%           ▅█▄██  12 logical                    DISK C:\ ███████░ 89%  53.6 GiB free                     ││
││RAM  27.0 GiB / 31.9 GiB  ███████░ 85%                   NET  ↓ 295 B/s  ↑ 195 B/s                                ││
││SWAP 12.9 GiB / 20.7 GiB                                 demo-host · Windows 11 Pro · up 13d 2h                   ││
│╰──────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯│
│╭ History (last 5s) ───────────────────────────────╮╭ Top by CPU (s: switch) ──────────────────────────────────────╮│
││CPU                                   ▅█▄██       ││PID      PROCESS                            CPU     MEM       ││
││RAM                                  ▇▇▇▇▇▇       ││83812    firefox.exe                        12%     398 MiB   ││
││↓                                     █▁▁▁▁       ││13772    firefox.exe                        12%     189 MiB   ││
││↑                                     ▁▁▁▁▁       ││37260    screenshot.exe                     12%     16.3 MiB  ││
││net scale: 0 .. 6.4 MiB/s                         ││75472    firefox.exe                        12%     526 MiB   ││
││                                                  ││45844    explorer.exe                       9.5%    298 MiB   ││
││c0   ███████████████████░  97%                    ││25304    com.docker.backend.exe             4.8%    1.1 GiB   ││
││c1   ███████████████████░  97%                    ││47584    claude.exe                         4.8%    465 MiB   ││
││c2   ████████████████████  98%                    ││79804    firefox.exe                        4.8%    253 MiB   ││
││c3   ████████████████████  98%                    ││77316    claude.exe                         2.4%    350 MiB   ││
││c4   ████████████████████  99%                    ││10648    firefox.exe                        2.4%    265 MiB   ││
││c5   ████████████████████  99%                    ││                                                              ││
││c6   ████████████████████ 100%                    ││                                                              ││
││c7   ████████████████████  98%                    ││                                                              ││
││c8   ████████████████████  99%                    ││                                                              ││
││c9   ████████████████████  98%                    ││                                                              ││
││c10  ████████████████████  98%                    ││                                                              ││
││c11  ████████████████████ 100%                    ││                                                              ││
││                                                  ││                                                              ││
││                                                  ││                                                              ││
││                                                  ││                                                              ││
││                                                  ││                                                              ││
│╰──────────────────────────────────────────────────╯╰──────────────────────────────────────────────────────────────╯│
│ 1-6 view  s CPU/MEM  w changes  Space pause  Ctrl+K cmds  ? help  q quit                                           │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
```

**Processes with the details pane (118Ã30)**
```text
╭ systui v0.1.0 ────────────────────────────────────────────────────────────────────────────────────────── live 0.1s ╮
│  1 Overview   2 Processes   3 Network   4 Storage   5 Dev   6 Services                                             │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│╭ Processes 333/333  sort CPU v  CPU: 100%=1 core ────────────────╮╭ Details (Esc close) ──────────────────────────╮│
││PID     NAME                 CPU     MEM        STATUS    PPID   ││Name    firefox.exe                            ││
││61908   dllhost.exe          19%     14.5 MiB   —         1588   ││PID     83812                                  ││
││75472   firefox.exe          14%     526 MiB    —         58592  ││Parent  75472 (firefox.exe)                    ││
││83812   firefox.exe          14%     398 MiB    —         75472  ││Status  not reported by this OS API            ││
││47584   claude.exe           12%     467 MiB    —         84488  ││Started running 4h 52m                         ││
││13772   firefox.exe          9.3%    189 MiB    —         75472  ││CPU     14.0%                                  ││
││37260   screenshot.exe       9.3%    16.7 MiB   —         83324  ││Memory  398 MiB (resident/working set)         ││
││88644   audiodg.exe          4.7%    19.4 MiB   —         3848   ││I/O     off (press i)                          ││
││70252   WindowsTerminal.exe  2.4%    110 MiB    —         73596  ││Path    C:\Program Files\Mozilla               ││
││72824   Docker Desktop.exe   2.3%    194 MiB    —         71508  ││Firefox\firefox.exe                            ││
││72980   claude.exe           2.3%    150 MiB    —         86820  ││Cmd     loading…                               ││
││1480    ChatGPT.exe          0.0%    183 MiB    —         45972  ││                                               ││
││3580    ChatGPT.exe          0.0%    39.1 MiB   —         1480   ││CPU hist                                ▄▄█▄▄▄ ││
││3748    ChatGPT.exe          0.0%    307 MiB    —         1480   ││MEM hist                                ██████ ││
││3876    ChatGPT.exe          0.0%    64.1 MiB   —         1480   ││                                               ││
││5236    AnyDesk.exe          0.0%    3.5 MiB    —         1400   ││p parent  c child  K terminate                 ││
││5948    conhost.exe          0.0%    9.5 MiB    —         62564  ││                                               ││
││6064    ShellExperienceHost. 0.0%    43.3 MiB   —         1588   ││                                               ││
││7780    codex-code-mode-host 0.0%    19.0 MiB   —         61812  ││                                               ││
││7836    wslhost.exe          0.0%    11.2 MiB   —         37432  ││                                               ││
││8296    svchost.exe          0.0%    10.8 MiB   —         1400   ││                                               ││
││10648   firefox.exe          0.0%    265 MiB    —         75472  ││                                               ││
││11456   MicrosoftStartFeedPr 0.0%    26.6 MiB   —         1588   ││                                               ││
│╰─────────────────────────────────────────────────────────────────╯╰───────────────────────────────────────────────╯│
│ j/k move  / filter  s sort  t tree  i I/O  Enter details  K terminate  w changes  ? help  q quit                   │
╰────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯
```

**Compact layout (72Ã22)**
```text
╭ systui v0.1.0 ──────────────────────────────────────────── live 0.1s ╮
│  1 Overview   2   3   4   5   6                                      │
│──────────────────────────────────────────────────────────────────────│
│╭ Resources ─────────────────────────────────────────────────────────╮│
││CPU   99%  █▄██████  12 logical                                     ││
││RAM  27.1 GiB / 31.9 GiB  ███████░ 85%                              ││
││SWAP 12.9 GiB / 20.7 GiB                                            ││
││DISK C:\ ███████░ 89%  53.6 GiB free                                ││
││NET  ↓ 20.9 KiB/s  ↑ 1.7 KiB/s                                      ││
││demo-host · Windows 11 Pro · up 13d 2h                              ││
│╰────────────────────────────────────────────────────────────────────╯│
│╭ Top by CPU (s: switch) ────────────────────────────────────────────╮│
││PID      PROCESS                                  CPU     MEM       ││
││61908    dllhost.exe                              19%     14.5 MiB  ││
││75472    firefox.exe                              14%     526 MiB   ││
││83812    firefox.exe                              14%     398 MiB   ││
││47584    claude.exe                               12%     467 MiB   ││
││13772    firefox.exe                              9.3%    189 MiB   ││
││37260    screenshot.exe                           9.3%    16.7 MiB  ││
│╰────────────────────────────────────────────────────────────────────╯│
│ ? help  q quit                                                       │
╰──────────────────────────────────────────────────────────────────────╯
```


## What it does

| View | Key | Shows |
| --- | --- | --- |
| Overview | `1` | CPU (aggregate + per core), RAM/swap, main volume, network rate, top processes, 5-minute history |
| Processes | `2` | All visible processes: sort, filter, details, parent/child jump, guarded terminate |
| Network | `3` | Per-interface rates (bytes/s) and cumulative totals, per-interface history |
| Storage | `4` | Mounted volumes: capacity, used %, free (capacity only, not disk I/O) |
| Dev | `5` | Process families (node, dotnet, cargo, docker, …) with counts, memory, CPU and hedged "possible" flags; plus **GPU (NVIDIA)**, **Docker** and **WSL** panels |
| Services | `6` | Your own **local health checks** (TCP, HTTP status, process, OS service) and **alerts** (both opt-in) |

Missing data is shown as `—`, `denied`, `warming up…` or `unavailable`, never as `0`.

## Install

Requires the [Rust toolchain](https://rustup.rs) **1.95 or newer** (that is the minimum `sysinfo` 0.40 supports; verified: 1.95 builds and passes the unit tests, 1.94 is rejected).

**Windows (PowerShell, per-user, no admin):**
```powershell
.\scripts\install.ps1            # builds, copies to %USERPROFILE%\.local\bin, asks before editing your user PATH
# non-interactive: .\scripts\install.ps1 -AddToPath      (or -NoPath to never touch PATH)
```
**Linux (per-user, no sudo):**
```sh
./scripts/install.sh             # installs to ~/.local/bin and prints the PATH line to add
```
**Either OS, via Cargo:**
```sh
cargo install --path . --locked  # installs to ~/.cargo/bin (normally already on PATH)
```
Then open a **new** terminal and run `systui`.

## Keys

| Key | Action |
| --- | --- |
| `1`–`4` | Switch view (`5`,`6` explain they are not available) |
| `Ctrl+K` | Command palette (fixed internal commands only; never runs shell text) |
| `?` | Help overlay |
| `Tab` | Cycle pane focus |
| `Space` | Pause/resume the display (sampling continues; paused samples are not added to history) |
| `r` | Request an immediate refresh |
| `q`, `Ctrl+C` | Quit (terminal is restored) |
| `j`/`k`, arrows, `PgUp`/`PgDn`, `Home`/`End` | Move selection |
| `/` | Process filter (name or PID, case-insensitive). `Enter` keeps, `Esc` clears |
| `s` / `S` | Cycle sort key (CPU, MEM, NAME, PID) / reverse. On Overview `s` flips the top list CPU↔MEM; on Storage it cycles sort |
| `Enter` | Process details (`Esc` closes) |
| `e` | In details: reveal the command line (hidden by default; it may contain secrets) |
| `p` / `c` | Jump to parent / first child |
| `t` | Processes: tree view (parents with children). `Left`/`Right` (or `h`/`l`) collapse/expand; collapsed nodes show `Σ` subtree totals. Paused while a filter is active |
| `i` | Processes: per-process **I/O** columns (read/write bytes per second); collected only while on |
| `w` | **What changed?** overlay: new/exited processes and the biggest CPU/memory moves since ~5 s, 30 s or 2 min ago (`[` `]` pick the window) |
| `L` | Network: **listening sockets** with owning process (collected once on demand; `R` refreshes) |
| `Ctrl+R` | Start/stop **session recording** (JSONL; needs `[recording] enabled = true`; shows `REC` in the header) |
| `R` | Dev/Services/Network-ports: refresh the focused panel (Docker also runs `system df`) |
| `l` / `S` / `X` | Dev, Docker panel: container **logs** preview / **stop** / **restart** (stop and restart need `[docker] allow_actions = true` and typing the container name) |
| `C` | Services, SSH hosts (after `Tab`): give **consent** to contact the selected allowlisted host |
| `x` | Export the selected process as sanitized JSON to the current directory (never overwrites) |
| `K` | Terminate the selected process (see below) |

## Terminating a process

`K` opens a dialog showing the name, PID and path. You must **type the PID and press Enter**
(set `confirm_terminate = false` for a single `y`). The target is fixed when the dialog opens;
just before signalling, systui re-reads the live process and refuses if the PID now belongs
to a different process (PID reuse) or the name changed. systui itself and critical OS
processes (`lsass`, `csrss`, `systemd`, …) are refused; `svchost`, `explorer` etc. get a
warning. Afterwards it checks the next samples and tells you whether the process really exited.

* **Linux:** SIGTERM first; `f` in the dialog switches to SIGKILL and needs a fresh confirmation.
* **Windows:** there is no graceful option in the API used, so the only action is an immediate
  forced terminate, and the dialog says so.
* The protected list is best-effort by name, not a guarantee. Use judgement.

## Semantics worth knowing

* **CPU %** in the process table is per-core-equivalent by default: 100 % = one logical CPU, so
  a busy multi-threaded process can exceed 100 %. Set `cpu_mode = "host_share"` for share of total.
  The table title states which mode is active.
* **Memory** is resident set (Linux) / working set (Windows), not virtual memory.
* **Rates** are bytes/second between two samples. `TOTAL` columns are cumulative bytes since the
  counter started (boot/driver), not a rate. The Overview network figure sums non-loopback
  interfaces and may double-count virtual adapters.
* **`denied`**: Windows refuses counters for protected processes (services, `vmmemWSL`, …) in a
  normal session. About 40 % of processes showed this on the (unelevated) test machine. systui never asks for admin/UAC/sudo.
* **Windows process status** is `—`: the library reports a fixed value there, which would be invented.
* The first CPU reading needs two samples (~0.5 s); until then it says "warming up".

## Subcommands & flags

```
systui [--config PATH] [--refresh-ms 500..5000] [--no-color]
systui doctor
systui snapshot [--json] [--output PATH [--force]] [--top N]
```
* `--help`, `--version`, `doctor`, `snapshot` never enter raw mode or the alternate screen.
* Interactive mode without a TTY exits 1 with a message; `snapshot` works in scripts/CI.
* `snapshot --json` is a stable, versioned document (`schema_version: 1`). Unavailable values are
  `null`. By default it **omits** hostname, usernames (home directories in paths become `~`,
  other profiles `<user>`), command lines, environment and network addresses. `--output` refuses
  to overwrite without `--force` and rejects UNC/URL targets.
* `NO_COLOR` and `--no-color` give a monochrome UI (selection uses reverse video).

## Configuration

Optional. Path: `%APPDATA%\systui\config\config.toml` (Windows) or `~/.config/systui/config.toml`
(Linux); `systui doctor` prints the exact path. Copy [config/config.example.toml](config/config.example.toml).
Out-of-range values (e.g. `refresh_ms = 0`) are rejected with a message naming the key; unknown
keys are ignored silently. Command-line flags override the file.

## Privacy and safety

Local collection only. No telemetry, update checks, cloud calls or network sockets of its own.
Read-only except the guarded terminate action. Process names/paths/command lines (and container
names from Docker) are sanitized so hostile text cannot inject terminal escapes.

**External programs.** By default systui starts no other program. Only if you enable them:
* `[modules] docker = true` runs `docker ps`, `docker stats --no-stream`, `docker version`, and
  `docker system df` only when you press `R` on the Docker panel.
* `[modules] wsl = true` runs `wsl.exe --list --verbose` (this does not start a distribution).
* `systui doctor` probes Docker/WSL/NVML once because you asked for diagnostics.
* `[modules] health = true` makes **loopback-only** connections (or private-LAN ones with `allow_lan = true`)
  to exactly the targets you list; `process` checks read the local process list and `service` checks run
  `sc.exe query` (Windows) / `systemctl show` (Linux). Public addresses and hostnames are rejected when the
  config loads; hostnames are never resolved; `https://` and credentials in URLs are refused; query strings are
  never displayed.
* Pressing `L` in the Network tab runs `netstat -ano` (Windows) / `ss -H -ltnup` (Linux) once.
* `[modules] ssh = true` runs the system `ssh` client against hosts in your `[[ssh.hosts]]` allowlist, but only for
  hosts you consented to in the UI this session and only while the Services tab is open (see below).
* Docker `l` runs `docker logs --tail N`; `S`/`X` run `docker stop`/`docker restart` on the single container whose name you typed.
  Nothing else mutating is possible: there is no remove/kill/prune code.
* Ctrl+R recording writes JSONL files into the recordings directory (default: per-user data dir), only after you start it.* The NVIDIA panel loads the NVML library (`nvml.dll` / `libnvidia-ml`) in-process; no program is run.

Every command uses a fixed program and argument list (never a shell), a hard timeout (the process
is killed), and a 256 KiB output cap. Docker output uses explicit templates so labels and
environment variables are never requested. To turn all of it off: leave `docker`/`wsl` at
`false` and set `gpu = false`.

## Troubleshooting

* `interactive mode needs a terminal` – run in a real terminal, or use `snapshot`.
* Garbled borders – set `unicode = "never"` in config for ASCII, or fix the terminal font/encoding.
* Many `denied` rows – expected without elevation; see above.
* Run `systui doctor` first and include its output in bug reports.

## Known limitations

* Linux was tested under WSL2 only (no battery/thermal hardware, no real SSH server); macOS is untested. On Linux, process names longer than 15 characters are recovered from the executable/argv when readable (the kernel truncates them), and threads are not listed as processes.
* Not implemented: per-process GPU, multi-GPU selector, WSL guest metrics, saved workspace/bookmark presets, alerts on remote hosts, alert on container exit (Docker data is only collected while the Dev tab is open, so it cannot watch in the background).
* Health checks run one at a time, each bounded by its own timeout; many slow/failing checks delay the others. HTTP checks are plain `http://` only (no TLS).
* Per-process I/O on Windows counts all I/O (files, pipes, network), not just disk; unreadable processes show `denied`.
* "What changed?" cannot see processes that start and exit between two stored snapshots (taken at most every 5 s while a process view is open).
* Listening ports on Linux depend on `ss` and show owner `unknown` for other users' sockets without privileges; the Linux parser is unit-tested on sample output only.
* Docker polling is heavy with many containers (`docker stats --no-stream` took 2–3 s with 32 running); data is only collected while the Dev tab is open.
* Docker/WSL/GPU were exercised only on one Windows machine (Docker Desktop 28.3, WSL2, GTX 1080 Ti). WSL on Linux hosts and non-NVIDIA GPUs are unsupported.
* CPU depends on the view: the collector only refreshes what the visible tab shows (Storage / Services ≈ 0.5 %, Network ≈ 0.9 %, Overview / Processes / Dev ≈ 2.7 % at 1 s on the test machine).
* Default 1 s refresh costs ≈2.8 % of one CPU core on a 12-thread Windows box with ~330 processes
  (the OS process enumeration dominates); `--refresh-ms 5000` brings it under 1 %. See PERFORMANCE.md.
* Disk activity (I/O) rates per volume are not collected.
* `theme = "auto"` means dark (terminal background can't be reliably detected).

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked                          # 380+ tests, incl. real-OS collector and kill tests
cargo run --release --example bench -- 60 1000
cargo run --release --example screenshot     # regenerate docs/screens (review the output before publishing)
python3 scripts/pty_smoke.py                 # Linux/macOS: real TUI in a pseudo-terminal (build --release first)
```

Requires Rust 1.95+. CI (`.github/workflows/ci.yml`) runs the same checks on Ubuntu and Windows plus an MSRV check;
`release.yml` builds Linux and Windows archives when a `v*` tag is pushed (neither workflow has been run yet).

Architecture: collector thread (`src/collectors`) → immutable snapshots → `App` reducer
(`src/app.rs`) → pure render (`src/ui`). The only code that may terminate a process is
`src/actions/process.rs`. Decisions are logged in [DECISIONS.md](DECISIONS.md).

## Dev tab (M4)

Press `5`. Integrations start **lazily the first time you open this tab** and poll **only while it
is visible**; core monitoring is never blocked by them.

* **Process families** – whole-word matches on process names (`com.docker.backend.exe` is `docker`,
  `gitkraken` is not `git`). Edit `[developer] process_families` / `[developer.aliases]`. Memory is the
  sum of known working sets (shared pages counted per process; unreadable processes are shown as
  `+N?`, not added as 0). Flags are explicitly hedged ("possible high memory", "possibly idle … heuristic");
  systui never calls a process an orphan. `Tab` cycles panels, `j/k` selects, `Enter` opens the family
  in the Processes tab (`Esc` clears).
* **GPU (NVIDIA)** – on by default but only shows data if NVML exists; every field the driver does not
  report is `n/a`. Refresh 2 s. No per-process GPU data.
* **Docker** (opt-in) – containers with state, CPU % and memory text exactly as Docker reports it; refresh 5 s.
  `R` on the panel runs `docker system df` once (logical sizes, which are not Windows `.vhdx` savings).
  Docker is never started and nothing is ever cleaned up. Container memory is part of host/WSL memory – do not add it.
* **WSL** (opt-in, Windows) – distro name/state/version. Add `[wsl] tracked_vhdx_paths` to see the host
  file size of specific virtual disks (that is allocated space on the host, not guest free space). systui
  never searches for `.vhdx` files and never compacts or shuts anything down.

## Services & alerts (M5)

Press `6`. Both features are **off by default**.

```toml
[modules]
health = true

[[health.checks]]
name = "API"
type = "http"                         # http | tcp | process | service
url  = "http://127.0.0.1:7141/health"
interval_seconds = 30                 # 5-3600
timeout_ms = 1000                     # 100-10000, below the interval

[alerts]
enabled = true                        # CPU/RAM >= 90 % for 15 s, disk free < 10 %, service down after 3 failures
```
* Statuses are kept apart on purpose: `healthy` (HTTP 2xx/3xx, process running, service active),
  `port open` (TCP connected only: **not** proof the app works), `http 503` (answered, but failing),
  `not reachable`, `not running`, `unknown` (not checked yet / disabled) and `STALE` (no recent result).
* Alerts need the condition to hold for `sustain_seconds`, clear only after the value is
  `hysteresis_points` back inside the limit for `clear_seconds`, and cannot re-raise during
  `cooldown_seconds`. Missing data never raises or clears an alert. They are shown in the header
  (`ALERT n`), the Overview, and the Services tab; **no OS notifications are sent**. Alerts keep
  evaluating while the display is paused.

## Stretch features (M6) — all off by default

### Battery & temperatures
Shown on the Overview only when the platform reports them (otherwise nothing is invented): a `BAT` line with
percent, state and the OS time-remaining estimate, and a `TEMP` line with the hottest sensor, sensor count and
CPU thermal-throttle events. **Linux** reads `/sys/class/power_supply`, `hwmon` and `thermal` (tested against a fake sysfs
tree only). **Windows** reads the battery through `GetSystemPowerStatus`; Windows has no unprivileged CPU
temperature API, so temperatures are reported as unavailable *with that reason* (GPU temperature is in the Dev
tab). Both are included in `snapshot --json` (`sensors`, `null`/empty when absent) and checked by `systui doctor`.

### Session recording
```toml
[recording]
enabled = true            # allows recording; it still only starts when YOU press Ctrl+R
interval_seconds = 5
max_file_mb = 10          # new file before a file would exceed this
max_files = 10            # retention: at most this many files...
max_total_mb = 100        # ...at most this much in total...
max_age_days = 7          # ...and nothing older
```
Each line is the same sanitized, redacted document as `snapshot --json` (no hostname, usernames, command lines,
environment). Writing happens on a separate thread through a bounded queue: if the disk is slow, lines are dropped
and counted instead of stalling the UI. If the process list or network was not being refreshed for the visible tab,
that line says so (`processes_fresh: false`) and withholds the stale values. Retention only ever deletes
`systui-*.jsonl` files directly in the recordings directory. Recording keeps running while the display is paused.

### Docker logs, stop and restart
On the Dev tab, Docker panel: `l` shows the last N lines (default 100, merged stdout+stderr in time order, ANSI
sequences stripped). Logs can contain secrets, so they are shown only in that overlay and never saved or exported.
`S` / `X` stop / restart the selected container after you type its **name**; the id and name are re-read from Docker
immediately beforehand and nothing happens if they changed. A `PROJECT` column shows the Docker Compose project
(only that one label is requested) and containers are grouped by it.

### Remote hosts over SSH (read-only)
```toml
[modules]
ssh = true

[[ssh.hosts]]
name = "home-lab"
host = "192.168.1.50"     # hostname, IP, or a Host alias from ~/.ssh/config
user = "viko"
units = ["docker", "nginx"]
```
Open the Services tab, press `Tab` to focus the hosts table, then `C`. The consent dialog shows the exact fixed
script that will run. It uses your existing `ssh` keys/agent (`BatchMode=yes`: it can never prompt, and systui
stores no credentials), requires the host key to be **already trusted** (`StrictHostKeyChecking=yes`: it never accepts
a new key), forbids all forwarding, and only runs while the Services tab is open. Consent is per host, per session, and
never saved. The script reads load, memory, root-disk usage, uptime and `systemctl is-active` for the units you list
(units are validated; host/user values that could be mistaken for ssh options are rejected at config load). The remote
side must be Linux with `/proc`.

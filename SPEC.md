# SysTUI — Product & Engineering Specification

> **Status:** Implementation-ready specification (proposal, not an implemented product)  
> **Product:** `systui`  
> **Version target:** v0.1.0 MVP, with staged v0.2 and v1.0 extensions  
> **Primary environment:** Windows 11 developer workstation (PowerShell / Windows Terminal / VS Code integrated terminal)  
> **Secondary environment:** Ubuntu/Linux machine or SSH session  
> **Implementation:** Native Rust CLI/TUI  
> **Design direction:** Minimal, elegant, keyboard-first terminal UI; text-based micro-graphics; inspired by the clarity and interaction patterns of Claude Code, without reusing its branding or copying its interface verbatim.

## 0. How Claude Code must use this document

You are the lead engineer implementing SysTUI. Read this entire specification before changing files.

1. **Build a functioning, tested product**; do not stop after scaffolding or static UI mockups.
2. Implement the work in the milestones under §16, in order. Complete and verify each milestone before beginning the next.
3. Treat **P0 / MVP** requirements as non-negotiable, **P1** as follow-on, and **P2** as stretch goals. Do not let optional integrations delay core functionality.
4. Choose stable, compatible versions based on official documentation and record the actual versions in `Cargo.lock`. Do not copy example code from older major APIs without checking it against the selected versions.
5. Favor reliable, cheap reads and graceful degradation. Never invent readings, imply a metric is available when it is not, or conceal collector errors.
6. Default to **read-only**. Destructive actions require deliberate activation and confirmation. No elevated privileges by default.
7. Do not add Node.js, Python, Electron, a web server, database, background daemon, cloud account, or telemetry system to achieve the MVP.
8. Keep a live progress checklist in `TASKS.md`, with status based on verification rather than assumption. Maintain `DECISIONS.md` for non-obvious implementation decisions and platform limitations.
9. On Windows and Linux, validate the actual product, not only the Rust type-checker. If you cannot run a target OS, create testable platform abstraction and document **not tested** honestly.
10. Produce build instructions and an install path that exposes the `systui` command on `PATH`.

## 1. Vision & intended use

**SysTUI** is a responsive, low-overhead, native terminal system monitor and developer workstation control panel.

It answers:
- What is my computer doing **right now**?
- Which process or process tree is using CPU, memory, GPU, or disk resources?
- What changed recently, and what might be causing slowdown?
- Which disks, network interfaces, services, containers, and optional developer environments are active?
- Can I safely inspect a suspicious process and take an intentional action without leaving the terminal?

The app must feel faster and cleaner than a graphical task manager for everyday investigation. It must also be usable on a remote Linux SSH terminal.

### 1.1 Product priorities, in order

1. Accurate, plainly labeled system data.
2. Instant keyboard navigation and legible UI.
3. Low resource footprint and quiet operation.
4. Reliable behavior on Windows and Linux.
5. Useful developer views without heavyweight integrations.
6. Small, polished aesthetic touches; never at the expense of data legibility.

### 1.2 Non-goals

- Replacing OS-native security / performance profiling / ETW / PerfMon / eBPF tools.
- Promising per-process network or GPU metrics on every platform or driver.
- A remote agent, multi-host monitoring service, distributed telemetry platform, or persistent time-series database.
- Editing system configuration, cleaning Docker or WSL, or optimizing OS settings automatically.
- Arbitrary shell execution from a command palette.
- Using an LLM, subscription, login, or external API for local monitoring.

## 2. Supported systems and metric boundaries

| Environment | Priority | Expected behavior |
| --- | --- | --- |
| Windows 11 x64 (native binary) | P0 | First-class local monitoring and process inspection. |
| Ubuntu 22.04/24.04+ x86_64 | P0 | First-class local monitoring; SSH-friendly. |
| Windows Terminal / PowerShell / CMD | P0 | Correct Unicode/color fallback, resizing, raw terminal lifecycle. |
| Windows VS Code terminal | P0 | Usable with reasonable compact layouts. |
| Linux under WSL2 | P1 | Monitor Linux guest from a Linux binary, clearly labeled as WSL guest metrics. |
| Windows native host with WSL installed | P1 | Report distro state from host; **do not claim to see every WSL guest process** using ordinary Windows process enumeration. |
| macOS | P2 | Best-effort compilation and baseline metrics; not a release blocker. |
| GPU-specific metrics | P1 | Opt-in adapter, conditional on available drivers/APIs. |

**Metric contract:** Every metric has a unit, collection timestamp, source, and availability state. Show `—`, `unsupported`, `permission denied`, or `stale` instead of 0 when actual data is unavailable. Don't silently add together values representing different namespaces (host RAM vs WSL guest vs container memory).

## 3. Target user experience

### 3.1 Launch / CLI

The compiled executable is named `systui` (`systui.exe` on Windows). These commands must work:

```text
systui                     # open interactive full-screen UI
systui --help
systui --version
systui doctor              # check collector and optional dependency availability
systui snapshot --json     # one-shot, read-only system summary to stdout
systui snapshot --json --output <PATH>  # write to explicit file
systui --config <PATH>     # override user config file
systui --refresh-ms 1000   # override refresh interval within safe range
systui --no-color          # disable colors, retain text contrast
```

`--help`, `--version`, `doctor`, and `snapshot` must not enter alternate screen or raw input mode. Normal `systui` must restore the console on exit or recoverable failure. If stdout is not a TTY, interactive mode prints an actionable error and non-zero exit code; `snapshot` remains scriptable.

### 3.2 Interaction philosophy

- One main screen with a stable header, tab strip, central work area, and footer hints.
- No startup animation, splash screen delay, advertising, or logo that displaces live content.
- Focus indicator and selected row are obvious; restrained accent palette.
- Rapid actions are discoverable through a `Ctrl+K` command palette and `?` help overlay.
- Keyboard-first; mouse scroll/click is optional and may be added later.
- No screen repaint when nothing changes; never target 60 FPS for a monitor that samples at 1 Hz.
- Unicode Braille/sparkline symbols where supported, ASCII fallback otherwise.

### 3.3 Color / visual language

Use calm, text-led styling: dark charcoal background, slightly lighter panel borders, cream/light-gray text, muted warm accent (orange/terracotta) for focus, subdued cyan for time-series curves, and yellow/red only for meaningful thresholds. Also provide `light`, `dark` (default), `mono`, and `auto` themes. Honor `NO_COLOR` and `--no-color`.

Do not depend on Nerd Fonts, emoji rendering, sixel, kitty graphics, images, or true-color support. Color detection and glyph support must degrade cleanly to basic ANSI colors / plain ASCII.

## 4. Reference layout

### 4.1 Standard layout (illustrative, data is fictitious)

```text
╭─ systui ─────────────────────────────────── windows · workstation ── ● live ─╮
│  1 Overview   2 Processes   3 Network   4 Storage   5 Dev   6 Services     │
├──────────────────────────────────────────────────────────────────────────────┤
│ CPU   24%  ▁▁▃▅▂▃▄▂▆   12 logical  │ RAM  18.2 / 32 GiB ░██████░░░ 57%    │
│ GPU   41%  ▁▃▄▆▂▃▅▂▁   3.2 GiB VRAM │ DISK C: 65% ░███████░░  440 GiB free │
├──────────────────────────────┬───────────────────────────────────────────────┤
│ CPU history (5 min)          │ Top resource consumers                        │
│   ▁▂▃▃▄▂▅▆▃▄▂▁▄▅▂          │ PID    PROCESS       CPU    RSS           │
│                              │ 9184   Code.exe      16%    1.2 GiB       │
│ Network                      │ 1312   docker.exe     8%    820 MiB       │
│ ↑ 1.2 MiB/s  ↓ 4.6 MiB/s      │ 20344  dotnet.exe     5%    412 MiB       │
├──────────────────────────────┴───────────────────────────────────────────────┤
│ Alert: none                  Sample: 1.0 s        Last: 15:24:02           │
│ / filter  Tab focus  Enter inspect  Ctrl+K commands  ? help  q quit          │
╰──────────────────────────────────────────────────────────────────────────────╯
```

**This is a structural wireframe, not a pixel-perfect implementation.** Ratatui layout constraints and terminal dimensions govern actual placement. Avoid tightly packed information at small widths.

### 4.2 Responsive behavior

- **Wide** (>=110 columns and >=30 rows): 2-column overview with multiple tiny history graphs and process list.
- **Standard** (80–109 columns, >=24 rows): fewer graphs / compact labels; keep tabs, resource headline, process list and footer.
- **Compact** (60–79 columns or 18–23 rows): single-pane focus; hide secondary previews, shorten column labels, leave navigation functional.
- **Too small** (<60 columns or <18 rows): show a friendly minimum-size message with `q` to quit, rather than panic or corrupt the display.
- On resize, compute constraints safely and preserve selection where possible.

## 5. Views, priority & acceptance criteria

### 5.1 `1` Overview — P0

Live key performance indicators:
- CPU: aggregate busy %, optionally per-core utilization sparkline; CPU core count.
- Memory: total/used/available physical RAM with clear units; swap where supported.
- Storage: top relevant mount/volume free/used space and percent.
- Network: selected interface RX/TX **rates** in bytes/sec and recent history.
- System: uptime, hostname, OS label, last sample age.
- Top 5–10 processes by CPU, with switch to RAM sorting.
- A 2–5 minute history using bounded in-memory rings (not permanent storage).
- Collector errors and missing metrics shown unobtrusively.

**Accept when:** The overview is populated with real readings after a valid sampling window, updates without UI stalls, and displays understandable units/source labels.

### 5.2 `2` Processes — P0

A searchable, scrollable, sortable table of all processes that the current user can enumerate. Fields:

| Field | P0 / P1 | Clarification |
| --- | --- | --- |
| PID, Name | P0 | Stable unique selection uses PID + process start time; guard against PID reuse. |
| CPU % | P0 | Label semantics: single-logical-CPU-equivalent (can exceed 100%) OR normalized host share; allow configurable mode. |
| RSS / working-set memory | P0 | Size in MiB/GiB; do not confuse with virtual memory. |
| Process status | P0 | Running/sleeping/stopped/etc where OS provides it. |
| Parent PID and parent name | P0 | Allow quick parent/child traversal where possible. |
| Executable path, start time | P0 | Open process details pane; best effort for access restrictions. |
| Disk read/write delta rate | P1 | Show only when per-process I/O counters are provided; no invented rates. |
| User and command-line arguments | P1 | Lazy collect; redact sensitive command-line content in exports by default. |
| Process tree / group by executable | P1 | Expand/collapse, preserve selection, aggregate cautiously. |
| GPU % / VRAM | P2 | Only when trustworthy per-process source is available. |

Capabilities:
- Search `/` (case-insensitive substring by name/PID initially; P1 supports `cpu>10`, `mem>500MiB`, etc.).
- Sort by CPU, memory, name, PID; reverse direction. Default CPU descending.
- Move selection with `j/k`, arrow keys, `Home/End`, `PgUp/PgDn`.
- `Enter` opens an inspection panel with identity, parent, command-line (if safe/available), uptime, CPU/RAM history, and available actions.
- `x` exports the selected process **sanitized** details; `e` may show the full command line locally after an explicit reveal prompt.
- `K` opens **Terminate process** confirmation (not enabled by default from the overview). Show PID, process name, executable path when available, and confirmation. Try graceful termination where supported; force kill requires a second, distinct confirmation/command. Provide clear permission-denied outcomes. Never kill a process just because its row changed while a dialog is open.
- Exclude SysTUI itself from accidental termination by default; show it in the list anyway. Protect OS-critical processes as far as reasonably identifiable and make the risk explicit; do not promise universal detection.

**Accept when:** Sorting/filtering does not visibly jump the selected process, PID reuse is checked before termination, access errors are non-fatal, and details are informative.

### 5.3 `3` Network — P0

- List interfaces, state, upload/download **current rates** and cumulative bytes where available.
- Timeline graph (selected interface) and aggregate total, with a note if totals include virtual/loopback interfaces.
- Per-interface IP address / gateway / link metadata only if readily available and inexpensive; otherwise P1.
- Interface selection and stable history; handle interface disappearance.
- Clear distinction between *bytes transferred since boot/counter* and *bytes per second*.

**P1:** Listening ports and owning process where platform/privileges allow; TCP connection list; filter by PID/port. Avoid marketing this as a packet sniffer. Do not attempt privileged raw packet capture.

### 5.4 `4` Storage — P0

- All mounted volumes / filesystems with total, available, used, percent, type where available.
- Sorting by utilization/free space.
- Disk activity rates **only when supported**, distinct from file-system capacity.
- Read-only device and mount metadata; handle removable disks, duplicates, inaccessible mount points.

**P1:** Simple hot-path / watched-folder disk usage *on-demand*, opt-in, depth and scan limit bounded; explicit warning about scan cost. No continual recursive directory walking.

### 5.5 `5` Developer workspace — P1 (highest-value extension)

**Intent:** Diagnose workloads common to .NET, Node/Next.js, Rust, Docker, and local coding tools without hunting through generic processes.

- Group known process families using configurable, user-editable mappings: `dotnet`, `node`, `pnpm`, `npm`, `cargo`, `rustc`, `git`, `docker`, `wsl`, editors and browsers.
- Show process count, aggregate RSS (not deduplicated shared pages), approximate summed CPU, most expensive process, and current executable names.
- Drill into family processes and their parents/children; detect multiple parallel build workers.
- Flag **possible** high-memory / long-idle development processes using transparent heuristic labels, never declare a process "orphan" merely because its parent exited or CPU is low.
- One-click navigation (keyboard action) to the corresponding filtered process table.
- Optional trusted project directory bookmarks in config; do not scan the whole disk for repositories.

**P1 optional subpanel: Docker**
- Detect Docker availability lazily, without blocking app startup or auto-starting Docker Desktop.
- Show running/stopped container list, image, state, uptime, CPU and RAM where the Docker API/CLI supplies it, with refresh slower than core sampling.
- Show read-only `docker system df` size/space summary **on explicit refresh**; do not equate logical Docker reclaimable size to Windows physical `.vhdx` savings.
- Show Docker engine/permission errors clearly; never attempt cleanup automatically.
- Explicitly avoid double counting Docker container consumption as additional host consumption.
- P2: container logs preview (last N lines), restart/stop actions with confirmation, compose grouping.

**P1 optional subpanel: WSL (Windows host only)**
- Detect WSL availability without requiring admin privileges or starting stopped distributions.
- Display distro name, state, and WSL version via supported host commands (e.g. `wsl --list --verbose`).
- Show a read-only `.vhdx` **physical file size**, but only for explicitly configured, user-approved paths. Discovery may be added after a privacy/security review; do not scan user profiles indiscriminately.
- Distinguish `.vhdx` allocated file size from guest disk used/free space, host free space, and Docker's own VHDX storage.
- Never compact, migrate, delete, detach, or shut down WSL from SysTUI in v0.1/v0.2.
- P2: opt-in guest metrics collection that does not start a stopped distro without confirmation.

### 5.6 `6` Services & health — P1

- Configurable **local-only** health checks: TCP connect, HTTP GET (status only), process existence, and read-only OS service status where accessible.
- Examples in documentation (not hardcoded as user data): ASP.NET Core API on a custom health port, a local Docker engine, Portainer, a local DevOps build agent.
- Show service name, check type, status, latency, last successful check, next scheduled check, and failure message.
- Support requests with explicit timeout (default 1 s) and modest polling frequency (default 15–30 s).
- Distinguish `healthy` from `port open`, `http responded`, `not reachable`, `unknown`, and `stale`; a TCP success is not an app health guarantee.
- Never query cloud endpoints or local applications unless explicitly configured by user.
- Bind addresses from config; default sample config ships disabled.
- P2: remote SSH read-only checks to an Ubuntu server using existing SSH keys/agent, explicit allowlist and user consent. Do not bundle a privileged remote daemon or store plaintext credentials.

### 5.7 Optional NVIDIA GPU panel — P1

- Detect GPU support at runtime; do not require NVIDIA libraries to launch.
- For supported NVIDIA driver and GPU, show utilization %, memory used/total, temperature, power and fan speed **only if reported**.
- Integrate `nvml-wrapper` via optional lazy-loaded adapter; handle missing symbols and old/unsupported GPU driver APIs.
- Do not claim accurate per-process GPU use without a verified implementation.
- Default refresh >=2 s; avoid loading and rescanning GPU devices per UI frame.
- P2: multi-GPU selector and basic GPU process list when available.

### 5.8 Diagnostics / "What changed?" — P1

- Show top CPU/memory changes vs previous interval (clearly labeled `Δ`), newly seen and exited processes.
- Time-window comparisons are **in-memory during this session only**; bounded ring buffers with no DB.
- Allow `systui snapshot --json` to output a portable summary (selected metrics and top processes), omitting sensitive args/env, usernames and network addresses by default.
- Export includes collection time, OS/architecture, collector status and semantic schema version.
- P2: opt-in JSONL recording with retention and size limits; no background recorder by default.

## 6. Essential keyboard controls

| Key | Context | Function |
| --- | --- | --- |
| `1`–`6` | Any normal view | Navigate view tabs; disabled/unavailable tabs show reason. |
| `Tab`, `Shift+Tab` | All | Cycle focus between major panes. |
| `j`/`k`, `↑`/`↓` | List focused | Move selection. |
| `PgUp`/`PgDn`, `Home`/`End` | List | Navigate long lists. |
| `/` | Any searchable view | Focus filter input. |
| `Esc` | Input/dialog/detail | Cancel / close one layer. |
| `Enter` | Selected entity | Open detail. |
| `s` | Sortable list | Cycle available sort keys. |
| `Shift+S` | Sortable list | Toggle sort order. |
| `Space` | Main UI | Pause/unpause visible live updates (sampling behavior documented). |
| `r` | Main UI | Request one immediate refresh, respecting minimum safe collector interval. |
| `Ctrl+K` | Main UI | Open fuzzy searchable command palette. |
| `?` | Main UI | Help/shortcut panel. |
| `K` | Selected process | Request terminate confirmation. |
| `q` | Main UI | Quit safely. |

Prevent shortcuts from triggering while typing in search or modal text fields. Palette searches **known internal commands only**; it must never parse or run arbitrary shell text. `Ctrl+C` should exit gracefully.

## 7. Notifications and alert thresholds

P1 only. A lightweight, non-intrusive indicator may highlight:
- CPU above a configurable threshold for **N successive samples**.
- RAM above a threshold.
- Disk free space below a threshold.
- A configured service becomes unhealthy after retries.
- A monitored container unexpectedly exits.

Rules:
- Default thresholds: CPU >=90% sustained for 15 s; RAM >=90% for 15 s; volume free <10%; service 3 consecutive failures. User-configurable.
- Apply hysteresis and cool-down to avoid flicker; do not use OS notifications by default.
- No unsupported alerts from missing data.
- Separate alerts from warnings about missing/permission-denied metrics.

## 8. Performance requirements

The application is intended to be open while working, building containers, coding, or SSH-ing into a server. It must not materially perturb the system being measured.

### 8.1 Targets (benchmarks, not unconditional promises)

| Metric | Target | Measurement method |
| --- | --- | --- |
| App idle CPU | <1% of one logical CPU average on test workstation | Sample externally over 5 min with default refresh. |
| Resident memory | <60 MiB typical, <100 MiB with optional P1 panels | Measure 5 min with 300+ processes. |
| Interactive startup | <500 ms typical before optional adapters; first CPU % may need warm-up | Instrument startup and first TUI draw. |
| UI input response | <50 ms p95 without overloaded collector | Timed synthetic keyboard events. |
| Render cost | <16 ms p95 at 120x40, 300+ processes | Internal optional benchmark/test backend. |
| Basic refresh cadence | 1000 ms default, configurable 500–5000 ms | Monotonic clock, not wall-clock assumptions. |
| Optional Docker / GPU cadence | 2–5 s; rate-limited | Per-collector cadence. |
| Optional health cadence | 15–30 s | Per-collector cadence with timeout. |

The 1% CPU target is aspirational and must be measured on both Windows and Linux rather than declared met from compilation alone. Report benchmark hardware, OS, process count, intervals and limitations in `PERFORMANCE.md`.

### 8.2 Required strategies

- Separate **collection**, **state aggregation**, **render**, and **input**; don't do blocking collector calls from `draw()`.
- Use targeted `sysinfo` refresh types; avoid `System::new_all()` on every tick and expensive full-detail refreshes per process.
- Keep a long-lived `sysinfo::System` across samples and warm up delta-dependent CPU counters; respect minimum sample interval.
- Cache static per-process metadata (exe path, start time, etc.) and refresh only when necessary; don't fetch entire environment/command line in a 1 s hot loop.
- Per-process entries keyed by (PID, creation/start time) for safety against reuse; prune stale snapshots.
- Only recalculate expensive sorts and lists on new snapshots / changed filters.
- Render on meaningful state/input changes; coalesce rapid input; avoid busy-wait loops.
- Bounded channel queues or last-value-wins snapshots so laggy UI consumers cannot accumulate unlimited backlog.
- Use bounded `VecDeque` histories: e.g. at most 300 samples per chart and a bounded number of tracked process histories.
- No unbounded stdout/stderr capture from external programs.
- Avoid polling Docker, WSL, Git, GPU, or services when corresponding modules are disabled.
- Use timeouts and cancellation for optional external commands; one slow module must never freeze core monitoring.

## 9. Architecture and folder structure

Keep the MVP a **single native executable** with separate modules rather than a distributed system.

```text
systui/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml                 # pin tested toolchain if needed
├── README.md
├── SPEC.md
├── CLAUDE.md
├── TASKS.md
├── DECISIONS.md
├── PERFORMANCE.md                     # added when benchmarks exist
├── config/
│   └── config.example.toml
├── src/
│   ├── main.rs                         # CLI entry and exit handling
│   ├── cli.rs                          # clap-style arguments/commands
│   ├── app.rs                          # application state/reducer
│   ├── event.rs                        # input and timer events
│   ├── config.rs                        # validated user settings
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── metrics.rs                   # typed values and availability
│   │   ├── process.rs                   # stable process identity
│   │   └── snapshot.rs                  # immutable sample view
│   ├── collectors/
│   │   ├── mod.rs                       # Collector trait, schedules
│   │   ├── system.rs                    # CPU/memory/processes
│   │   ├── network.rs                   # per-interface counters
│   │   ├── storage.rs                   # volumes and mounts
│   │   ├── gpu.rs                       # optional NVIDIA adapter
│   │   ├── docker.rs                    # optional Docker adapter
│   │   ├── wsl.rs                       # Windows-only optional adapter
│   │   └── health.rs                    # optional health checks
│   ├── ui/
│   │   ├── mod.rs                       # central render router
│   │   ├── theme.rs                     # colors/styles/fallbacks
│   │   ├── layout.rs                    # size-dependent layout
│   │   ├── widgets/                     # bars, sparkline, footer, modal
│   │   └── views/
│   │       ├── overview.rs
│   │       ├── processes.rs
│   │       ├── network.rs
│   │       ├── storage.rs
│   │       ├── developer.rs
│   │       └── services.rs
│   ├── actions/
│   │   └── process.rs                   # gated process operations
│   └── util/
│       ├── format.rs                    # consistent units / formatting
│       └── time.rs                      # monotonic sample deltas
├── tests/
│   ├── config_tests.rs
│   ├── process_identity_tests.rs
│   ├── rate_tests.rs
│   ├── navigation_tests.rs
│   └── snapshot_schema_tests.rs
└── scripts/
    ├── install.ps1
    └── install.sh
```

The folder structure is a proposal: simplify unnecessary files, but **preserve** the boundaries between collectors, state, UI, actions, and integrations. Avoid unnecessary layers, interfaces for one trivial function, macro-heavy abstraction, dynamic plugins, and async runtimes until measurable need exists.

### 9.1 Core contracts

Implement equivalents of these semantic types (actual Rust API names at Claude's discretion):

```rust
struct Metric<T> {
    value: Option<T>,
    observed_at: SystemTime,
    source: MetricSource,
    state: MetricState, // Available, Unsupported, PermissionDenied, Stale, Error
}

struct ProcessIdentity {
    pid: u32,
    started_at: Option<SystemTime>, // treat absence as reduced safety, not identity proof
}

struct SystemSnapshot {
    sampled_at: SystemTime,
    cpu: CpuMetrics,
    memory: MemoryMetrics,
    processes: Vec<ProcessSample>,
    network: Vec<NetworkInterfaceSample>,
    volumes: Vec<VolumeSample>,
    collector_diagnostics: Vec<CollectorStatus>,
}
```

- Data snapshots are immutable once published to UI.
- Rates always derived from two samples with monotonic elapsed duration and counter-reset handling.
- No silent `unwrap_or(0)` for missing values.
- UI state contains current tab, focused pane, sort, filter, selected process identity, modal state, and pause state; it does **not** own operating-system collector handles.
- The process action layer is the only layer allowed to terminate processes.

### 9.2 Sampling and scheduling

- Start with a single worker thread for `sysinfo` core snapshots; event-driven terminal/input loop. Other collectors may use separate low-frequency workers or carefully bounded tasks.
- Prefer standard library channels and threads if sufficient. Add `tokio` only if the chosen integrations genuinely benefit and document the reason.
- Timestamp every sample. Use `Instant` for interval calculation; `SystemTime` for display/export.
- On resume from pause, do not display multi-second old rates as current. `Space` freezes the view, not necessarily all sampling; document that choice. Default: sampling continues and history is bounded while display is paused; visually label the view `PAUSED`.
- On window focus/resize, avoid expensive reinitialization.

### 9.3 Platform adapters

Use platform-independent Rust data models, with `cfg(target_os = "windows")` and `cfg(target_os = "linux")` where needed. Different APIs or missing permissions must not break the other platform. `sysinfo` handles cross-platform basics; platform-specific OS APIs or well-bounded commands are only for missing features.

For external commands, use fixed executable + argument arrays (`std::process::Command`), **never shell interpolation** of config values. Add explicit deadlines; prevent or terminate hung subprocesses. Treat output as untrusted data. Never leak secrets from command lines or environment in JSON exports.

## 10. Recommended dependencies

Check official docs and use compatible versions (do not blindly pin anything mentioned here if newer stable incompatible APIs exist):

- `ratatui` — layout, widgets, drawing, test backend.
- `crossterm` — terminal keyboard events and backend if not already reexported compatibly.
- `sysinfo` — CPU, memory, processes, storage, network where supported.
- `clap` — CLI flags and subcommands.
- `serde`, `serde_json`, `toml` — config and safe snapshot serialization.
- `directories` or `dirs` — platform-aware user config paths.
- `thiserror` or `anyhow` — error boundaries; avoid excessive anyhow in domain contracts.
- `tracing` (optional) — debugging to a file only when explicitly enabled.
- `nvml-wrapper` (optional P1) — NVIDIA GPU stats, dynamically initialized.
- A maintained fuzzy matcher (optional) or small custom subsequence matcher for Ctrl+K.

**Review dependencies for footprint**. Disable large default features when appropriate; keep one terminal backend; inspect release size and startup costs before adopting libraries. Use Cargo audit/deny if available but don't make internet-dependent verification a mandatory offline build step.

## 11. Configuration

Use platform-standard per-user configuration (`%APPDATA%`/`%LOCALAPPDATA%` as appropriate on Windows; XDG config path on Linux). Do **not** demand a config file for basic usage. `systui doctor` prints the resolved paths.

Example `config/config.example.toml`:

```toml
[ui]
theme = "dark"                    # dark | light | mono | auto
unicode = "auto"                  # auto | always | never
refresh_ms = 1000
history_samples = 300
start_view = "overview"
show_clock = true

[processes]
default_sort = "cpu"
cpu_mode = "per_core_equivalent"   # may exceed 100%; label in UI
confirm_terminate = true

[modules]
gpu = true                        # safe detection; hidden if unsupported
docker = false                    # opt-in
wsl = false                       # opt-in, Windows host only
health = false                    # opt-in

[alerts]
enabled = false
cpu_percent = 90
memory_percent = 90
disk_free_percent_below = 10

[[health.checks]]
name = "Local API (example — disabled)"
type = "http"
url = "http://127.0.0.1:7141/health"
enabled = false
interval_seconds = 30
timeout_ms = 1000

[developer]
process_families = ["dotnet", "node", "pnpm", "npm", "cargo", "rustc", "git", "docker"]

[wsl]
# Explicit path list only; no implicit broad file search.
tracked_vhdx_paths = []
```

Config loading:
- Apply safe defaults when file absent.
- Show **actionable** validation errors for invalid typed values (e.g. zero refresh interval).
- Constrain user values to safe ranges and reject impossible configurations.
- Unknown keys may warn but should not break everyday use unless security-sensitive.
- Explicit command-line options override file settings.
- Persist UI preferences only on explicit save action in P1; avoid rewriting config every refresh.
- Don't store passwords, tokens, private keys, or service credentials in config.

## 12. Safety, privacy & security

1. The application performs local metrics collection only by default. No telemetry, no analytics, no update pings, no login, no cloud connectivity.
2. The Docker, WSL, service, remote SSH, and optional directory-scan adapters are disabled by default or run only after explicit user choice, as specified.
3. **Dangerous actions:** require exact process identity revalidation, visible confirmation with name/PID, and scoped permissions. Guard PID reuse. A successful "signal sent" is not necessarily process exited; refresh and confirm actual state.
4. Any external command receives a fixed executable/arg list and explicit timeout; no system shell from text input.
5. Do not auto-elevate to Administrator/root or prompt for UAC/sudo on startup.
6. Hide or redact secrets from CLI args, environment variables, command output, health check URLs, and exports. Do not collect environment variables as a routine monitoring operation.
7. No arbitrary HTTP headers or authentication secrets are required for MVP health checks. Expand only after threat modeling.
8. Handle maliciously long/unusual process names, Unicode, terminal control characters and ANSI escapes as plain sanitized text; prevent terminal injection.
9. Logs are off by default; diagnostic logging, when enabled, goes to an OS-appropriate per-user directory with file-size limit and sensitive-data redaction.
10. `snapshot --output` must not silently overwrite an existing file unless an explicit force flag is present; restrict output to local file paths provided by user.

## 13. Accessibility & robustness

- All important states available as text, not only color.
- High contrast dark/light/mono modes; predictable keyboard controls.
- No hard requirement for custom fonts or terminal image protocols.
- Unicode fallback, constrained text truncation and escape sanitization.
- Graceful behavior during sleep/resume, permissions changes, disappearing processes or drives, Docker unavailable, unresponsive subprocess, window resize, SSH disconnect.
- Clean terminal restore on `q`, `Ctrl+C`, error, normal panic handling, as far as OS allows. Never leave a broken hidden cursor or permanently raw-mode shell after ordinary failures.
- On first CPU sample, show `warming up` rather than a misleading 0%.
- Optional modules may fail independently without crashing the TUI.

## 14. Validation and automated tests

Implement these tests with injected samples/mocks where real hardware is not available:

### Core unit tests (P0)
- Rate calculation: two samples, elapsed time, counter wrap/reset, no first delta, overflow handling.
- Unit formatting: bytes vs bytes/s, KiB/MiB/GiB, percentage formatting, `None`/stale.
- CPU normalizations and column labels, including >100% per-core process use.
- Process identity: PID reused between selection and termination => reject.
- Sort/filter: stable selection after reorder, search input focus, empty results.
- Config defaults, precedence and invalid bounds.
- Resizing to tiny terminal does not panic; selected row stays bounded.
- JSON snapshot schema is stable, and sensitive fields absent by default.

### TUI integration tests (P0)
- Render Overview and Processes using Ratatui `TestBackend` at wide, standard, compact and tiny dimensions.
- Keyboard tab switching, search, detail, help, command palette, cancel, quit.
- Error states: inaccessible process, no available network, no disk mounts, missing optional adapters.
- Confirmation: kill cannot occur on one mistaken key, process changes or stale identity.
- Terminal restore smoke test, where platform test environment permits.

### Manual system smoke tests (P0)
- Windows Terminal/PowerShell startup, 5-minute run, quit and shell restoration.
- Ubuntu/SSH startup, 5-minute run, quit and shell restoration.
- CPU-intensive short-lived process appears, uses CPU, then disappears without stale rows.
- Memory-intensive process updates RSS correctly within expected sampling delay.
- Network traffic produces nonzero selected-interface rates; rates return toward idle.
- Disk mounting/removal or an inaccessible mount doesn't panic.
- Rapid terminal resizes and repeated search/sort/navigation do not freeze.
- Test process termination on a **controlled disposable test process only**; never kill critical services to satisfy tests.
- `doctor`, `--help`, `--version`, and `snapshot --json` work from CI/noninteractive shell.

### Performance tests (P0)
- Collect startup time and 5-minute idle CPU/RSS externally.
- Repeat with >=300 processes or with synthetic test data if actual host lacks enough.
- Measure p95 render and input time; record environment and failures.
- Compare optional adapters enabled/disabled; document overhead in `PERFORMANCE.md`.

## 15. Installation, packaging & documentation

### Windows
- Compile and release `systui.exe` for `x86_64-pc-windows-msvc`.
- `scripts/install.ps1` installs to a **per-user** bin directory and adds it to user `PATH` with consent; avoid machine-wide changes/admin elevation.
- Provide manual instructions to place the executable on `PATH` (for example `%USERPROFILE%\\.local\\bin`) and restart terminal.
- Document `cargo install --path . --locked` as developer option (requires Rust toolchain).

### Linux
- Compile release for common Linux GNU target(s), noting actual libc portability, rather than claiming any Linux build runs anywhere.
- `scripts/install.sh` installs per-user to `~/.local/bin` and gives shell `PATH` guidance, no `sudo` requirement.
- Document `cargo install --path . --locked` for developer build.

### README must cover
- 30-second intro, screenshot/terminal capture **from implemented UI** (not fabricated), prerequisites.
- Install and `systui` launch commands for Windows and Ubuntu.
- Keybind reference and all view descriptions.
- CPU / memory / network unit semantics, permissions, platform capabilities.
- Opt-in integrations and sample config.
- Security and privacy behavior; how to disable external commands.
- Troubleshooting, `systui doctor`, known limitations.
- Benchmark status and accurate list of completed features.

## 16. Implementation milestones (execute in sequence)

### M0 — Foundation (P0)
- Create Cargo project, lockfile, selected versions and tooling.
- Implement CLI, safe config defaults, app state, terminal lifecycle, quit/help, static responsive skeleton.
- Create tests for layout and terminal event lifecycle; verify `cargo fmt`, `cargo clippy`, `cargo test`.
- **Exit criteria:** `systui` renders tabs and gracefully exits on Windows or Linux target; `--help`/`--version` work.

### M1 — Real-time machine telemetry (P0)
- Implement persistent `sysinfo` collector, CPU warm-up, RAM, disks, network rates and histories.
- Live Overview with actual metrics and strong error states.
- **Exit criteria:** accurate values validated against OS tools within expected sampling/normalization differences; no blocking TUI.

### M2 — Full process explorer (P0)
- All-process listing, detail pane, sort, filter, stable identity selection and confirmation-protected terminate action.
- **Exit criteria:** controlled process tests and PID-reuse safety tests pass; keyboard navigation works with 300+ synthetic processes.

### M3 — UI polish & scriptability (P0)
- Command palette, 4 terminal size tiers, themes/mono mode, one-shot JSON snapshot, doctor, benchmark and installer scripts.
- **Exit criteria:** meets documented P0 test checklist, package can be run as `systui`, baseline perf reported.

### M4 — Developer essentials (P1)
- Developer process families, read-only Docker container stats and disk summary, Windows-host WSL distro panel and user-configured VHDX size, and NVIDIA GPU adapter. Deliver independently; skip unsupported feature cleanly.
- **Exit criteria:** each adapter has disabled/unavailable/testable states, uses separate bounded cadence and never blocks Overview.

### M5 — Practical operations (P1)
- Services/health dashboard, optional thresholds and diff snapshots, process tree/groups, per-process I/O where supported, network listener display where feasible.
- **Exit criteria:** configurable health checks and alerts verified with a local disposable server; no surprise network calls.

### M6 — Quality-of-life extensions (P2; not MVP blockers)
- Optional remote SSH read-only Linux health, Docker container log preview and confirmed actions, multi-GPU details, persistent opt-in session recording, richer process family heuristics, saved workspace/bookmark presets.
- **Exit criteria:** targeted functionality works with explicit user opt-in and isolated failure behavior.

**Do not start M4–M6 before M0–M3 function.** The deliverable is a usable system monitor first, not a large codebase of unimplemented adapters.

## 17. Release gates / definition of done

A milestone is **Done** only if:

- Relevant code exists and builds for the current target.
- Its behavior is exercised by tests and, when meaningful, manual run evidence.
- Any platform caveats are captured in `DECISIONS.md` and README.
- Relevant actions cannot silently cause damage.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass or an exact, justified exception is documented.
- No static/mock metrics are presented as live in normal execution.
- No P1 feature is claimed in release notes unless demonstrated.

**MVP acceptance:** A user downloads/installs the native executable, types `systui`, gets a functioning, navigable Overview/Processes/Network/Storage monitor, can sort/filter/inspect processes, sees clear failures for unsupported metrics, and exits without disrupting the terminal.

## 18. Developer-specific additions to prioritize

The following are **proposals based on the intended development workflow**, not assumptions that a particular service is always installed or running:

| Value | Extension | Why it's useful | Priority |
| --- | --- | --- | --- |
| Very high | Docker containers + build cache footprint | Spot container RAM/CPU load and identify space occupied by images/build cache. | P1 |
| Very high | WSL host view + configured `.vhdx` sizes | Distinguish Linux guest, Docker virtual disk, and host storage use without destructive cleanup. | P1 |
| High | .NET / Node / pnpm / Cargo process families | Identify runaway builds, duplicated dev servers and excessive build workers quickly. | P1 |
| High | NVIDIA GPU / VRAM | Monitor resource contention during local model inference or GPU workloads. | P1 |
| High | Custom health endpoints / service state | Inspect local APIs, Portainer and build agents while developing/deploying. | P1 |
| Medium | Tailscale / local TCP ports | Check basic network reachability for home lab without building a network analyzer. | P2 |
| Medium | Diagnostic snapshots / deltas | Save a sanitized evidence file when CPU/RAM spikes occur. | P1 |
| Medium | Battery, temperature, fan, thermal throttling | Valuable on a laptop/home lab where supported; absent when sensors unavailable. | P2 |

## 19. External technical references

Official/maintainer documentation to consult during implementation:

- Ratatui: https://ratatui.rs/ and https://docs.rs/ratatui/latest/ratatui/
- Ratatui examples: https://ratatui.rs/examples/
- sysinfo: https://docs.rs/sysinfo/latest/sysinfo/
- sysinfo process CPU and minimum refresh interval: https://docs.rs/sysinfo/latest/sysinfo/struct.Process.html
- NVIDIA NVML wrapper: https://docs.rs/nvml-wrapper/latest/nvml_wrapper/
- WSL host commands: https://learn.microsoft.com/en-us/windows/wsl/basic-commands
- Docker stats reference: https://docs.docker.com/reference/cli/docker/container/stats/
- Standard app config paths: https://docs.rs/directories/latest/directories/struct.ProjectDirs.html

---

**Final instruction to coding agent:** Implement the smallest complete, credible native monitoring product that meets all P0 requirements. Prefer a measured working executable over attractive screenshots, speculative abstractions, or stubbed integrations. Report exactly what works, what has been tested, and what remains.

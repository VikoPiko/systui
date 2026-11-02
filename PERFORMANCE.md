# SysTUI performance report

Measured, not promised. Every number below came from running the commands shown.

## Environment

| | |
| --- | --- |
| Date | 2026-11-02 |
| OS | Windows 11 Pro (x86_64), unelevated shell |
| CPU | Intel Core i7-8700K, 12 logical CPUs |
| RAM | 31.9 GiB |
| Processes | 327–335 during the runs |
| Build | `cargo build --release` (opt-level 3, thin LTO), rustc 1.99.1 |
| Linux | **not measured** (no Linux host was available) |

## Method (and what it does NOT include)

`cargo run --release --example bench -- <seconds> <refresh_ms>` drives the **real**
pipeline headlessly: collector thread → `App::ingest` → full render into ratatui's
`TestBackend` (120×40), with the same 100 ms idle wake-up the real event loop uses.
CPU time is this process's own accumulated CPU time (all threads) read through
`sysinfo`, divided by wall time.

It does **not** include the cost of writing escape sequences to a real terminal and the
terminal emulator's own CPU. Those were not measured. The interactive run was only
checked by hand for correctness, not profiled.

`cargo run --release --example sample_cost` times each underlying `sysinfo` call.

## Results

### Steady-state CPU and memory (60 s each, default views)

| refresh_ms | CPU of one logical CPU | RSS (end / peak) |
| ---: | ---: | ---: |
| 1000 (default) | **2.81 %** | 16.5 / 16.6 MiB |
| 2000 | 1.64 % | 16.4 / 16.5 MiB |
| 5000 | 0.86 % | 16.3 / 16.4 MiB |

* **Spec target "<1 % of one logical CPU at default refresh" is NOT met** (2.81 %).
  It is met only at 5 s refresh. Whole-machine impact is ≈0.23 % of a 12-thread CPU.
* **Spec target "<60 MiB RSS" is met** (≈16.5 MiB with 330 processes).

### Where the CPU goes (per refresh, release build, 327 processes)

| call | ms |
| --- | ---: |
| `refresh_processes` (cpu + memory + exe if unset) | ~28 |
| ↳ enumerating the process list alone | ~10 |
| `Networks::refresh` | ~14 |
| memory | ~1 |
| CPU usage | 0.1 |
| volumes (only every 10 s) | 0.6 |
| our ingest (sort, histories) | 0.03 |
| our render (any view) | ~0.6 |

So the cost is the OS enumeration done by `sysinfo` on Windows, not the TUI.
Reducing it would need either a lower refresh rate (above) or fewer processes sampled;
neither was done because the default cadence is part of the spec. (The 1 s run above
costs ~28 ms of CPU per second, while the micro-timings sum to ~43 ms per refresh; I did
not investigate the difference, so treat the per-call table as indicative only.)

### Latency (release, 120×40, 330 processes, 500 samples each)

| operation | p50 | p95 | max |
| --- | ---: | ---: | ---: |
| render Overview | 0.63 ms | 1.10 ms | 1.7 ms |
| render Processes | 0.71 ms | 1.23 ms | 1.8 ms |
| render Network | 0.59 ms | 1.04 ms | 1.8 ms |
| render Storage | 0.57 ms | 1.14 ms | 4.7 ms |
| keypress + redraw (Processes) | 0.77 ms | 1.30 ms | 1.8 ms |
| ingest new snapshot | 0.031 ms | 0.038 ms | 0.06 ms |

Targets: render p95 < 16 ms ✔, input p95 < 50 ms ✔ (both headless; a real terminal adds
its own latency).

### Startup

Time from process start to first sample **and** first drawn frame (headless): **431 ms**
(target <500 ms ✔). The very first CPU % is "warming up" for ~0.5 s by design.

## Reproduce

```
cargo build --release --examples
target/release/examples/bench      60 1000
target/release/examples/sample_cost
```

## Not measured / caveats

* Linux and SSH sessions.
* Real-terminal I/O cost, and a full 5-minute run (60 s runs were used).
* Behavior on a loaded machine; other apps were running normally during these runs, so
  numbers have some noise (run-to-run variation of the 1 s case was 2.8–5.3 % before I
  fixed a bug in the benchmark's own memory probe; the 2.81 % figure is the corrected one).
* Optional adapters: see the M4 section below. Docker was only measured with one workload (32 running containers); Dev-tab render time itself was not benchmarked.

## M4 modules: overhead (Docker + WSL + GPU enabled, Dev tab open)

`BENCH_MODULES=1 target/release/examples/bench 60 1000` — same method as above, with all three
modules enabled and the Dev tab kept open for the whole run (this host runs 32 containers).

| | modules off | modules on, Dev tab open |
| --- | ---: | ---: |
| CPU of one logical CPU (this process) | 2.71 % | 3.72 % (**+1.0 pt**) |
| RSS | 16.1 MiB | 32.8 MiB (**+16.7 MiB**, NVML load + parsed container data) |
| render Processes p95 (not the Dev tab; difference is noise) | 1.2 ms | 0.8 ms |

Not included above: the CPU of the **child processes** `docker.exe` / `wsl.exe`, which run outside
this process. Their wall time per call on this host (Docker Desktop, 71 containers, 32 running):

| command | wall time | cadence |
| --- | ---: | --- |
| `docker ps -a` | ~0.5 s | every 5 s while the Dev tab is open |
| `docker stats --no-stream` | 2.1–3.2 s | every 5 s (so Docker is nearly continuously busy) |
| `docker system df` | ~2.8 s | only when you press `R` |
| `wsl.exe --list --verbose` | ~0.19 s | every 10 s |
| NVML query | in-process, sub-ms | every 2 s |

With the Dev tab closed or modules disabled the workers do not poll at all (tested). The
`ModuleSet` creates no threads until the tab is first opened. Thread count is not measured separately.

## Per-tab CPU after the "needs" optimisation (M5)

The collector now refreshes only what the visible tab shows (CPU, memory and volumes are always
sampled because alerts need them). Same method as above, 40 s runs, 1 s refresh, release build,
~330 processes, Dev modules off:

| Visible tab | Refreshes | CPU of one logical CPU | before |
| --- | --- | ---: | ---: |
| Storage | – | **0.51 %** | ~2.7 % |
| Services | – (health worker is separate) | **0.55 %** | ~2.7 % |
| Network | network every sample | **0.90 %** | ~2.7 % |
| Overview | processes every sample, network every 2 s | 2.65 % | ~2.7 % |
| Processes | processes | 2.69–2.81 % | ~2.7 % |
| Dev | processes | 2.81 % | ~2.7 % |

**What this does and does not fix.** Views that do not list processes now cost 0.5–0.9 %, which
meets the spec's <1 % target. Views that do list processes are unchanged: the OS process
enumeration and per-process CPU read dominate (~26 of ~38 ms per refresh) and `sysinfo` offers no
cheaper way. An experiment (`examples/sample_cost2.rs`) showed that refreshing only selected PIDs is
*slower* (36 ms for the 171 readable processes vs 27 ms for all), so that idea was dropped. The
network refresh turned out to cost only ~0.4 points in situ, so slowing it on Overview saved
almost nothing (2.65 vs 2.71–2.81 %, within noise). For the process views the remaining lever is
`--refresh-ms` (2 s ≈ 1.6 %, 5 s ≈ 0.9 %).

Per-process I/O adds ~1 ms per refresh (`examples/sample_cost2.rs`); it is collected only while
the `i` toggle is on. Render cost did not change: Processes p50 0.65 ms / p95 1.2 ms (tree mode and
the extra columns were not benchmarked separately).

When the collector resumes a subsystem it switched off (tab change), CPU % and network rates show
"warming up" for ~0.5 s instead of an average across the gap.

## After M6 (re-check)

Added per-sample work: battery/temperature read every 10 s, the recording hook (a no-op unless a
session is running), alert/changes bookkeeping. Re-measured, same method, 40 s, 1 s refresh:

| Tab | CPU of one logical CPU | RSS |
| --- | ---: | ---: |
| Overview | 2.73 % | 18.6 MiB |
| Storage (3 runs) | 0.47 / 0.66 / 0.70 % | 18.3 MiB |

No regression beyond run-to-run noise. RSS rose about 1.5–2 MiB versus M4 (16–17 MiB), which I attribute to the
extra code and the `windows` Power bindings but did not profile. Not measured: the cost of an active recording
session (one JSON line of a few KB every `interval_seconds` on a background thread), SSH polling (one `ssh`
process per host per interval, only while the Services tab is open and only after consent), and Docker log fetches.

## Linux (first measurements)

Ubuntu 24.04 **inside WSL2** (a VM, not bare metal), kernel 6.6.87.2, 12 logical CPUs (same i7-8700K host),
rustc 1.99.1, release build, **44 processes** (far fewer than the 330 on the Windows side, so the two are NOT
comparable as OS-vs-OS numbers). Same headless harness, 40 s each:

| Visible tab / refresh | CPU of one logical CPU | RSS |
| --- | ---: | ---: |
| Overview, 1000 ms | 0.80 % | 4.6 MiB |
| Processes, 1000 ms | 1.00 % | 4.5 MiB |
| Storage, 1000 ms | 0.17 % | 4.5 MiB |
| Overview, 5000 ms | 0.25 % | 4.4 MiB |

Render p95 ≈ 0.9–1.1 ms and keypress+redraw p95 ≈ 0.8–1.0 ms; time to first sample + first frame 16–53 ms.
The <1 % CPU target is met at the default refresh on this Linux setup, but with an unrepresentatively small
process list; the Windows process-enumeration cost (≈ 26 ms for 330 processes) is what dominates there. Not
measured: bare-metal Linux, a busy server with thousands of processes, SSH sessions, a 5-minute run.

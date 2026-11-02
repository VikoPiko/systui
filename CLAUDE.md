# CLAUDE.md — SysTUI coding-agent instructions

## Mission
Implement the product defined in [SPEC.md](SPEC.md), with a lightweight native Rust executable called `systui`. Follow M0–M3 (P0) before P1/P2. Read the full spec first.

## Non-negotiable constraints
- Build functioning functionality, not static screens or placeholder readings.
- No Electron/Node/Python/web server/daemon/cloud service/database for the MVP.
- Favor Ratatui + Crossterm + sysinfo; check selected crate APIs and versions first.
- Never block TUI rendering with hardware, process, Docker, WSL or network commands.
- Default to read-only; protect destructive actions with deliberate confirmation and process identity revalidation.
- No automatic admin/UAC/sudo, telemetry or external connection.
- Don't invent missing metrics; display unsupported/error/stale states accurately.
- No `unwrap_or(0)` for unavailable data; avoid `unwrap()` in runtime input/collector paths.
- Keep efficient polling; do not recreate `sysinfo::System` every tick.
- Preserve terminal state on quit, errors and normal panic flows.
- Sanitize terminal strings and redact exports.

## Operating workflow
1. Inspect repo and read `SPEC.md`; identify implemented vs pending work.
2. If greenfield, create `TASKS.md` from spec milestones and `DECISIONS.md` for real tradeoffs.
3. Work on **one milestone at a time**. Write tests along with feature code.
4. After changes run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and any relevant smoke tests; keep outputs/limitations in your summary.
5. Only mark a task complete when code and verification exist. Never mark Windows/Linux manually tested without running it there.
6. Prefer small, focused modules; no unnecessary plugin frameworks or speculative infrastructure.
7. Do not add an optional integration as a reason to postpone M0–M3.

## What to deliver at the end of each milestone
- Files/features implemented.
- Exact tests/commands run and their results.
- Manual verification steps, if not run.
- Known platform limitations and performance notes.
- Updated task checklist and next pending milestone.

## First action in a fresh repository
Start with M0. Select dependency versions that agree on the Crossterm backend and target toolchain. Create a usable CLI, safe terminal initialization/restoration, responsive shell with tabs and footer hints, and tests. Then proceed toward real collectors; do not produce mock numbers as live data.

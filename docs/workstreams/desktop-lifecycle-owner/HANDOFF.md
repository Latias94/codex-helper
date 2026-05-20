# Desktop Lifecycle Owner — Handoff

Status: Complete
Last updated: 2026-05-20

## Current State

M0 through M4 are complete. The workstream now has a core lifecycle domain module, owner marker metadata, best-effort owner marker lifecycle helpers, CLI/TUI/daemon/GUI lifecycle alignment, hidden desktop-managed sidecar preparation, documentation, and targeted verification.

## Completed

- Added `crates/core/src/runtime_manager.rs` with:
  - `ProxyLifecycleMode`
  - `RuntimeOwnerKind`
  - `RuntimeOwnerMarker`
  - run-dir owner marker path/read/write/clear helpers
  - explicit normal-exit descriptions
  - `RuntimeConnectionMode`, `RuntimeStopIntent`, `RuntimeStopAction`
  - `decide_runtime_stop_action`
  - focused unit tests
- Exported `runtime_manager` from core, root crate, and GUI crate.
- Added hidden `serve --desktop-managed` flag for future desktop/tray sidecar preparation.
- Added hidden `serve --supervisor-managed` flag for `daemon supervise` child processes.
- `run_server` treats desktop-managed as resident and writes `Desktop` owner marker.
- `run_server --resident` writes `ManualCli` owner marker.
- `daemon supervise` writes `Supervisor` owner marker and starts child `serve` with supervisor ownership so the child does not overwrite status as manual CLI.
- Owner marker cleanup is best-effort on normal drop/supervisor exit.
- `daemon status` surfaces owner metadata when available.
- `daemon status` reads owner markers best-effort, so corrupt/stale marker metadata cannot break status output.
- Attached TUI and help copy describe attached observer semantics: `q` exits only the console and leaves the resident proxy running.
- GUI `stop` / `stop_owned` now routes owned/attached stop semantics through core runtime manager decisions:
  - owner exit + attached => detach only;
  - explicit stop + attached shutdown API => remote shutdown;
  - owned runtime => stop owned runtime.
- GUI window close, app `on_exit`, and tray Quit use owner-exit semantics (`stop_owned`), so normal GUI exit cannot remote-stop an attached runtime owned by someone else.
- GUI tray / page explicit Stop Proxy still uses explicit-stop semantics (`stop`), so users can intentionally stop an attached runtime when the runtime advertises the shutdown API.
- README/README_EN/config docs document simple default behavior, advanced resident/attached behavior, owner markers, and the hidden future desktop sidecar path.

## Validation

- `cargo fmt --check`
- `cargo check -p codex-helper-core`
- `cargo check -p codex-helper`
- `cargo check -p codex-helper-gui`
- `cargo check -p codex-helper-tui`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
- `cargo nextest run -p codex-helper-gui lifecycle_defaults --no-fail-fast`
- `cargo nextest run -p codex-helper-tui attached --no-fail-fast`
- Final closeout rerun:
  - `cargo fmt --check`
  - `git diff --check`
  - `cargo check -p codex-helper-core -p codex-helper-gui -p codex-helper-tui -p codex-helper`
  - `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
  - `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
  - `cargo nextest run -p codex-helper-gui lifecycle_defaults --no-fail-fast`
  - `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
  - `cargo nextest run -p codex-helper-tui attached --no-fail-fast`

## Next Task / Follow-ons

- Optional: split a future Tauri/tray workstream if we want a visible always-on desktop owner.
- Optional: add live process tests for owner marker startup/cleanup once test harness can spawn configured proxy listeners safely.
- Optional: add a first-class OS service/autostart story; out of scope for this workstream.

## Risks / Notes

- Do not reintroduce silent attach-first as default.
- Owner marker writes are intentionally best-effort; do not make them fatal for proxy startup/shutdown.
- Full Tauri app and OS service installation remain out of scope.
- Existing uncommitted resident/attach-first changes are still present in the working tree.

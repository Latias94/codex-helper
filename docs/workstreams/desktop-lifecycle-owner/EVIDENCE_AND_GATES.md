# Desktop Lifecycle Owner — Evidence And Gates

Status: Complete
Last updated: 2026-05-20

## Required Gates

- `cargo fmt --check`
- `cargo check -p codex-helper-core`
- `cargo check -p codex-helper-gui`
- `cargo check -p codex-helper-tui`
- `cargo check -p codex-helper`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
- `cargo nextest run -p codex-helper-gui lifecycle_defaults --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo nextest run -p codex-helper-tui attached --no-fail-fast`

## Evidence Log

### 2026-05-20 — Workstream opened

- Created `docs/workstreams/desktop-lifecycle-owner/`.
- Froze target state: simple default remains ephemeral; resident/desktop sidecar modes are explicit.

### 2026-05-20 — DLO-020 lifecycle domain

Commands:

- `cargo fmt`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`

Result:

- PASS — 4 runtime_manager tests.

Evidence:

- `crates/core/src/runtime_manager.rs`
- `crates/core/src/lib.rs`

### 2026-05-20 — DLO-030 owner markers and CLI status integration

Commands:

- `cargo check -p codex-helper-core`
- `cargo check -p codex-helper`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo fmt --check`

Result:

- PASS — core check.
- PASS — root package check.
- PASS — 4 runtime_manager tests.
- PASS — 14 CLI/supervisor targeted tests.
- PASS — fmt check.

Evidence:

- `crates/core/src/runtime_manager.rs`
- `src/cli_app.rs`
- `src/cli_types.rs`
- `src/lib.rs`

Concern:

- Live `serve --resident` owner marker integration is not process-tested because it needs real upstream config and bound listener orchestration. Covered with pure marker tests plus CLI parse/supervisor marker tests.

## Deferred / Not Run Yet

- Full workspace nextest: defer until implementation stabilizes; use targeted nextest gates during slices to avoid unnecessary memory pressure.

### 2026-05-20 — DLO-040 manager stop-decision seam

Commands:

- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
- `cargo check -p codex-helper-gui`
- `cargo check -p codex-helper-core`

Result:

- PASS — 5 runtime_manager tests.
- PASS — 38 GUI proxy_control tests.
- PASS — GUI package check.
- PASS — core package check.

Evidence:

- `crates/core/src/runtime_manager.rs`
- `crates/gui/src/gui/proxy_control.rs`
- `crates/gui/src/lib.rs`

Notes:

- `RuntimeStopIntent::OwnerExit` + attached connection now resolves to detach-only.
- `RuntimeStopIntent::ExplicitStop` + attached shutdown API resolves to remote shutdown.

### 2026-05-20 — DLO-050 CLI/TUI lifecycle copy and status owner semantics

Commands:

- `cargo check -p codex-helper-tui`
- `cargo nextest run -p codex-helper-tui attached --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo fmt --check`

Result:

- PASS — TUI package check.
- PASS — 4 attached TUI lifecycle/copy tests.
- PASS — 15 CLI/supervisor targeted tests.
- PASS — fmt check.

Evidence:

- `src/cli_app.rs`
- `src/cli_types.rs`
- `crates/tui/src/tui/attached.rs`
- `crates/tui/src/tui/view/modals/help.rs`
- `crates/tui/src/tui/view/modals/tests.rs`

Notes:

- `daemon status` now reads owner markers best-effort so stale/corrupt marker metadata cannot break status output.
- `daemon supervise` launches children with hidden `--supervisor-managed`, avoiding child `ManualCli` markers overwriting supervisor ownership.
- Attached TUI toast/help copy now names attached observer semantics: `q` exits only the console and leaves the resident proxy running.

### 2026-05-20 — DLO-060 desktop-managed sidecar preparation

Commands:

- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`

Result:

- PASS — 7 runtime_manager tests.
- PASS — 15 CLI/supervisor targeted tests.

Evidence:

- `crates/core/src/runtime_manager.rs`
- `src/cli_types.rs`
- `src/cli_app.rs`
- `README.md`
- `README_EN.md`
- `docs/CONFIGURATION.md`
- `docs/CONFIGURATION.zh.md`

Notes:

- Hidden `serve --desktop-managed` is explicit and non-default; it implies resident behavior and writes `RuntimeOwnerKind::Desktop` / `ProxyLifecycleMode::DesktopOwned`.
- Full desktop/Tauri shell, tray quit orchestration, OS service install, and autostart are deferred follow-ons.

### 2026-05-20 — DLO-070 final targeted gates

Commands:

- `cargo fmt --check`
- `cargo check -p codex-helper-core`
- `cargo check -p codex-helper-tui`
- `cargo check -p codex-helper`
- `cargo check -p codex-helper-gui`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
- `cargo nextest run -p codex-helper-gui lifecycle_defaults --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo nextest run -p codex-helper-tui attached --no-fail-fast`

Result:

- PASS — fmt check.
- PASS — all four package checks.
- PASS — 7 runtime_manager tests.
- PASS — 38 GUI proxy_control tests.
- PASS — 1 GUI lifecycle default test.
- PASS — 15 CLI/supervisor targeted tests.
- PASS — 4 attached TUI tests.

Evidence:

- Workstream task ledger marked DLO-050, DLO-060, and DLO-070 complete.
- README/README_EN/config docs document default simple ownership, advanced resident/attached mode, best-effort owner marker metadata, and hidden future desktop sidecar semantics.

Skipped broader gate:

- Full workspace `cargo nextest run --workspace` not run in this slice to avoid unnecessary memory pressure on this large workspace; targeted gates cover modified lifecycle modules and adapters.

### 2026-05-20 — DLO-070 closeout audit

Commands:

- `cargo nextest run -p codex-helper-gui proxy_control --no-fail-fast`
- `cargo nextest run -p codex-helper-gui lifecycle_defaults --no-fail-fast`
- `cargo fmt --check`
- `git diff --check`
- `cargo check -p codex-helper-core -p codex-helper-gui -p codex-helper-tui -p codex-helper`
- `cargo nextest run -p codex-helper-core runtime_manager --no-fail-fast`
- `cargo nextest run -p codex-helper cli_types supervisor_tests --no-fail-fast`
- `cargo nextest run -p codex-helper-tui attached --no-fail-fast`

Result:

- PASS — GUI closeout policy tests still pass after the final close-path audit.
- PASS — GUI lifecycle defaults remain non-background-owner by surprise.
- PASS — fmt check.
- PASS — diff whitespace check; Git reported only existing LF/CRLF working-copy warnings.
- PASS — all four package checks in one command.
- PASS — 7 runtime_manager tests.
- PASS — 15 CLI/supervisor targeted tests.
- PASS — 4 attached TUI tests.

Evidence:

- `crates/gui/src/gui/app.rs`

Notes:

- Final audit found GUI window close and tray Quit still using explicit-stop semantics. Both now call `stop_owned`, so normal GUI exit stops only GUI-owned runtimes and detaches from attached runtimes without remote shutdown. The explicit Stop Proxy action still uses `stop`.

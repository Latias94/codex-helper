# Resident Proxy And Attach-First Operator Consoles

Status: Closed
Last updated: 2026-05-20

## Why This Lane Exists

codex-helper is becoming a long-running local compatibility layer, not just an interactive terminal
command. The proxy should survive ordinary operator-console exits, terminal failures, and UI
restarts. The TUI and GUI should become attachable control surfaces rather than owners of the
proxy's lifetime.

## Relevant Authority

- Existing docs:
  - `docs/GUI_REFACTOR_PLAN.md`
  - `docs/GUI_PROGRESS.md`
  - `README.md`
  - `README_EN.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
- Related workstreams:
  - `docs/workstreams/codex-tui-operator-polish/`
  - `docs/workstreams/codex-control-plane-refactor/`
  - `docs/workstreams/codex-architecture-deepening/`

## Problem

The current `codex-helper serve` path starts the proxy/admin listeners and the interactive TUI in
one process. That gives a simple one-command experience, but it also means the proxy lifetime is
tightly coupled to a terminal UI. If the process aborts under memory pressure or the terminal UI
exits, Codex can remain pointed at a local port that is no longer serving traffic. GUI integrated
mode has the same architectural shape when it hosts the proxy in-process.

## Target State

- A **resident proxy** mode exists and can run without an attached UI.
- Operator consoles are **attach-first**:
  - They discover or attach to a running proxy when possible.
  - Starting a proxy is a separate, explicit lifetime decision.
  - Exiting a TUI/GUI console does not implicitly kill a resident proxy.
- A lightweight **supervisor/watchdog** can restart a resident proxy child process with bounded
  backoff and user-visible status.
- The legacy one-command ephemeral `serve` experience remains available for users who expect
  terminal exit to stop the proxy and restore client patch state.
- Control-plane API and docs make the selected lifetime mode observable.

## In Scope

- CLI surface for resident proxy, attach/status/stop, and supervisor/watchdog.
- Refactoring current `run_server` startup into reusable runtime/lifetime modules.
- TUI attach path for core observability and safe exit semantics.
- GUI default behavior moving toward attach/start-resident instead of in-process ownership.
- Runtime status/crash marker/health endpoints or fields needed for UX.
- Tests and docs for lifetime semantics.

## Out Of Scope

- Full Windows Service / launchd / systemd installation.
- Auto-updater, installer, or privileged service manager.
- Replacing existing TUI/GUI rendering.
- Remote non-loopback admin expansion beyond the current token-protected control-plane model.
- Reworking provider routing, relay compatibility, or request protocol normalization unless required
  by lifetime separation.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Admin API v1 is rich enough for an attach-first GUI and a first TUI attach proof. | High | GUI attach mode already uses capabilities, snapshot, operator summary, request ledger, control trace, and mutation APIs. | Need to add missing endpoints before UI migration. |
| Full OS service install is not needed for the first stable UX win. | Medium | Existing GUI autostart already starts a desktop companion; user request focuses on crash/long-running stability, not service installation. | Add OS service as a follow-on, not this lane. |
| The existing ephemeral `serve` behavior must remain for backward compatibility. | High | README documents `serve` and `serve --no-tui`; current Drop guard restores Codex/Claude config on exit. | Users relying on auto-restore could be surprised; keep explicit mode split. |
| A child-process supervisor is enough to recover allocator aborts. | Medium | Rust OOM abort cannot be caught in-process, but a parent process can observe child exit and restart. | If OS kills the whole process tree, only external service manager helps. |

## Architecture Direction

Use lifetime mode as the primary seam:

- `ProxyRuntime` owns the loaded config, `ProxyService`, proxy listener, admin listener, state, and
  shutdown channel.
- `EphemeralServe` keeps the current contract: start proxy, maybe TUI, restore client patch on exit.
- `ResidentServe` starts the same runtime but does not bind lifetime to an interactive UI.
- `AttachedConsole` reads/writes through admin API and treats local host files as optional
  host-local capability.
- `Supervisor` owns a child process running resident proxy and is the only component that attempts
  automatic restart.

This keeps a deep module behind a small lifetime interface: callers choose a lifetime policy without
relearning proxy construction, admin listener wiring, switch-on safety, or shutdown mechanics.

## Closeout Condition

This lane can close when:

- resident proxy mode is implemented and documented,
- an attach-first console path is shipped for the primary UI surface,
- lightweight watchdog restart behavior is implemented with bounded backoff and status evidence,
- legacy ephemeral behavior still passes targeted regression tests,
- evidence gates pass,
- and remaining work is either split or explicitly deferred.

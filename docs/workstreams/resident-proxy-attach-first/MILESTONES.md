# Resident Proxy And Attach-First Operator Consoles — Milestones

Status: Closed
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

Exit criteria:

- Problem and target lifetime model are explicit.
- Non-goals include full OS service installation.
- First implementation slice is chosen.
- Evidence gates are written before code changes.

Primary evidence:

- `docs/workstreams/resident-proxy-attach-first/DESIGN.md`
- `docs/workstreams/resident-proxy-attach-first/TODO.md`

## M1 — Resident Runtime Seam

Exit criteria:

- Proxy/admin startup has an explicit lifetime policy seam.
- Legacy ephemeral `serve` behavior remains intact.
- Resident proxy mode can run without a TUI.
- Status/stop semantics are documented or exposed through CLI.

Primary gates:

- `cargo check -p codex-helper`
- targeted core/admin tests touching proxy runtime and control plane

## M2 — Attach-First Consoles

Exit criteria:

- GUI prefers attaching to or starting a resident proxy for normal operation.
- TUI has a first attach path for core observability.
- UI exit semantics are explicit and do not surprise users.
- TUI/GUI status copy distinguishes integrated vs attached mode and explains safe exit/stop
  behavior.

Primary gates:

- `cargo nextest run -p codex-helper-gui`
- `cargo nextest run -p codex-helper-tui`

## M3 — Supervisor / Watchdog

Exit criteria:

- A lightweight supervisor can start a resident child, detect exit/health failure, restart with
  bounded backoff, and stop after repeated failures.
- Crash/restart state is user-visible through logs and lightweight crash markers.
- The solution does not require privileged OS service installation.

Primary gates:

- supervisor decision-logic tests
- `cargo check -p codex-helper`

## M4 — Closeout

Exit criteria:

- Gate set is recorded with fresh command output.
- README/config docs teach the recommended user flow.
- Remaining work is either completed, deferred, or split into a follow-on.
- `WORKSTREAM.json` status is updated.

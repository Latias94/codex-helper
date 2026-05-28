# Codex Fearless Refactor Hardening — Milestones

Status: Complete
Last updated: 2026-05-28

## M0 — Scope Frozen

Exit criteria:
- Workstream docs define problem, non-goals, tasks, and validation commands.

## M1 — Bounded Local Logs

Exit criteria:
- Runtime, GUI, request/debug/control/retry traces, and relay evidence all use the shared bounded
  store or an explicitly justified equivalent.
- Existing oversized files are repaired by startup or first append paths.
- Regression tests cover legacy rotated files, active oversized files, and runtime rotation.

## M2 — Routing Compatibility Boundary

Exit criteria:
- Manual graph/compat sync calls are removed from high-level CLI/GUI call sites where practical.
- Compatibility behavior is tested at the persisted-routing boundary.

## M3 — Request Ledger Read Model

Exit criteria:
- UI and command consumers use a ledger read-model boundary for tail and summary behavior.
- Raw request log schema assumptions are localized.

## M4 — Relay Diagnostics Split

Exit criteria:
- Live-smoke orchestration is smaller and delegates case behavior to case modules.
- Existing relay diagnostic tests pass.

## M5 — Closeout

Exit criteria:
- Changelog and evidence are updated.
- Final validation results are recorded.
- Workstream status is complete or remaining work is split explicitly.

# Codex Fearless Refactor Hardening — Handoff

Status: Active
Last updated: 2026-05-28

Current task: CFR-050

## Continuation Notes

- CFR-020 is complete: `crates/core/src/local_log_store.rs` owns bounded rotation, startup/first-write repair, JSONL naming, and rotated-file pruning.
- Runtime and GUI tracing now use `RotatingLogWriter`.
- request/debug/control/retry trace JSONL and relay evidence now use bounded append helpers.
- CFR-030 is complete: high-level CLI, GUI, and admin call sites now use semantic routing authoring methods instead of manual compat sync.
- CFR-040 is complete: `RequestLedgerStore` owns tail, find, finished request projection, and summary reads while compatibility wrapper functions remain.
- Continue with CFR-050 by splitting relay live-smoke diagnostics by case without changing response shape.

## Risks

- Do not delete route compatibility fields until migration tests prove old configs still load.
- Do not change relay diagnostic response shape while splitting modules.
- Preserve existing env vars for request and runtime log limits.

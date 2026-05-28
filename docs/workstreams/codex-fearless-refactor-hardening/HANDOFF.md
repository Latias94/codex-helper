# Codex Fearless Refactor Hardening — Handoff

Status: Active
Last updated: 2026-05-28

Current task: CFR-030

## Continuation Notes

- CFR-020 is complete: `crates/core/src/local_log_store.rs` owns bounded rotation, startup/first-write repair, JSONL naming, and rotated-file pruning.
- Runtime and GUI tracing now use `RotatingLogWriter`.
- request/debug/control/retry trace JSONL and relay evidence now use bounded append helpers.
- Continue with CFR-030 by finding high-level manual graph/compat sync callers and moving them behind a persisted routing document boundary without changing route selection behavior.

## Risks

- Do not delete route compatibility fields until migration tests prove old configs still load.
- Do not change relay diagnostic response shape while splitting modules.
- Preserve existing env vars for request and runtime log limits.

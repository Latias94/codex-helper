# Codex Fearless Refactor Hardening — Handoff

Status: Complete
Last updated: 2026-05-28

> Historical status (superseded 2026-07-14): JSONL files are now bounded post-commit debug output only, while the helper-owned SQLite store is the request-ledger authority. Route compatibility fields, long-lived legacy runtime readers, and wrapper-owned production ledger reads described below were removed by canonical relay/runtime modernization. A separate one-time startup/CLI converter now migrates supported legacy TOML/JSON into canonical version 5 before typed loading.

Current task: none

## Continuation Notes

- CFR-020 is complete: `crates/core/src/local_log_store.rs` owns bounded rotation, startup/first-write repair, JSONL naming, and rotated-file pruning.
- Runtime and GUI tracing now use `RotatingLogWriter`.
- request/debug/control/retry trace JSONL and relay evidence now use bounded append helpers.
- CFR-030 is complete: high-level CLI, GUI, and admin call sites now use semantic routing authoring methods instead of manual compat sync.
- CFR-040 is complete: `RequestLedgerStore` owns tail, find, finished request projection, and summary reads while compatibility wrapper functions remain.
- CFR-050 is complete: live-smoke case descriptors, wire specs, and request bodies now live in `codex_relay_live_smoke/cases.rs`.
- CFR-060 is complete: changelog and evidence are updated.

## Risks

- Do not delete route compatibility fields until migration tests prove old configs still load.
- Do not change relay diagnostic response shape while splitting modules.
- Preserve existing env vars for request and runtime log limits.

# Codex Fearless Refactor Hardening — Evidence And Gates

Status: Complete
Last updated: 2026-05-28

## Required Gates

- `cargo fmt --check`
- `cargo nextest run -p codex-helper runtime_log --no-fail-fast`
- `cargo nextest run -p codex-helper-core logging codex_relay_evidence --no-fail-fast`
- `cargo nextest run -p codex-helper-core config route routing --no-fail-fast`
- `cargo nextest run -p codex-helper-core request_ledger logging --no-fail-fast`
- `cargo nextest run -p codex-helper-core relay_live_smoke codex_live_smoke --no-fail-fast`

## Evidence Log

- 2026-05-28: Workstream opened after architecture scan found the highest-risk follow-up is shared
  bounded local log storage, including `control_trace.jsonl`.
- 2026-05-28: CFR-020 implementation moved local append-only files behind
  `crates/core/src/local_log_store.rs`, covering runtime, GUI, request/debug, control trace, retry
  trace, and relay evidence writes.
- 2026-05-28: `cargo fmt --check` passed.
- 2026-05-28: `cargo nextest run -p codex-helper-core local_log_store codex_relay_evidence logging --no-fail-fast` passed: 24 tests.
- 2026-05-28: `cargo nextest run -p codex-helper --no-fail-fast` passed: 40 tests.
- 2026-05-28: `cargo check -p codex-helper-gui` passed.
- 2026-05-28: CFR-030 added semantic routing authoring helpers on `ServiceViewV4` and
  `RoutingConfigV4`, then replaced high-level CLI, GUI, and admin API compat-sync call sites.
- 2026-05-28: `cargo nextest run -p codex-helper-core config route routing --no-fail-fast` passed: 260 tests.
- 2026-05-28: `cargo nextest run -p codex-helper --no-fail-fast` passed: 40 tests.
- 2026-05-28: `cargo check -p codex-helper-gui` passed.
- 2026-05-28: CFR-040 added `RequestLedgerStore`, routed CLI/TUI/GUI/admin consumers through it, and changed recent/filter reads to bounded streaming windows.
- 2026-05-28: `cargo nextest run -p codex-helper-core request_ledger logging --no-fail-fast` passed: 30 tests.
- 2026-05-28: `cargo nextest run -p codex-helper --no-fail-fast` passed: 40 tests.
- 2026-05-28: `cargo check -p codex-helper-gui` passed.
- 2026-05-28: CFR-050 split live-smoke case descriptors, HTTP specs, and request bodies into `crates/core/src/proxy/codex_relay_live_smoke/cases.rs`.
- 2026-05-28: `cargo nextest run -p codex-helper-core relay_live_smoke codex_live_smoke --no-fail-fast` passed: 22 tests.
- 2026-05-28: `cargo nextest run -p codex-helper --no-fail-fast` passed: 40 tests.
- 2026-05-28: `cargo check -p codex-helper-gui` passed.
- 2026-05-28: `cargo fmt --check` passed.

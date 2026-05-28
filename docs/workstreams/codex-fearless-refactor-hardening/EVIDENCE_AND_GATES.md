# Codex Fearless Refactor Hardening — Evidence And Gates

Status: Active
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

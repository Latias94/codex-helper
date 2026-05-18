# 2026-05-18 — Diagnostics And Static Fallback

## Summary

- Added request-ledger `path` filtering so compact diagnostics do not require raw `rg` against
  JSONL logs.
- Documented `official-relay-bridge` operator flow in English and Chinese configuration docs.
- Rechecked Codex and sub2api references:
  - Codex selects remote compaction from `supports_remote_compaction()`.
  - `name = "OpenAI"` is enough for Codex's OpenAI provider branch.
  - sub2api handles `/responses/compact` in the HTTP Responses path and WebSocket through a separate
    upgrade/forwarder path.
- Decided first release should remain explicit and static. Active compact probing and capability
  hints are deferred.

## Evidence

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core request_ledger`
- `cargo nextest run -p codex-helper-core capabilities`
- `cargo nextest run -p codex-helper-gui request_ledger`
- `cargo check -p codex-helper`
- `cargo check -p codex-helper-tui`
- `cargo check -p codex-helper-gui`
- `cargo nextest run -p codex-helper-core responses_compact`
- targeted official relay compact tests
- `cargo nextest run -p codex-helper-core`
- `cargo run -q --bin codex-helper -- usage find --path responses/compact --limit 20`

## Next

Run closeout review for CORB-050, then decide whether to run `cargo nextest run --workspace` or
record a narrower gate rationale.

# Codex Relay Smoke Evidence CLI — Closeout

Date: 2026-05-19
Status: Complete

## Delivered

- Added `~/.codex-helper/logs/codex_relay_evidence.jsonl` as a separate diagnostic evidence store.
- Added sanitized evidence records for capability diagnostics and live-smoke responses.
- Kept evidence out of request ledger, routing, affinity, health, balance, retry, and patch-mode
  automation.
- Added `codex-helper codex relay-capabilities` for terminal-first validation-only diagnostics.
- Added `codex-helper codex relay-live-smoke` for compact-only or compact + image live smoke using
  the same acknowledgement guard as the API/TUI.
- Added `codex-helper codex relay-evidence` for local evidence inspection with filters and JSON
  output.
- Documented CLI usage, evidence path, and safety boundaries in English and Chinese config docs and
  changelog.

## Gates

- `cargo nextest run -p codex-helper-core codex_relay_evidence`
- `cargo nextest run -p codex-helper codex_relay_cli`
- `cargo nextest run -p codex-helper-core codex_relay_live_smoke`
- `cargo nextest run -p codex-helper-core codex_relay_probe`
- `cargo nextest run -p codex-helper-core codex_live_smoke_api`
- `cargo nextest run -p codex-helper-core`
- `cargo nextest run -p codex-helper`
- `cargo fmt --check`

See `EVIDENCE_AND_GATES.md` for command results.

## Follow-Ons

- Evidence rendering in TUI/GUI.
- WebSocket relay smoke.
- Remote compaction v2 smoke after upstream Codex semantics stabilize.
- Optional manual real-relay evidence collection for specific paid accounts.

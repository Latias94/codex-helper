# Codex Official Relay Bridge — Closeout

Date: 2026-05-18
Status: Complete

> Historical status (superseded 2026-07-12): this closeout records a removed client patch preset. Responses and compact support now come from the selected provider contract and observations; the local switch only updates the helper selector/stanza in Codex `config.toml`.

## Delivered

- Added `official-relay-bridge` as an explicit Codex client patch mode.
- Patched Codex provider output to use `name = "OpenAI"`, `wire_api = "responses"`, and
  `supports_websockets = false`, without requiring OpenAI auth or patching `auth.json`.
- Preserved helper-side upstream credentials and stripped Codex client auth in the new bridge mode.
- Proved `/responses/compact` forwards to upstream `/v1/responses/compact`.
- Added path-based request-ledger diagnostics through CLI and admin API.
- Documented how to diagnose `/responses/compact` versus ordinary `/responses` fallback.
- Documented unsupported relay fallback: switch back to `default` or use a relay account that
  supports compact.

## Gates

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
- `cargo nextest run --workspace`

## Follow-Ons

- WebSocket upgrade forwarding for OpenAI Responses.
- Remote compaction v2 support after Codex and relay semantics stabilize.
- Active compact probing or capability hints if static operator selection is not enough.

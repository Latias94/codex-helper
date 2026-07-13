# Codex Relay Live Smoke Diagnostics — Closeout

Date: 2026-05-19
Status: Complete

> Historical API status (updated 2026-07-12): explicit acknowledged live smoke remains available through process-local CLI/TUI actions, but the remote admin POST endpoint delivered by this lane was removed when the control plane became GET/HEAD-only.

## Delivered

- Added a separate Codex relay live-smoke core path for cost-bearing verification.
- Required `acknowledgement = "run-live-codex-relay-smoke"` before any upstream live-smoke IO.
- Added compact-only smoke for `/responses/compact` and explicit hosted `image_generation` smoke.
- Exposed `POST /__codex_helper/api/v1/codex/relay-live-smoke` through the admin API surface.
- Added TUI Settings double-confirm flows: `X` for compact-only and `Y` for compact plus image.
- Kept live smoke isolated from normal routing, retry, affinity, passive health, balance state, and
  request ledger side effects.
- Documented API examples, TUI usage, side effects, and safety boundaries in English and Chinese
  configuration docs.

## Gates

- `cargo nextest run -p codex-helper-core codex_relay_live_smoke`
- `cargo nextest run -p codex-helper-core codex_live_smoke_api`
- `cargo nextest run -p codex-helper-tui codex_relay_live_smoke`
- `cargo nextest run -p codex-helper-core`
- `cargo nextest run -p codex-helper-tui`
- `cargo fmt --check`

See `EVIDENCE_AND_GATES.md` for command results.

## Follow-Ons

- Real paid relay smoke logs can be collected manually for specific relay accounts, but should stay
  outside automation because hosted image smoke can spend quota or create artifacts.
- WebSocket relay smoke remains out of scope.
- Remote compaction v2 remains diagnostic-only until upstream Codex semantics stabilize.

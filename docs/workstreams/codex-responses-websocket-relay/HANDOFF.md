# Codex Responses WebSocket Relay — Handoff

Status: Active
Last updated: 2026-05-19

## Current objective

Implement a helper-owned Responses WebSocket relay and an explicit opt-in transport switch: `codex.client_patch.responses_websocket = true` / `codex-helper switch on --responses-websocket`.

## Key constraints

- Existing `official-relay` and `official-imagegen` presets must remain HTTP-only by default with `supports_websockets = false`; WebSocket must be explicitly enabled by the separate option.
- Do not make Codex connect directly to `sub2api` as the product path.
- Do not spoof official encrypted compaction payloads.
- WebSocket failover is only safe before an upstream WebSocket connection is established.

## Current state

The first shippable slice is in place:

1. User-facing config/CLI now uses client patch `preset`; legacy `mode` is read-only compatible.
2. `responses_websocket` is an orthogonal switch in config and CLI.
3. Helper owns the Responses WebSocket relay for the three expected GET upgrade routes.
4. Docs and diagnostics explain the new switch and default HTTP-only posture.

## Follow-on work

1. Add usage extraction from Responses WebSocket events if upstream metadata makes it reliable.
2. Add a live smoke test against a real relay target after explicit acknowledgement.
3. Review whether the upstream websocket path should mirror Codex's permessage-deflate/TLS connector
   behavior more closely for certain relays.


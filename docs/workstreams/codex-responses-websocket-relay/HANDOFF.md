# Codex Responses WebSocket Relay — Handoff

Status: Historical (superseded 2026-07-12)
Last updated: 2026-05-19

> Historical status (superseded 2026-07-12): the client-side WebSocket switch and preset compatibility described below were removed. Responses WebSocket support is now a provider/catalog capability, and the relay performs the upstream handshake before downstream upgrade while binding each accepted connection to one captured endpoint/runtime revision.

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
5. `relay-live-smoke --websocket` can validate a selected real upstream's Responses WebSocket v2
   handshake, auth, model mapping, beta header, and first `response.create` frame.
6. Relay diagnostics can target route-graph providers directly with `--provider` / `--endpoint`;
   responses report `provider_endpoint_key` when available.
7. Real smoke results: `input8` accepts Responses WebSocket v2 (`codex.rate_limits` after HTTP
   101), while `ciii` upgrades but closes with code 1011 `upstream websocket proxy failed`.

## Follow-on work

1. Add usage extraction from Responses WebSocket events if upstream metadata makes it reliable.
2. Review whether the upstream websocket path should mirror Codex's permessage-deflate/TLS connector
   behavior more closely for certain relays.
3. If needed, report the ciii WebSocket close code 1011 to the relay operator; its HTTP endpoints
   pass validation probes, so the failure is specific to WebSocket upstream proxying.

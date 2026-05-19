# Codex Responses WebSocket Relay — Milestones

Status: Implemented
Last updated: 2026-05-19

## M0 — Scope And Safety

Done. The design rejects direct-Codex-to-sub2api as the product solution and records the opt-in relay
shape.

## M1 — Patch Mode Surface

Done. Codex TOML patching, status inference, config parsing, diagnostics, and capability profile
logic understand `responses_websocket` as a separate transport switch while existing modes keep
their old default output.

## M2 — Relay Vertical Slice

Done. A local WebSocket client can connect through helper to a local upstream WebSocket server, send
a `response.create` frame, observe mapped/filtered upstream frame and injected auth headers, and
receive an upstream event back.

## M3 — Operator Surface

Done. Configuration docs explain relay requirements, the explicit transport switch, and the
HTTP-only fallback posture when WS support is absent.

## M4 — Closeout

Done. Fresh gates are recorded in `EVIDENCE_AND_GATES.md`; usage parsing/live relay smoke/permessage
deflate parity are separated as follow-ons.


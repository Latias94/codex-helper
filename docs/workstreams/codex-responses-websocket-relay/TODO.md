# Codex Responses WebSocket Relay — TODO

Status: Active
Last updated: 2026-05-19

## M0 — Scope And Safety

- [x] CRW-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-responses-websocket-relay]
  Goal: Freeze the correct product shape: helper-owned WS relay plus an explicit transport switch, not more patch-preset combinations.
  Validation: design, tasks, gates, and handoff docs exist.
  Evidence: `DESIGN.md`, `EVIDENCE_AND_GATES.md`

## M1 — Codex Patch Mode Surface

- [x] CRW-020 [owner=main] [deps=CRW-010] [scope=crates/core/src/codex_integration.rs,crates/core/src/config_storage.rs,crates/core/src/codex_capability_profile.rs]
  Goal: Keep patch presets focused on auth/provider identity and add `responses_websocket` as an orthogonal opt-in switch.
  Validation: targeted codex integration/config/capability tests.
  Evidence: record commands in `EVIDENCE_AND_GATES.md`.

## M2 — Helper WebSocket Relay

- [x] CRW-030 [owner=main] [deps=CRW-020] [scope=crates/core/src/proxy]
  Goal: Implement Responses WebSocket upgrade handling, routing, upstream handshake, auth injection, first-frame model extraction/mapping/filtering, and bidirectional relay.
  Validation: local upstream WebSocket integration test proves header injection, model mapping, and frame relay.
  Evidence: record targeted test command in `EVIDENCE_AND_GATES.md`.

## M3 — Operator Surface

- [x] CRW-040 [owner=main] [deps=CRW-020,CRW-030] [scope=docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,diagnostics as needed]
  Goal: Document when to enable `responses_websocket`, relay-side requirements, and fallback to HTTP-only official presets.
  Validation: docs match implemented flags and diagnostics.
  Evidence: doc diff plus targeted tests if diagnostics change.

## M4 — Verification And Closeout

- [x] CRW-050 [owner=main] [deps=CRW-020,CRW-030,CRW-040] [scope=workspace]
  Goal: Run fresh gates, capture residual risks, and close or split follow-ons.
  Validation: formatting and core tests pass; broader workspace gate if feasible.
  Evidence: `cargo fmt --check`, targeted nextest, `cargo nextest run -p codex-helper-core`, optional workspace gate.


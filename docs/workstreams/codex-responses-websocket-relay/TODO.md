# Codex Responses WebSocket Relay — TODO

Status: Historical (superseded by the canonical relay runtime on 2026-07-13)
Last updated: 2026-05-19

> This completed checklist describes the retired client-patch transport
> switch. Current WebSocket behavior is selected from canonical
> provider/endpoint capabilities and runtime routing. See
> [Configuration](../../CONFIGURATION.md) and the
> [canonical relay runtime modernization plan](../../plans/2026-07-10-002-refactor-canonical-relay-runtime-modernization-plan.md).

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

## M5 — Real Relay Smoke Surface

- [x] CRW-060 [owner=main] [deps=CRW-030,CRW-040] [scope=crates/core/src/proxy/codex_relay_live_smoke.rs,src/cli_types.rs,src/commands/codex.rs,docs/CONFIGURATION*.md]
  Goal: Add an explicit Responses WebSocket live smoke case so operators can verify real relay WebSocket support without enabling it implicitly.
  Validation: local WebSocket upstream test proves `OpenAI-Beta` injection, upstream auth, model mapping, and `response.create` first-frame shape; CLI parses `--websocket`.
  Evidence: record targeted test commands in `EVIDENCE_AND_GATES.md`.

## M6 — Route-Graph Diagnostic Targeting

- [x] CRW-070 [owner=main] [deps=CRW-060] [scope=crates/core/src/proxy/codex_relay_target.rs,src/cli_types.rs,src/commands/codex.rs,docs/CONFIGURATION*.md]
  Goal: Let capability diagnostics and live smoke target route-graph provider ids/endpoints directly, not only legacy station names and upstream indexes.
  Validation: `--provider ciii` (and optional `--endpoint`) resolves the route-graph provider endpoint without changing normal routing; targeted unit tests cover provider, endpoint, and station fallback selection.
  Evidence: record commands in `EVIDENCE_AND_GATES.md`.

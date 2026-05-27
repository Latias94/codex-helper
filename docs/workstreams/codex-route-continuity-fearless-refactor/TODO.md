# Codex Route Continuity Fearless Refactor - TODO

Status: Complete
Last updated: 2026-05-27

## M0 - Scope And Evidence Freeze

- [x] RCF-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-route-continuity-fearless-refactor]
  Goal: Open the workstream for the three approved fearless refactors.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Review: Ensure this lane does not reopen the closed `codex-architecture-deepening` lane.
  Evidence: `docs/workstreams/codex-route-continuity-fearless-refactor/DESIGN.md`
  Handoff: Start with the continuity contract because later selector and test harness work should consume its vocabulary.

## M1 - Deep Continuity Contract

- [x] RCF-020 [owner=main] [deps=RCF-010] [scope=crates/core/src/proxy/request_continuity.rs,crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/responses_websocket.rs]
  Goal: Replace split continuity policy facts with one deep continuity contract consumed by HTTP and Responses WebSocket.
  Validation: `cargo nextest run -p codex-helper-core -E 'test(route_continuity) | test(route_graph_policy) | test(response_semantics_compact) | test(response_semantics_websocket)'`
  Review: Hard and legacy missing-affinity behavior must remain fail-closed; fallback-sticky must remain tryable.
  Evidence: `EVIDENCE_AND_GATES.md` RCF-020 section records passing targeted gate and format check.
  Handoff: DONE. `RequestContinuityContract` now owns missing-affinity, provider-failover, trace, and route-state policy facts consumed by HTTP and Responses WebSocket.

## M2 - Route Target Selection Seam

- [x] RCF-030 [owner=main] [deps=RCF-020] [scope=crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/route_*.rs]
  Goal: Consolidate route graph runtime preparation, affinity application, candidate choice, concurrency filtering, and route unavailable reporting behind a transport-neutral route target selection seam.
  Validation: `cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket) | test(route_unavailable) | test(concurrency)'`
  Review: HTTP attempt execution and WebSocket I/O must remain separate adapters; selection semantics must not drift.
  Evidence: `EVIDENCE_AND_GATES.md` RCF-030 section records passing selection/concurrency gate and format check.
  Handoff: DONE. `route_target_selection` now owns route graph runtime preparation, concurrency snapshots/permits, missing-affinity trace/gate, selection policy, route unavailable failure, and the WebSocket route graph target adapter.

## M3 - Compact Semantics Harness

- [x] RCF-040 [owner=main] [deps=RCF-020] [scope=crates/core/src/proxy/tests/failover]
  Goal: Extract a compact semantics test harness for policy, transport, provider behavior, prior affinity, and expected route outcome.
  Validation: `cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'`
  Review: Keep behavior-specific assertions visible; the harness should remove setup noise, not hide expected policy.
  Evidence: `EVIDENCE_AND_GATES.md` RCF-040 section records passing compact/WebSocket gate.
  Handoff: DONE. `CompactPolicyFixture` now owns repeated two-provider route graph setup, upstream counters, request dispatch, request-log lookup, and continuity trace lookup for the high-churn compact policy tests.

## M4 - Docs And Closeout

- [x] RCF-050 [owner=main] [deps=RCF-030,RCF-040] [scope=docs/workstreams,cargo gates]
  Goal: Update behavior docs, run fresh gates, and close or split residual follow-ons.
  Validation:
  - `cargo fmt --all --check`
  - `cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'`
  - `cargo nextest run -p codex-helper-core`
  Review: Use review-workstream before closeout claims.
  Evidence: `EVIDENCE_AND_GATES.md` RCF-050 section records docs review, targeted hard/domain regression, semantic gate, package gate, and format check.
  Handoff: DONE. No residual follow-on is required for this lane; WebSocket hard explicit-domain selection now matches HTTP selection.

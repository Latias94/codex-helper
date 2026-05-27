# Codex Route Continuity Fearless Refactor - Milestones

Status: Complete
Last updated: 2026-05-27

## M0 - Scope And Evidence Freeze

Exit criteria:

- Workstream docs exist and agree on the three approved refactors.
- Closed architecture workstream is referenced but not reopened.
- First executable task is chosen.

Primary evidence:

- `docs/workstreams/codex-route-continuity-fearless-refactor/DESIGN.md`
- `docs/workstreams/codex-route-continuity-fearless-refactor/TODO.md`

## M1 - Deep Continuity Contract

Exit criteria:

- One continuity contract owns missing-affinity, failover, trace, and route-state policy.
- HTTP and Responses WebSocket consume the same contract.
- Hard, fallback-sticky, legacy, and single-endpoint behavior remains covered.

Primary gate:

- `cargo nextest run -p codex-helper-core -E 'test(route_continuity) | test(route_graph_policy) | test(response_semantics_compact) | test(response_semantics_websocket)'`

## M2 - Route Target Selection Seam

Exit criteria:

- Route graph selection behavior is shared through a clear seam or a documented narrower split.
- HTTP and WebSocket remain separate transport adapters.
- Route unavailable reporting and concurrency filtering do not drift between transports.

Primary gate:

- `cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket) | test(route_unavailable) | test(concurrency)'`

## M3 - Compact Semantics Harness

Exit criteria:

- High-churn compact policy tests use a harness interface.
- Behavior-specific assertions remain explicit.
- The hard/fallback-sticky policy matrix is easy to extend.

Primary gate:

- `cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'`

## M4 - Docs And Closeout

Exit criteria:

- Behavior docs no longer contradict fallback-sticky tryable compact.
- Fresh gates are recorded.
- Remaining work is completed; no follow-on is required for this lane.
- `WORKSTREAM.json` status is updated.

# Codex Route Continuity Fearless Refactor

Status: Complete
Last updated: 2026-05-27

## Why This Lane Exists

The proxy now intentionally treats fallback-sticky remote compaction as tryable instead of locally
rejecting it when route affinity is missing. That fix exposed three shallow modules: continuity
policy is still split across HTTP and WebSocket execution, route target selection is duplicated
between transports, and compact semantics tests repeat the same setup in many places.

This lane deepens those modules without changing the public routing model.

## Relevant Authority

- Existing docs:
  - `docs/workstreams/codex-continuity-decision-refactor/DESIGN.md`
  - `docs/workstreams/codex-remote-compaction-v2-continuity/DESIGN.md`
  - `docs/workstreams/codex-routing-graph-refactor/FEARLESS_REFACTOR.md`
  - `docs/workstreams/codex-routing-runtime-ir-refactor/FEARLESS_REFACTOR.md`
  - `docs/workstreams/codex-architecture-deepening/TODO.md`
- Current behavior baseline:
  - `19e3886 fix(proxy): allow fallback-sticky compact routing without affinity`

## Problem

Route continuity decisions are shallow: callers must know request class, affinity policy, missing
affinity behavior, fallback scope, route-state restriction, and trace fields. HTTP and Responses
WebSocket have separate implementations of related selection and blocking behavior. Compact tests
encode the same provider-policy matrix through repeated setup instead of a clear test seam.

## Target State

- A deep continuity contract module owns continuity classification and execution policy.
- HTTP and Responses WebSocket consume the same continuity contract for missing affinity, fallback
  scope, trace fields, and route-state action.
- Route target selection has one transport-neutral seam with HTTP and WebSocket adapters.
- Compact semantics tests use a small harness interface for policy, transport, provider behavior,
  and expected route outcome.
- Existing fallback-sticky, hard, legacy, single-endpoint, and explicit continuity-domain behavior
  remains covered by fresh tests.

## In Scope

- Refactor `request_continuity`, `provider_execution`, and `responses_websocket`.
- Add or migrate test helpers under `crates/core/src/proxy/tests`.
- Update this workstream evidence and any docs that still claim fallback-sticky missing affinity
  must fail closed.
- Preserve the public config shape and request log compatibility.

## Out Of Scope

- New routing strategies or public config syntax.
- New relay-specific heuristics for shared encrypted state.
- Changing OpenAI or Codex upstream protocol semantics.
- Rewriting the full route graph compiler.
- Replacing the legacy execution path beyond adapter compatibility required by this lane.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Fallback-sticky compact should try a provider when no route affinity exists. | High | User direction and commit `19e3886`. | The continuity contract would encode the wrong fallback scope. |
| Hard compact and legacy multi-upstream compact should remain fail-closed on missing affinity. | High | Existing hard and legacy regression tests. | Refactor could reintroduce unsafe cross-provider compact. |
| HTTP and Responses WebSocket can share selection policy without sharing transport I/O. | Medium | Both already compile a `RoutePlanTemplate` and build `AttemptTarget`. | The selector seam may need a smaller first slice around policy only. |
| Compact test setup is ready for a harness. | High | `response_semantics_compact.rs` is over 3000 lines with repeated provider setup. | Harness extraction should be selective, not blanket migration. |

## Architecture Direction

Deepen `request_continuity` into the owner of a continuity contract. The contract should expose
small methods or values for:

- continuity class and reason,
- whether known affinity is required,
- whether provider failover is allowed,
- whether route state should be restricted to the affinity continuity domain,
- missing-affinity failure status/message,
- trace payload fields.

`provider_execution` and `responses_websocket` should stop re-deriving those facts. They should
adapt transport-specific inputs into the contract, then consume the contract.

After the contract is stable, route target selection can move behind a transport-neutral seam:
runtime preparation, affinity application, candidate choice, concurrency filtering, and route
unavailable reports live in one module; HTTP and WebSocket remain adapters for different I/O.

Tests should then expose the compact behavior matrix directly:

```text
transport x affinity_policy x prior_affinity x provider_result -> expected route outcome
```

## Closeout Condition

This lane can close when:

- all three target refactors are implemented,
- HTTP and WebSocket consume one continuity contract,
- route target selection behavior is shared through a clear seam or a documented narrower split,
- compact semantics tests use a harness for high-churn policy cases,
- evidence gates pass,
- docs reflect the shipped fallback-sticky and hard behavior,
- and follow-on work is either split or explicitly deferred.

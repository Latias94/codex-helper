# Codex Continuity Decision Refactor

Status: Complete
Last updated: 2026-05-26

Update, 2026-05-27: later fallback-sticky work relaxed the missing-affinity bootstrap rule for
route-graph compact requests. `fallback-sticky` compact can now try the configured route without
prior affinity and record the successful endpoint; `hard` compact and legacy multi-upstream compact
retain fail-closed missing-affinity behavior.

## Why This Lane Exists

Codex helper now supports ordinary Responses HTTP, remote compaction v1, remote compaction v2, and Responses WebSocket relay. These paths all need route continuity decisions, but the decision is currently spread across request body parsing, request flavor flags, provider execution, route affinity, and WebSocket target selection.

The result is a shallow interface: callers must know whether a request is compact, state-bound, WebSocket, route-graph, legacy, single-endpoint, multi-endpoint, or session-affinity-driven. This lane deepens that surface into one continuity decision module.

## Relevant Authority

- Related workstreams:
  - `docs/workstreams/codex-session-route-continuity/`
  - `docs/workstreams/codex-remote-compaction-v2-continuity/`
  - `docs/workstreams/codex-responses-websocket-relay/`
  - `docs/workstreams/codex-routing-graph-refactor/`
  - `docs/workstreams/codex-routing-preference-runtime-refactor/`
- Reference repos:
  - `repo-ref/codex`: upstream Codex sends remote_compaction_v2 over WebSocket as `response.create` with `compaction_trigger`.
  - `repo-ref/sub2api`: v0.1.131 added OpenAI upstream transport profile, Responses `response.failed` stream termination, and broader WebSocket tool-output classification.

## Problem

State-bound Codex requests can be over-pinned, under-pinned, or pinned in different ways depending on transport. This can produce false 502s when an ordinary conversation turn is hard-stuck to an unhealthy endpoint even though another endpoint is healthy, and can also let WebSocket compact requests bypass HTTP compact continuity policy.

## Target State

- HTTP and Responses WebSocket derive continuity from one module.
- Ordinary conversation turns use soft session affinity: prefer the last successful endpoint, but escape when the endpoint is unavailable.
- Compact and encrypted-state requests use policy-sensitive continuity:
  `fallback-sticky` may bootstrap through the route graph, while `hard` stays
  within a proven continuity domain and fails closed otherwise.
- Missing affinity bootstrap is explicit policy behavior, not an accidental
  preference-group fallback.
- `continuity_domain` is explicit operator configuration. Domain-name equality is a diagnostic hint, not sufficient proof for relay state sharing.
- Official OpenAI direct endpoints may later gain a conservative canonical domain heuristic based on official profile, canonical base URL, credential source, and org/project identity. Relay endpoints do not get this heuristic.
- Route failure reporting distinguishes route unavailable, hard affinity unavailable, missing state-bound affinity, invalid state, and upstream transport/status failures.

## In Scope

- A deep `ContinuityDecision` module for HTTP bodies and WebSocket first frames.
- Shared compact v1/v2 and `compaction_trigger` classification across transports.
- Session soft-affinity vs state-bound hard-affinity policy in route graph and legacy execution.
- Initial `continuity_domain` identity and conservative config rules.
- Tests proving false 502 risk is reduced for ordinary turns while compact safety remains fail-closed.
- Operator-facing docs and diagnostics that explain when affinity blocked fallback.

## Out Of Scope

- Spoofing, decrypting, or synthesizing `encrypted_content`.
- Inferring relay implementation from hostname, UI label, balance endpoint, or model list.
- Automatically treating two sub2api/new-api relay URLs as the same continuity domain.
- Rewriting Codex upstream remote compaction protocol.
- Solving every WebSocket replay/previous-response recovery behavior from sub2api.
- Desktop release changes.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| WebSocket remote_compaction_v2 is a `response.create` frame containing `compaction_trigger`. | High | `repo-ref/codex/codex-rs/core/tests/suite/client_websockets.rs:243` | WebSocket compact would not need shared classification, but the current code still benefits from a shared continuity module. |
| Provider endpoint identity is the safest default continuity domain. | High | Prior route continuity workstreams and observed encrypted-state failures across endpoints. | We may fail closed more often than necessary until explicit domains are configured. |
| Domain-name equality alone is not proof of shared relay state. | High | Local proxy often fronts third-party relays; identical domains can hide different keys/accounts, and different domains can front the same relay account. | Automatic domain matching could corrupt compact state or produce invalid previous response chains. |
| Official OpenAI direct endpoints can eventually support a conservative canonical-domain heuristic. | Medium | Official API state is account/project-backed, but helper config may still vary credentials and org/project headers. | We should ship explicit domains first, then add official-only heuristics behind tests and diagnostics. |
| Ordinary conversation turns should not be hard-pinned. | High | Hard pinning can return false 502 while other healthy endpoints exist; state-free turns can safely retry elsewhere. | Some relays may prefer session stickiness for cache locality, but that is performance, not correctness. |

## Architecture Direction

Add a deep module under `crates/core/src/proxy/` that owns continuity classification and execution contract generation.

The module should expose a small interface:

```text
ContinuityDecision {
  class: Stateless | SessionPreferred | ProviderStateBound
  affinity: None | PreferExisting | RequireKnown | BootstrapIfSingleDomain
  fallback: AnyHealthy | SameContinuityDomain | NoFallback
  reason: CompactV1 | CompactV2 | EncryptedState | PreviousResponseId | OrdinaryTurn | WebSocketFrame
}
```

HTTP request handling and WebSocket preparation feed raw request semantics into this module. Provider execution and WebSocket target selection consume the resulting contract instead of re-deriving compact or affinity behavior.

`continuity_domain` should be modeled separately from provider endpoint:

- default: `ProviderEndpointKey`
- explicit: operator-provided stable domain id
- future official-only heuristic: canonical official OpenAI base URL plus helper-side credential identity and org/project identity

Relay endpoints remain provider-opaque. Domain matching can warn, suggest, or prefill docs, but it must not silently relax state-bound fallback.

## Closeout Condition

This lane can close when:

- HTTP and WebSocket compact requests use the same continuity decision contract,
- ordinary conversation turns can escape unhealthy soft affinity,
- state-bound compact remains fail-closed outside one proven domain,
- tests cover single-domain bootstrap, multi-domain missing affinity, known hard affinity, soft affinity escape, and WebSocket compact,
- diagnostics explain affinity-related route failure without masking upstream health,
- docs reflect official OpenAI direct and relay behavior,
- and follow-on work is either split or explicitly deferred.

# Codex Session Route Continuity

Status: Draft
Last updated: 2026-05-25

Update, 2026-05-27: this lane records the earlier provider-opaque baseline. The current shipped
route-graph behavior is policy-sensitive: `fallback-sticky` may bootstrap missing affinity by
trying the configured route and recording the successful endpoint, while `hard` and legacy
multi-upstream paths remain fail-closed on missing state-bound compact affinity.

## Why This Lane Exists

Codex remote compaction can carry provider-bound state such as encrypted
reasoning content. The proxy currently records session route affinity as
runtime-only state, so restarting the helper can lose the provider endpoint
that a Codex session must continue using. When affinity is missing, route
selection can silently choose a different provider endpoint under normal
preference-group routing.

## Relevant Authority

- Related workstreams:
  - `docs/workstreams/codex-protocol-normalization-affinity`
  - `docs/workstreams/codex-request-response-semantics`
  - `docs/workstreams/codex-routing-preference-runtime-refactor`
- Source anchors:
  - `crates/core/src/state.rs`
  - `crates/core/src/proxy/provider_execution.rs`
  - `crates/core/src/proxy/request_body.rs`
  - `crates/core/src/proxy/route_affinity.rs`

## Problem

Session route affinity is modeled as ephemeral proxy state even though
state-bound Codex requests require a durable route continuity contract. The
proxy also mixes compact request semantics, provider failover policy, and
runtime health into boolean decisions that are hard to diagnose.

## Target State

- Session route affinity needed for Codex continuity survives helper restarts.
- Provider endpoints remain opaque; the proxy does not assume whether a relay
  is OpenAI, sub2api, new-api, or another intermediary.
- State-bound compact requests either use the known provider endpoint, bootstrap
  a new endpoint when the active policy explicitly allows it, or fail with an
  explicit continuity error instead of silently selecting a new one.
- Compact requests whose affinity provider endpoint fails have explicit
  fallback semantics based on continuity class: non-state-bound compact may
  fallback like a normal session request, while state-bound compact follows the
  active affinity policy and may require an explicitly configured safe
  continuity domain.
- Route logs explain the continuity class, affinity source, and failover
  decision.
- Balance probes and runtime health signals are kept separate.

## In Scope

- Introduce a durable session route ledger for provider-endpoint affinity.
- Restore route affinity from disk during proxy startup.
- Classify remote compact requests by continuity requirement.
- Improve logs for affinity source and state-bound failover blocking.
- Add targeted tests for restart recovery and missing-affinity behavior.

## Out Of Scope

- Inferring relay internals from provider names, base URLs, or balance adapters.
- Changing upstream relays such as sub2api or new-api.
- Allowing state-bound compact fallback across different provider endpoints by
  default.
- Treating all compact failures as terminal when the request is not
  state-bound.
- Broad routing preference redesign beyond the continuity cases in this lane.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Provider endpoint identity is the safest default continuity unit. | High | Existing route graph work re-keyed runtime state by provider endpoint identity. | A later explicit continuity-domain feature may be needed. |
| State-bound compact can break when sent to a different provider endpoint. | High | Requests can contain `encrypted_content` and prior response state. | Failing closed may be too conservative for some relays, requiring an opt-in override. |
| The proxy cannot reliably identify relay implementation type. | High | User providers may point at OpenAI, sub2api, new-api, or opaque relays. | Any implementation-specific policy would be brittle. |
| Balance status is not runtime compact availability. | High | Logs showed `exhausted = false` while compact returned 429. | Runtime health must remain separate from balance probes. |

## Architecture Direction

Deepen the session route affinity module into a durable session route ledger.
Callers should ask the ledger for a continuity decision instead of reading a
runtime hash map and composing compact/failover booleans locally.

The ledger owns persistence, pruning, restore validation, and affinity source
attribution. Provider execution remains responsible for selecting and executing
provider endpoints, but it consumes a continuity decision with explicit
semantics:

- use a known provider endpoint,
- allow provider failover,
- or fail closed because the request is state-bound and no durable affinity is
  available.

When the known provider endpoint fails, fallback is not a yes/no compact rule.
It is a continuity decision:

- Stateless or session-preferred requests may continue through the existing
  provider failover path.
- Provider-state-bound requests follow the active affinity policy: `fallback-sticky`
  can continue through the configured route graph and update affinity, while
  `hard` stays on the affinity endpoint unless an operator-configured continuity
  domain proves that multiple endpoints share the same upstream state.
- Missing affinity for provider-state-bound requests should either be an
  explicit policy bootstrap or an explicit continuity error, not an accidental
  preference-group re-entry.

Provider internals stay opaque. Relay-specific adapters may contribute balance
or observed health data, but they must not become routing truth unless the
operator explicitly configures a future continuity domain.

## Closeout Condition

This lane can close when:

- restart-safe session route continuity is implemented,
- state-bound compact missing-affinity behavior is explicit and tested,
- logs expose the continuity decision clearly,
- targeted and package gates pass,
- and any continuity-domain or relay-capability follow-up is split or deferred.

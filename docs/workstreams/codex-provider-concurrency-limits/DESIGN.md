# Codex Provider Concurrency Limits

## Problem

codex-helper can route by provider order, tags, health, cooldown, balance exhaustion, and session affinity, but it cannot protect a relay that only allows a small number of concurrent requests.

When multiple Codex sessions share one local proxy, session affinity can concentrate traffic on the same relay. If that relay only supports five concurrent requests, the sixth request should not blindly enter that upstream and cause avoidable queueing, 429s, or degraded latency.

## Target State

- Providers and endpoints can declare local concurrency limits.
- The route executor treats a saturated provider/endpoint as temporarily unavailable for selection.
- Saturation does not count as upstream failure, does not open cooldown, and does not poison session affinity.
- A selected request holds a permit until the upstream attempt is finished. Streaming attempts hold the permit until the stream response is finalized.
- Route attempts and operator views can explain that a candidate was skipped because its concurrency limit was saturated.

## Scope

- v5 route graph provider/endpoint config.
- Runtime route selection and selected-upstream execution.
- Request/route observability needed to explain saturation.
- Focused unit or integration tests around selection and permit behavior.

## Non-Goals

- Distributed limits across multiple codex-helper processes.
- Token-per-minute, request-per-minute, or adaptive rate limiting.
- Persistent queueing as the default behavior.
- Reworking the existing retry/failover contract.

## Architecture Direction

Add a reusable `ProviderConcurrencyLimits` config block that can live on a provider or endpoint. Endpoint values override provider defaults. The compiled route candidate carries the effective limit metadata.

Runtime enforcement should be independent from `active_requests`. Active request tracking is an observability ledger, while concurrency gating must be an atomic runtime resource. The proxy should maintain a keyed semaphore registry by `service + limit group`. The default limit group should be the provider endpoint key; an explicit `limit_group` allows several endpoints/providers to share one account-level cap.

Route selection should see current saturation before choosing a candidate. For a saturated candidate, selection records `concurrency_saturated` and moves to the next route candidate under normal failover semantics.

The actual upstream attempt must acquire a permit immediately before sending to the selected upstream and release it after the attempt is done. This protects against races where several requests all observe capacity and then enter the same upstream.

## Open Assumptions

- The first implementation defaults overflow to failover, not queue.
- Limits are local-process limits only.
- `max_concurrent_requests = 0` is invalid; missing limit means unlimited.

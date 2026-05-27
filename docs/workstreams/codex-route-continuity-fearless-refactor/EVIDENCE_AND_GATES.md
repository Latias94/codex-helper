# Codex Route Continuity Fearless Refactor - Evidence And Gates

Status: Complete
Last updated: 2026-05-27

## Smallest Current Repro

The original regression was local 503 for fallback-sticky remote compaction without existing route
affinity. The baseline fix is commit `19e3886`.

```bash
cargo nextest run -p codex-helper-core -E 'test(proxy_allows_remote_compaction_v2_without_route_affinity_under_fallback_sticky) | test(responses_websocket_allows_fallback_sticky_compaction_without_route_affinity)'
```

## Gate Set

### RCF-020 Continuity Contract Gate

```bash
cargo nextest run -p codex-helper-core -E 'test(route_continuity) | test(route_graph_policy) | test(response_semantics_compact) | test(response_semantics_websocket)'
```

Proves HTTP/WebSocket continuity policy and compact semantics still agree after contract extraction.

### RCF-030 Route Target Selection Gate

```bash
cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket) | test(route_unavailable) | test(concurrency)'
```

Proves shared route selection preserves compact, WebSocket, unavailable-route, and concurrency paths.

### RCF-040 Compact Harness Gate

```bash
cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'
```

Proves the migrated harness still covers the compact policy matrix.

### Package Gate

```bash
cargo nextest run -p codex-helper-core
```

Required before closeout because the refactors touch shared proxy routing modules.

### Format Gate

```bash
cargo fmt --all --check
```

Required before closeout and before any requested commit.

### Review Gate

Run `review-workstream` before accepting task or lane completion. Record blocking findings, missing
gates, and residual risks here or link to the review note.

## Evidence Anchors

- `docs/workstreams/codex-route-continuity-fearless-refactor/DESIGN.md`
- `docs/workstreams/codex-route-continuity-fearless-refactor/TODO.md`
- `docs/workstreams/codex-route-continuity-fearless-refactor/MILESTONES.md`
- `crates/core/src/proxy/request_continuity.rs`
- `crates/core/src/proxy/provider_execution.rs`
- `crates/core/src/proxy/responses_websocket.rs`
- `crates/core/src/proxy/tests/failover/response_semantics_compact.rs`
- `crates/core/src/proxy/tests/failover/response_semantics_websocket.rs`

## Running Evidence

Record fresh command output here as tasks land.

### 2026-05-27 - RCF-020 Continuity Contract

Claim: `RequestContinuityContract` owns the continuity policy facts needed by HTTP provider
execution and Responses WebSocket selection without changing compact routing behavior.

Commands:

```bash
cargo nextest run -p codex-helper-core -E 'test(route_continuity) | test(route_graph_policy) | test(response_semantics_compact) | test(response_semantics_websocket)'
cargo fmt --all --check
```

Result:

- `cargo nextest`: pass, 37 tests run, 37 passed, 689 skipped.
- `cargo fmt --all --check`: pass.

Behavior proven:

- fallback-sticky compact without existing route affinity remains tryable for HTTP and WebSocket,
- hard and legacy missing-affinity compact remain fail-closed,
- explicit continuity-domain compact failover remains covered,
- route graph policy and continuity contract unit coverage passed.

Broader gates not run:

- `cargo nextest run -p codex-helper-core` deferred until RCF-030/RCF-040 because RCF-020 changed
  only continuity policy plumbing and the targeted semantic gate covers the touched behavior.

### 2026-05-27 - RCF-030 Route Target Selection Seam

Claim: route graph runtime preparation, concurrency filtering, missing-affinity gate/trace, candidate
selection policy, route unavailable failure, and the WebSocket route graph target adapter live behind
`route_target_selection` without changing HTTP/WebSocket route behavior.

Commands:

```bash
cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket) | test(route_unavailable) | test(concurrency)'
cargo fmt --all --check
```

Result:

- `cargo nextest`: pass, 42 tests run, 42 passed, 684 skipped.
- `cargo fmt --all --check`: pass.

Behavior proven:

- compact route graph semantics remain intact,
- Responses WebSocket route graph selection still succeeds/fails with expected request logs,
- route unavailable reports still produce expected retryable failures,
- local provider concurrency saturation still avoids saturated candidates.

Broader gates not run:

- `cargo nextest run -p codex-helper-core` deferred until RCF-040 and closeout because RCF-030 is a
  routing seam extraction with targeted semantic coverage.

### 2026-05-27 - RCF-040 Compact Semantics Harness

Claim: high-churn compact policy tests share a compact semantics harness without hiding the
behavior-specific assertions for fallback-sticky tryability, hard missing-affinity fail-closed
behavior, request-log compatibility, route affinity recording, and continuity trace payloads.

Commands:

```bash
cargo nextest run -p codex-helper-core -E 'test(proxy_allows_state_bound_responses_compact_without_route_affinity_under_fallback_sticky) | test(proxy_allows_remote_compaction_v2_without_route_affinity_under_fallback_sticky) | test(proxy_rejects_remote_compaction_v2_without_route_affinity_under_hard_policy)'
cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'
```

Result:

- focused harness migration check: pass, 3 tests run, 3 passed, 723 skipped.
- RCF-040 semantic gate: pass, 32 tests run, 32 passed, 694 skipped.

Behavior proven:

- fallback-sticky state-bound `/responses/compact` without route affinity still tries provider `b`,
- fallback-sticky remote compaction v2 without route affinity still tries provider `b` and records
  `remote_compaction_v2_request`,
- hard-policy remote compaction v2 without route affinity still returns 503 without upstream hits
  and emits `route_continuity_blocked`,
- Responses WebSocket compact semantics still pass after the shared test harness landed.

Broader gates not run:

- `cargo nextest run -p codex-helper-core` deferred to RCF-050 closeout because RCF-040 changed
  only test harness structure and the required compact/WebSocket semantic gate passed.

### 2026-05-27 - RCF-050 Docs, Review, And Closeout

Claim: the three approved refactors are integrated, behavior docs match shipped
fallback-sticky/hard semantics, and the shared route target selector preserves HTTP and Responses
WebSocket continuity behavior.

Commands:

```bash
cargo fmt --all --check
git diff --check
cargo nextest run -p codex-helper-core -E 'test(responses_websocket_hard_compaction_fallback_stays_inside_explicit_continuity_domain) | test(proxy_allows_state_bound_compact_failover_with_explicit_continuity_domain) | test(proxy_does_not_infer_continuity_domain_from_same_base_url_for_hard_state_bound_compact)'
cargo nextest run -p codex-helper-core -E 'test(response_semantics_compact) | test(response_semantics_websocket)'
cargo nextest run -p codex-helper-core
```

Result:

- `cargo fmt --all --check`: pass.
- `git diff --check`: pass.
- hard/domain regression check: pass, 3 tests run, 3 passed, 724 skipped.
- compact/WebSocket semantic gate: pass, 33 tests run, 33 passed, 694 skipped.
- package gate: pass, 727 tests run, 727 passed, 0 skipped.

Review findings:

- Blocking issue found during review: WebSocket route-graph selection used the shared selector but
  did not restrict hard state-bound selection to the affinity continuity domain before selecting.
  Fixed by applying the same continuity-domain restriction used by HTTP and adding
  `responses_websocket_hard_compaction_fallback_stays_inside_explicit_continuity_domain`.
- No remaining blocking workstream-compliance findings.
- No remaining missing gates.

Behavior proven:

- fallback-sticky compact without existing route affinity remains tryable for HTTP and Responses
  WebSocket,
- hard and legacy missing-affinity compact remain fail-closed,
- hard state-bound compact failover is constrained to an explicit shared continuity domain for both
  HTTP and Responses WebSocket,
- same base URL / host still does not imply a shared continuity domain,
- package-level proxy, routing, logging, and config tests all pass after the refactor.

Residual risks:

- The new shared selector intentionally changes hard explicit-domain WebSocket fallback from
  under-covered behavior to match HTTP. No additional follow-on is required for this lane.
- Transport-level WebSocket reconnect/failover after a successful upstream handshake remains out of
  scope; the selector gate covers pre-connection route target choice.

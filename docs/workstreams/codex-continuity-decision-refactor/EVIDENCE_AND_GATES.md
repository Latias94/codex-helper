# Codex Continuity Decision Refactor - Evidence And Gates

Status: Complete
Last updated: 2026-05-26

## Targeted Gates

Run as tasks land:

```powershell
cargo nextest run -p codex-helper-core remote_compaction_v2 responses_websocket route_affinity --no-fail-fast
cargo nextest run -p codex-helper-core session_route_affinity route_unavailable --no-fail-fast
cargo nextest run -p codex-helper-core continuity_domain route_affinity --no-fail-fast
cargo nextest run -p codex-helper-core capabilities codex_capability_profile --no-fail-fast
cargo fmt --all --check
```

## Broad Gates

Before closeout:

```powershell
cargo nextest run -p codex-helper-core
cargo check -p codex-helper
```

## Evidence Log

| Date | Task | Command / Evidence | Result | Notes |
| --- | --- | --- | --- | --- |
| 2026-05-26 | CDC-010 | Workstream docs created | Pending review | Planning artifact only; no code gates run yet. |
| 2026-05-26 | CDC-020 | `cargo nextest run -p codex-helper-core remote_compaction_v2 responses_websocket route_affinity --no-fail-fast` | Passed: 29 tests | Proves existing HTTP compact/route-affinity behavior remains green after moving classification into `request_continuity` and adding WebSocket first-frame classification/logging. |
| 2026-05-26 | CDC-020 | `cargo fmt --all --check` | Passed | Formatting gate after implementation. |
| 2026-05-26 | CDC-020 | `cargo nextest run -p codex-helper-core request_continuity request_flavor_finalizes_remote_compaction_v2_from_body responses_websocket_relays_headers_model_mapping_and_frames route_graph_policy_treats_remote_compaction_v2_as_state_bound --no-fail-fast` | Passed: 7 tests | Fresh verification for the CDC-020 completion claim. |
| 2026-05-26 | CDC-030 | `cargo nextest run -p codex-helper-core responses_websocket_rejects_compaction_trigger_without_route_affinity responses_websocket_allows_compaction_trigger_without_prior_affinity_for_single_endpoint --no-fail-fast` | Passed: 2 tests | Proves WebSocket v2 compact fail-closed and single-endpoint bootstrap behavior. |
| 2026-05-26 | CDC-030 | `cargo nextest run -p codex-helper-core responses_websocket remote_compaction_v2 --no-fail-fast` | Passed: 24 tests | Task-level gate for WebSocket compact continuity and existing remote_compaction_v2 behavior. |
| 2026-05-26 | CDC-030 | `cargo fmt --all --check` | Passed | Formatting gate after CDC-030 implementation. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable --no-fail-fast` | Failed: 1 test | Red phase confirmed ordinary `Hard` route affinity returned false 502 when the pinned endpoint was unavailable and another endpoint was healthy. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable --no-fail-fast` | Passed: 1 test | Green phase after continuity-aware soft affinity selector in provider execution. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core route_plan_executor_soft_affinity_escapes_unavailable_hard_affinity --no-fail-fast` | Passed: 1 test | Unit coverage for the soft selector while preserving configured `Hard` selector behavior. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core session_route_affinity route_unavailable --no-fail-fast` | Passed: 6 tests | Task-level gate for session affinity and route-unavailable behavior. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core request_continuity proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable route_plan_executor_soft_affinity_escapes_unavailable_hard_affinity responses_websocket_rejects_compaction_trigger_without_route_affinity responses_websocket_allows_compaction_trigger_without_prior_affinity_for_single_endpoint proxy_rejects_remote_compaction_v2_without_route_affinity proxy_pins_remote_compaction_v2_responses_to_route_affinity --no-fail-fast` | Passed: 10 tests | Regression set proving ordinary soft escape did not weaken state-bound HTTP/WebSocket compact handling. |
| 2026-05-26 | CDC-040 | `cargo fmt --all --check` | Passed | Formatting gate after CDC-040 implementation. |
| 2026-05-26 | CDC-040 | `cargo nextest run -p codex-helper-core session_route_affinity route_unavailable proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable route_plan_executor_soft_affinity_escapes_unavailable_hard_affinity --no-fail-fast` | Passed: 8 tests | Fresh final CDC-040 verification after formatting. |
| 2026-05-26 | CDC-050 | `cargo nextest run -p codex-helper-core proxy_does_not_infer_continuity_domain_from_same_base_url_for_state_bound_compact proxy_allows_state_bound_compact_failover_with_explicit_continuity_domain --no-fail-fast` | Passed: 2 tests | Proves same base URL/domain is not automatic state-sharing proof and explicit `continuity_domain` permits state-bound fallback within the proven domain. |
| 2026-05-26 | CDC-050 | `cargo nextest run -p codex-helper-core continuity_domain route_affinity proxy_does_not_infer_continuity_domain_from_same_base_url_for_state_bound_compact proxy_allows_state_bound_compact_failover_with_explicit_continuity_domain routing_ir_continuity_domain_defaults_to_endpoint_and_supports_explicit_overrides migration_plan_replaces_provider_endpoint_state_when_continuity_domain_changes --no-fail-fast` | Passed: 17 tests | Task-level CDC-050 gate for explicit domain defaults/inheritance, migration reset, and route-affinity behavior. |
| 2026-05-26 | CDC-050 | `cargo fmt --all --check` | Passed | Formatting gate after CDC-050 implementation. |
| 2026-05-26 | CDC-050 | `cargo nextest run -p codex-helper-core request_continuity remote_compaction_v2 responses_websocket route_affinity session_route_affinity route_unavailable continuity_domain --no-fail-fast` | Passed: 43 tests | Broader continuity/compact regression set after CDC-040 and CDC-050. |
| 2026-05-26 | CDC-060 | `cargo check -p codex-helper-core` | Passed | Compile gate after adding capability/profile continuity diagnostics. |
| 2026-05-26 | CDC-060 | `cargo fmt --all --check` | Passed | Formatting gate after CDC-060 implementation. |
| 2026-05-26 | CDC-060 | `cargo nextest run -p codex-helper-core capabilities codex_capability_profile --no-fail-fast` | Passed: 36 tests | Proves capability/profile output includes conservative OpenAI identity wording and relay diagnostics expose explicit continuity-domain recommendations without same-domain auto-inference. |
| 2026-05-26 | CDC-070 | `cargo nextest run -p codex-helper-core request_continuity remote_compaction_v2 responses_websocket route_affinity session_route_affinity route_unavailable continuity_domain capabilities codex_capability_profile --no-fail-fast` | Passed: 78 tests | Final targeted regression set for shared continuity, HTTP/WS compact, route affinity, explicit domains, and diagnostics. |
| 2026-05-26 | CDC-070 | `cargo fmt --all --check` | Passed | Final formatting gate. |
| 2026-05-26 | CDC-070 | `cargo nextest run -p codex-helper-core --no-fail-fast` | Passed: 721 tests | Full core regression gate. |
| 2026-05-26 | CDC-070 | `cargo check -p codex-helper` | Passed | Binary compile gate. |
| 2026-05-26 | CDC-070 | `git diff --check` | Passed | Whitespace check; Git reported only line-ending normalization warnings. |

## Required Regression Evidence

- Ordinary turn with pinned unhealthy endpoint and another healthy endpoint does not return false 502. Covered by `proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable`.
- HTTP v2 compact with one configured domain can bootstrap affinity.
- HTTP v2 compact with multiple domains and no affinity fails closed.
- WebSocket v2 compact with `compaction_trigger` follows the same rules as HTTP.
- State-bound request with known affinity does not cross domains unless explicit `continuity_domain` proves it safe. Covered by `proxy_does_not_infer_continuity_domain_from_same_base_url_for_state_bound_compact` and `proxy_allows_state_bound_compact_failover_with_explicit_continuity_domain`.
- Diagnostics distinguish affinity-blocked fallback from upstream service failure. Capability diagnostics now report selected continuity domain, explicit-domain status, and continuity-domain recommendations.

## Gates Not Run Yet

None for this lane.

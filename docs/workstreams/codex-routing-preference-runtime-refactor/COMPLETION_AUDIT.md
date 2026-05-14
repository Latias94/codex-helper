# Completion Audit: Codex Routing Preference Runtime

## Audit Status

Status: complete.

This audit is intentionally strict. It maps the objective to current artifacts
and records the compatibility boundaries that remain after closing the
workstream.

## Objective Restated

The refactor is complete only when Codex Helper routing is a first-class route
graph runtime:

- request selection is modeled as route graph -> route plan -> preference group
  -> provider endpoint candidate -> attempt;
- user preference outranks automatic affinity by default;
- monthly-first routes return to preferred providers after fallback recovery;
- route graph execution does not depend on synthetic `routing` stations,
  compatibility `LoadBalancer` selection, station pins, or `last_good_index`;
- v4 and older station-shaped configs migrate deterministically to v5;
- runtime health, cooldown, usage exhaustion, and affinity are keyed by provider
  endpoint identity;
- explain output, request logs, CLI/TUI/GUI surfaces are endpoint/provider first;
- legacy station identity is limited to migration input, historical log reading,
  explicit compatibility projection, and legacy station execution paths;
- tests prove fallback recovery, explicit pins, retry behavior, cooldown/health,
  usage exhaustion, and migration behavior.

## Prompt-To-Artifact Checklist

| Requirement | Evidence | Coverage | Status |
| --- | --- | --- | --- |
| Route graph is first-class selection input, not a folded station list. | `RequestRouteSelection` is an enum with `Legacy { lbs }` and `RouteGraph { template }` in `crates/core/src/proxy/request_routing.rs`; `v4_route_selection_for_request` returns `RouteGraph { template }` after `compile_v4_route_plan_template_with_request`. | Separates route graph request selection from legacy station selection. | Pass |
| Route graph execution uses provider endpoint runtime state. | `execute_provider_chain_with_route_executor` matches `RequestRouteSelection::RouteGraph`, reads `route_plan_runtime_state_for_provider_endpoints`, and builds `AttemptTarget::from_candidate`. Legacy branch separately compiles `compile_legacy_route_plan_template`. | Covers the new executor entry boundary. | Pass |
| Route graph attempts do not mutate a synthetic `routing` `LbState`. | Route graph calls `execute_selected_upstream` with `legacy_lb: None`; test `proxy_v4_route_graph_health_does_not_write_synthetic_routing_lb_state` covers the behavior. | Covers health/cooldown writes for route graph attempts. | Pass |
| User preference outranks automatic affinity by default. | `RoutingAffinityPolicyV5::PreferredGroup` is the default; `best_candidate_by_affinity_policy` only applies affinity inside the best available preference group for that mode. | Covered by routing IR tests and failover integration. | Pass |
| Monthly-first fallback returns to preferred providers after recovery. | Test `proxy_v4_route_graph_affinity_is_session_scoped` sends a fallback request to `right`, then verifies the same session returns to `input` and records provider-endpoint affinity. | Covers the original user-visible bug. | Pass |
| Explicit old fallback-sticky behavior remains opt-in. | `RoutingAffinityPolicyV5::FallbackSticky` exists; docs describe explicit opt-in; tests cover fallback-sticky retention, `fallback_ttl_ms`, and `reprobe_preferred_after_ms`. | Covers compatibility mode and bounded reuse. | Pass |
| Retry semantics remain predictable. | `proxy_v4_route_graph_affinity_is_session_scoped` verifies `input -> input1 -> right`, provider/upstream max attempts, and `avoided_candidate_indices`; routing IR tests cover unsupported model and all-unsupported exhaustion. | Good coverage for the new route graph loop; broader legacy retry coverage still exists. | Pass |
| Route graph retry avoidance is candidate-index based. | Route graph selection returns `avoided_candidate_indices`; route graph attempts leave `avoid_for_station` empty in the failover integration test. | Covers removal of station-scoped avoid sets from route graph execution. | Pass |
| Runtime state is keyed by provider endpoint identity. | `RoutePlanRuntimeState` stores provider endpoint keys; route graph execution reads `route_plan_runtime_state_for_provider_endpoints`; routing explain test `proxy_api_v1_routing_explain_uses_provider_endpoint_runtime_health_for_v4_routes` proves `usage_exhausted` comes from provider endpoint state. | Covers selection/explain runtime state. | Pass |
| Session affinity identity is provider endpoint based. | `route_affinity.rs` applies session affinity through provider endpoint keys and records success only for provider endpoint attempt targets. | Covers route graph affinity state. | Pass |
| Station pins are replaced by route/provider/endpoint target overrides for route graph configs. | API tests in `persisted_crud.rs` reject station overrides, accept `backup.default` and `paygo.default`, and prove session route target overrides win over global route targets and persisted manual routing. | Covers persisted API behavior. | Pass |
| v4 -> v5 migration is deterministic and preserves endpoint identity. | Test `old_v4_route_graph_auto_migrates_to_v5_and_preserves_endpoint_identity` verifies version bump, backup creation, endpoint order, and endpoint tags. | Covers route graph schema migration. | Pass |
| station-shaped config migrates to route graph without station profile binding. | Test `station_shaped_v2_config_migrates_to_route_graph_without_profile_station_binding` covers v2 station/group migration, warnings, cleared profile station binding, and provider endpoint identity. | Covers legacy station-shaped migration. | Pass |
| Explain output is provider endpoint first and compatibility is optional. | `RoutingExplainCandidate` has `provider_endpoint_key`, `provider_id`, `endpoint_id`, `route_path`, `preference_group`, and optional `compatibility`; route graph explain test asserts no synthesized compatibility object for v4 route graph candidates. | Covers JSON API contract. | Pass |
| Control trace records skipped higher-priority groups. | `route_graph_selection_explain` event records selected provider endpoint, selected preference group, skipped groups, and skipped higher-priority candidates; `control_trace` tests parse the detail. | Covers diagnostic trace. | Pass |
| Request ledger display is endpoint/provider first. | `format_request_log_record_lines` renders `endpoint={} provider={} station={}`; test `display_lines_include_route_model_fast_cache_and_speed` verifies endpoint and missing station display. | Covers CLI/log reader display. | Pass |
| CLI/TUI/GUI route views are endpoint/provider first. | CLI route explain formatting prints `endpoint=... group=... provider=...`; TUI/GUI routing preview formatters use selected `provider_endpoint_key` and optional compatibility. Route graph controls and no-session prompts now say route target; legacy station pages keep station terminology only for legacy station configs and historical station views. | Covers the public route graph control and explain surfaces. | Pass |
| Documentation explains monthly-first troubleshooting and old behavior opt-in. | `docs/CONFIGURATION.md` documents `preferred-group`, `fallback-sticky`, `routing explain`, control trace, and monthly-first recipes. | Troubleshooting guidance added in this audit pass. | Pass |
| Legacy station is removed from all new route graph APIs. | Station write APIs are rejected for route graph configs; route target APIs exist. Request-ledger filters/summaries, station health pages, and station-oriented API names remain classified as legacy station config, historical log, compatibility projection, or legacy executor surfaces. | New route graph control and explain APIs do not use station identity as canonical routing identity. | Pass |
| Workstream has accepted defaults and test evidence. | This document provides the evidence map; `TODO.md` and `MILESTONES.md` are closed; verification commands passed after the final documentation and wording edits. | Completion evidence is current. | Pass |

## Verified Commands

The following commands were run after the final audit/documentation/wording
pass:

```bash
cargo fmt --all --check
cargo check --workspace --all-features
cargo nextest run -p codex-helper-core --test-threads 1 --no-fail-fast
cargo nextest run -p codex-helper-tui --test-threads 1 --no-fail-fast
cargo nextest run -p codex-helper-gui --test-threads 1 --no-fail-fast
```

Results:

- core nextest: 410 passed, 0 skipped;
- TUI nextest: 69 passed, 0 skipped;
- GUI nextest: 135 passed, 0 skipped.

Targeted evidence includes:

- `proxy_v4_route_graph_affinity_is_session_scoped`
- `proxy_v4_route_graph_health_does_not_write_synthetic_routing_lb_state`
- `proxy::tests::api_admin::routing_explain`
- `request_ledger::tests::display_lines_include_route_model_fast_cache_and_speed`
- GUI `overview_runtime_status_running` tests

This audit does not treat those commands as sufficient by themselves. They are
evidence only for the requirements listed above.

## Compatibility Boundaries

- Station remains a supported term for legacy station configs, station health
  pages, legacy station execution, migration diagnostics, and historical log
  reading.
- Route graph control surfaces use route target, provider, endpoint, preference
  group, and provider endpoint identity.
- Optional `compatibility` fields may report legacy station/upstream context
  when it exists, but route graph candidates without explicit legacy metadata
  omit that compatibility object.
- Request-ledger station filters remain compatibility filters over historical
  records and legacy station requests; endpoint/provider fields are the primary
  route graph identity.

## Accepted Decisions

- Default route graph affinity is `preferred-group`.
- `fallback-sticky` remains available only as an explicit compatibility mode.
- Trusted balance exhaustion is a provider-endpoint runtime signal for the
  current request/refresh window. It should not become a session-level permanent
  skip; later balance refreshes can make the preferred provider viable again.
- Route graph configs reject legacy station override writes and use
  route/provider/endpoint target overrides.
- Historical logs may still contain station/upstream fields and must remain
  readable.

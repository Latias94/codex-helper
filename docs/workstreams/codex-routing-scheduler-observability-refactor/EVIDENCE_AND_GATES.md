# Evidence And Gates

## Planned Gates

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core scheduler_observability`
- `cargo nextest run -p codex-helper-core upstream_throttle`
- `cargo nextest run -p codex-helper-core concurrency`
- `cargo nextest run -p codex-helper-tui session_metrics`
- GUI/admin snapshot tests for provider active/limit and saturation rendering,
  when those surfaces are changed.

## Evidence Log

- 2026-06-15: design proposed. No runtime code changed in this workstream yet.
- 2026-06-15: `cargo fmt --package codex-helper-core --package codex-helper-tui --package codex-helper-gui` passed after adding session output token-per-second fields.
- 2026-06-15: `cargo check --package codex-helper-core --package codex-helper-tui --package codex-helper-gui` passed.
- 2026-06-15: `cargo nextest run --package codex-helper-core -E 'test(session_cards_expose_last_and_average_output_token_speed) | test(finished_request_observability_derives_canonical_request_facts) | test(finished_request_serializes_materialized_observability_for_operator_api) | test(finished_request_legacy_payload_still_derives_observability)'` passed: 4 tests run, 4 passed.
- 2026-06-15: `cargo nextest run --package codex-helper-tui -E 'test(session) | test(runtime_skip_reasons_include_concurrency_counts)'` passed: 15 tests run, 15 passed.
- 2026-06-15: `cargo nextest run --package codex-helper-gui -E 'test(session) | test(history_observed) | test(attached_refresh)'` passed: 40 tests run, 40 passed.
- 2026-06-15: `cargo fmt --package codex-helper-core --package codex-helper-tui --package codex-helper-gui` passed after adding provider capacity surfaces.
- 2026-06-15: `cargo check --package codex-helper-core --package codex-helper-tui --package codex-helper-gui` passed after adding provider capacity surfaces.
- 2026-06-15: `cargo nextest run --package codex-helper-core -E 'test(proxy_v4_route_graph_skips_provider_when_local_concurrency_limit_is_saturated) | test(proxy_api_v1_provider_runtime_override_filters_v4_route_plan_routing) | test(proxy_api_v1_operator_summary_reports_runtime_target_and_retry)'` passed: 3 tests run, 3 passed.
- 2026-06-15: `cargo nextest run --package codex-helper-tui -E 'test(runtime_skip_reasons_include_concurrency_counts) | test(runtime_candidate_includes_capacity_surface) | test(station_routing_preview_sorts_multi_level_and_active_tiebreak)'` passed: 3 tests run, 3 passed.
- 2026-06-15: `cargo nextest run --package codex-helper-gui -E 'test(runtime_skip_reasons_include_concurrency_counts) | test(runtime_candidate_includes_capacity_surface) | test(format_provider_capacity_reports_active_limit_group_and_saturation)'` passed: 3 tests run, 3 passed.
- 2026-06-15: `git diff --check` passed with only Git line-ending warnings for touched Rust files.
- 2026-06-15: `cargo fmt --package codex-helper-core` passed after upstream throttle outcome integration.
- 2026-06-15: `cargo check --package codex-helper-core` passed.
- 2026-06-15: `cargo nextest run --package codex-helper-core -E 'test(classifies_rate_limited_429_with_retry_after) | test(classifies_overloaded_capacity_messages_even_on_400) | test(classifies_529_as_overloaded_without_body_keywords) | test(classifies_sub2api_flat_error_string_rate_limit) | test(classifies_quota_and_billing_errors_as_rate_limited) | test(classifies_concurrency_and_pending_queue_as_overloaded) | test(classifies_google_rpc_retry_info_and_resource_exhausted) | test(retry_after_secs_supports_retry_after_ms_and_resets_in_seconds) | test(retry_after_secs_supports_http_date_header) | test(does_not_classify_success_body_as_throttle)'` passed: 10 tests run, 10 passed.
- 2026-06-15: `cargo nextest run --package codex-helper-core -E 'test(proxy_capacity_body_400_fails_over_by_overloaded_class) | test(proxy_429_usage_limit_body_sets_retry_after_cooldown) | test(proxy_codex_stream_usage_exhausted_route_returns_retryable_response_failed_sse) | test(proxy_codex_stream_mixed_upstream_failure_and_cooldown_reports_route_unavailable_sse)'` passed: 4 tests run, 4 passed.

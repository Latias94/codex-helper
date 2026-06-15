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

# Evidence And Gates

## Planned Gates

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core <targeted tests>` when nextest is available.
- Fallback: `cargo test -p codex-helper-core <targeted tests>`.

## Evidence Log

- 2026-05-17: `cargo test -p codex-helper-core proxy_v4_route_graph_skips_provider_when_local_concurrency_limit_is_saturated -- --nocapture` passed. Proves a held primary provider permit makes a second real proxy request fail over to backup and `routing explain` reports `concurrency_saturated`.
- 2026-05-17: `cargo test -p codex-helper-core concurrency -- --nocapture` passed. Covered provider limit compile, endpoint override, default unlimited behavior, zero-limit rejection, limiter release, limit-change active-count preservation, and proxy saturation failover.
- 2026-05-17: `cargo test -p codex-helper-core route_plan_executor_skips_saturated_candidate_without_failure_penalty -- --nocapture` passed. Proves saturated candidates are skipped without failure penalty semantics.
- 2026-05-17: `cargo fmt --check` passed after formatting.
- 2026-05-17: `cargo nextest run -p codex-helper-core concurrency` passed: 7 tests run, 7 passed.
- 2026-05-17: `cargo nextest run -p codex-helper-core route_plan_executor_skips_saturated_candidate_without_failure_penalty` passed: 1 test run, 1 passed.
- 2026-05-17: `cargo test -p codex-helper-core proxy_api_v1_v4_persisted_control_plane_edits_v4_document -- --nocapture` passed. Covers persisted v4 provider spec `limits` readback, old-client preservation, explicit update, explicit clear, and zero-limit rejection.
- 2026-05-17: `cargo test -p codex-helper-core proxy_api_v1_provider_specs_crud_persists_endpoints_and_env_refs -- --nocapture` passed. Confirms v2 provider spec CRUD still persists through v4 migration after adding default catalog limits, and true v2 provider writes reject unsupported `limits` instead of silently dropping them.
- 2026-05-17: `cargo test -p codex-helper-gui runtime_skip_reasons_include_concurrency_counts -- --nocapture` passed. Covers GUI runtime preview formatting for `concurrency_saturated(active=N/limit=M)`.
- 2026-05-17: `cargo test -p codex-helper-tui runtime_skip_reasons_include_concurrency_counts -- --nocapture` passed. Covers TUI runtime preview formatting for `concurrency_saturated(active=N/limit=M)`.
- 2026-05-17: `cargo nextest run -p codex-helper-core proxy_api_v1_v4_persisted_control_plane_edits_v4_document` passed: 1 test run, 1 passed.
- 2026-05-17: `cargo nextest run -p codex-helper-core proxy_api_v1_provider_specs_crud_persists_endpoints_and_env_refs` passed: 1 test run, 1 passed.
- 2026-05-17: reran `cargo fmt --check`, `cargo nextest run -p codex-helper-core concurrency`, and `cargo nextest run -p codex-helper-core route_plan_executor_skips_saturated_candidate_without_failure_penalty`; all passed.
- 2026-05-17: one parallel `cargo test -p codex-helper-tui runtime_skip_reasons_include_concurrency_counts -- --nocapture` attempt failed during compilation with a rustc out-of-memory error while other cargo jobs were running. The same test passed when rerun serially.

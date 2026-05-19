# Evidence and Gates

## Commands

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core <targeted tests>`
- `cargo nextest run --workspace` if the change surface stays broad after implementation.

## Evidence Log

- `cargo check --workspace`
  - Result: passed.
- `cargo fmt --check`
  - Result: passed.
- `cargo nextest run -p codex-helper-core codex_bridge detect_request_flavor_marks_codex_bridge_compact_request request_log_serializes_codex_bridge_metadata --no-fail-fast`
  - Result: 6 passed.
- `cargo nextest run -p codex-helper-core codex_switch official_imagegen official_relay response_semantics request_log --no-fail-fast`
  - Result: 46 passed.
- `cargo nextest run --workspace`
  - Result: one failover test failed once in the full concurrent run, then passed when rerun alone.
- `cargo nextest run -p codex-helper-core proxy_same_upstream_retries_502_then_succeeds_without_failover --no-fail-fast`
  - Result: 1 passed.
- `cargo nextest run --workspace --no-fail-fast`
  - Result: 760 passed.

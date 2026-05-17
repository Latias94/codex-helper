# Task Ledger

## PCL-010 Config And IR Contract

- Status: completed
- Owner: main
- Scope: `crates/core/src/config.rs`, `crates/core/src/routing_ir.rs`, config tests.
- Goal: add persisted concurrency limit fields and compile them into route candidates.
- Validation: targeted config/routing tests prove provider defaults, endpoint overrides, explicit groups, and default unlimited behavior.
- Handoff: `ProviderConcurrencyLimits` is persisted on provider/endpoint config; endpoint values override provider values; missing limit remains unlimited; `max_concurrent_requests = 0` is rejected.

## PCL-020 Selection Saturation Semantics

- Status: completed
- Owner: main
- Scope: route runtime state and route executor selection tests.
- Goal: represent current in-flight counts and skip saturated candidates with `concurrency_saturated`.
- Validation: route executor selects fallback when primary is saturated and does not treat saturation as failure/cooldown/exhaustion.
- Handoff: route runtime state carries `concurrency_saturated`, `concurrency_active`, and `concurrency_limit`; skip reason is `concurrency_saturated`.

## PCL-030 Execution Permit Enforcement

- Status: completed
- Owner: main
- Scope: `crates/core/src/proxy/*`, runtime service state.
- Goal: acquire/release permits around selected upstream attempts, including streaming responses.
- Validation: concurrent proxy test proves configured limit is not exceeded and fallback is used when available.
- Handoff: v5 route graph execution acquires a local permit before selected upstream transport and releases it after buffered completion or SSE stream finalization.

## PCL-040 Observability And Operator Surface

- Status: completed
- Owner: main
- Scope: route attempt logs, route explain/admin snapshots, docs.
- Goal: expose configured limit, active count, and saturation skip reason where operators diagnose routing.
- Validation: tests or snapshots cover route attempt/explain output for a saturated candidate.
- Handoff: execution emits `route_candidate_concurrency_saturated`; routing explain reports `concurrency_saturated` with active and limit. Full TUI/GUI table surfacing remains a follow-on if desired.

## PCL-045 Persisted Control Plane And Preview Polish

- Status: completed
- Owner: main
- Scope: persisted provider spec API, v2/v4 catalog adapters, GUI/TUI routing preview.
- Goal: expose provider/endpoint `limits` through the persisted provider spec surface and make saturation skips easier to read in operator previews.
- Validation: provider spec CRUD tests cover v4 catalog readback, old-client preservation, explicit update, explicit clear, and zero-limit rejection; GUI/TUI unit tests cover `concurrency_saturated(active=N/limit=M)` rendering.
- Handoff: omitted `limits` preserves existing advanced fields; `{}` explicitly clears limits; endpoint-level default limits prevent unsafe inlining only when needed.

## PCL-050 Gates And Closeout

- Status: completed
- Owner: main
- Scope: validation commands and workstream docs.
- Goal: run focused `cargo fmt` and `cargo nextest`/`cargo test` gates, record evidence, and prepare commit proposal.
- Validation: evidence recorded in `EVIDENCE_AND_GATES.md`.

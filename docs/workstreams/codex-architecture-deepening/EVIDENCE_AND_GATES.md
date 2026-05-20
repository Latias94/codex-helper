# Codex Architecture Deepening — Evidence And Gates

Status: Complete
Last updated: 2026-05-20

## Gate Set

### Baseline / closeout

```powershell
cargo fmt --check
cargo nextest run -p codex-helper-core
```

### Session identity

```powershell
cargo nextest run -p codex-helper-core prompt_cache_key_affinity --no-fail-fast
```

### Shared request preparation

```powershell
cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity responses_websocket --no-fail-fast
```

### Relay diagnostic registry

```powershell
cargo nextest run -p codex-helper-core relay_capabilities relay_live_smoke codex_live_smoke --no-fail-fast
```

### Proxy test harness

```powershell
cargo nextest run -p codex-helper-core proxy::tests::failover::response_semantics --no-fail-fast
```

### Codex patch plan

```powershell
cargo nextest run -p codex-helper-core codex_switch codex_bridge codex_client_patch --no-fail-fast
```

## Fresh Evidence

### 2026-05-20 — CAD-010 Scope Freeze

Claim: this should be a new workstream rather than extending `codex-protocol-normalization-affinity`.

Evidence:

- The previous lane is complete and focused on protocol normalization/affinity behavior.
- This lane is broader architecture work across state identity, request preparation, relay diagnostics, proxy tests, and Codex patching.
- Each slice has different gates and can be reviewed independently, but they share the same objective: make Codex relay compatibility Modules deeper and more maintainable.

Commands: source/workstream inspection only.

### 2026-05-20 — CAD-020 Session Identity Semantics

Claim: header-derived sessions and `prompt_cache_key` fallback sessions now share the same routing key behavior while exposing source metadata in request logs, in-memory request state, session identity cards, and route affinity records.

Implementation notes:

- Added `SessionIdentitySource::{Header, PromptCacheKey}` and threaded optional `session_identity_source` through active/finished requests, request-log/debug-log entries, session stats/cards, and `SessionRouteAffinity`.
- Replaced HTTP and Responses WebSocket session extraction with `ClientSessionIdentity { value, source }`; header identity still wins over body fallback.
- Kept compatibility by preserving `session_id` as the routing key and making source fields optional/skip-serializing.

Observable examples:

- Official header path: `session_id = "sid-official"`, `session_identity_source = "header"` on finished requests, session cards, and route affinity.
- Fallback path: `session_id = "pcache-affinity"`, `session_identity_source = "prompt_cache_key"` after zstd request-body normalization and compact affinity reuse.

Commands:

```powershell
cargo fmt --check
# Result: PASS

cargo nextest run -p codex-helper-core request_log_serializes_request_id_when_present request_log_serializes_codex_bridge_metadata extract_session_id_uses_prompt_cache_key_body_fallback extract_session_id_prefers_headers_over_prompt_cache_key build_session_identity_cards_merges_sources_and_sorts_newest_first --no-fail-fast
# Result: PASS — 5 passed, 584 skipped

cargo nextest run -p codex-helper-core prompt_cache_key_affinity --no-fail-fast
# Result: PASS — 1 passed, 588 skipped

cargo nextest run -p codex-helper-core response_semantics --no-fail-fast
# Result: PASS — 20 passed, 569 skipped
```

Broader package gate not run for CAD-020 because this slice is covered by targeted identity, logging, and response-semantics gates; package-level `cargo nextest run -p codex-helper-core` remains reserved for CAD-070 closeout unless a later slice demands it.

### 2026-05-20 — CAD-030 Shared Codex Request Preparation

Claim: HTTP request preparation and Responses WebSocket first-frame preparation now share the deeper request-preparation workflow instead of duplicating session/body/route setup.

Before call graph:

- HTTP `prepare_proxy_request`: config reload -> detect flavor -> read/decode body -> session identity -> binding/overrides -> body rewrite -> begin request -> route selection -> retry/preview setup.
- WebSocket `prepare_responses_websocket`: config reload -> validate first frame -> session identity -> binding/overrides -> body rewrite -> begin request -> route selection -> retry setup.

After call graph:

- HTTP adapter keeps request body reading, content-encoding normalization, request-flavor detection, and HTTP-specific error logging.
- WebSocket adapter keeps first `response.create` frame validation and WebSocket handshake metadata.
- Shared `request_preparation::prepare_common_request` owns session identity extraction, session binding/touch, manual override/default-profile body rewrite, `begin_request`, route selection, retry plan, cooldown backoff, and body preview setup.

Compatibility notes:

- HTTP body read/decode errors still log using header-only identity when body fallback is unavailable.
- Responses WebSocket still rejects missing/non-`response.create` first data messages before common preparation.
- Auth injection and selected-upstream model mapping remain in transport/selected-upstream adapters.

Commands:

```powershell
cargo fmt --check
# Result: PASS

cargo nextest run -p codex-helper-core prepare_common_request_tracks_prompt_cache_identity_and_overrides_body --no-fail-fast
# Result: PASS — 1 passed, 589 skipped

cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity responses_websocket --no-fail-fast
# Result: PASS — 18 passed, 572 skipped

cargo nextest run -p codex-helper-core response_semantics --no-fail-fast
# Result: PASS — 20 passed, 570 skipped
```

Broader package gate not run for CAD-030 because the targeted request-encoding, affinity, WebSocket, and response-semantics gates exercise the changed shared preparation paths. Package-level `cargo nextest run -p codex-helper-core` remains reserved for CAD-070 closeout.

### 2026-05-20 — CAD-040 Relay Diagnostic Case Registry

Claim: relay capability diagnostics and live smoke execution now use explicit case registries instead of hard-coded compact/image/websocket branching in the orchestration path.

Implementation notes:

- Added `CodexRelayProbeCase` registry for `model_catalog`, `responses`, and `remote_compaction_v1`; `CodexRelayProbeSpec::for_kind` now derives its wire contract from that registry.
- `codex_relay_capabilities_for_proxy` runs registered probe cases and maps observations back into the existing `observed.models`, `observed.responses`, and `observed.responses_compact` response shape.
- Added `CodexRelayLiveSmokeCaseDescriptor` registry for compact, hosted image generation, and Responses WebSocket live-smoke cases.
- HTTP live-smoke descriptors now own method/path/stream/timeout/body/classifier; WebSocket descriptors own path, beta header, handshake/read timeouts, and body builder.
- Cost-bearing behavior remains guarded by the existing acknowledgement token; default live-smoke cases still include only `responses_compact`, and image/WebSocket smoke remain explicit-only warnings.
- Evidence semantics are unchanged because capability/live-smoke responses keep the same serialized payload fields consumed by `codex_relay_evidence`.

Registry characterization tests:

- `codex_relay_probe_registry_defines_existing_wire_contracts`
- `codex_relay_capabilities_observed_shape_is_built_from_probe_registry`
- `codex_relay_live_smoke_registry_preserves_default_and_explicit_cases`
- `codex_relay_live_smoke_http_registry_preserves_wire_specs`

Commands:

```powershell
cargo fmt --check
# Result: PASS

cargo nextest run -p codex-helper-core relay_capabilities relay_live_smoke codex_live_smoke --no-fail-fast
# Result: PASS — 19 passed, 575 skipped

cargo nextest run -p codex-helper-core codex_relay_probe_registry_defines_existing_wire_contracts codex_relay_capabilities_observed_shape_is_built_from_probe_registry codex_relay_live_smoke_registry_preserves_default_and_explicit_cases codex_relay_live_smoke_http_registry_preserves_wire_specs --no-fail-fast
# Result: PASS — 4 passed, 590 skipped

cargo nextest run -p codex-helper-core codex_relay_probe --no-fail-fast
# Result: PASS — 11 passed, 583 skipped
```

Broader package gate not run for CAD-040 because the changed relay diagnostics are covered by the relay capability/live-smoke API gates plus registry/probe characterization tests. Package-level `cargo nextest run -p codex-helper-core` remains reserved for CAD-070 closeout.

### 2026-05-20 — CAD-050 Proxy Integration Test Harness

Claim: the highest-churn response semantics tests now have a small reusable integration-test harness that removes duplicated proxy/upstream setup while keeping behavior-specific assertions explicit.

Implementation notes:

- Added `crates/core/src/proxy/tests/harness.rs`.
- Introduced RAII-style `TestProxyServer` and `TestUpstreamServer` wrappers over `spawn_axum_server`; dropped handles automatically abort spawned servers.
- Added focused helpers:
  - `spawn_test_upstream` for upstream servers;
  - `spawn_test_proxy` and `spawn_proxy_service` for proxy router lifecycle;
  - default `upstream_config`;
  - `post_responses_json` and `post_compact_json`;
  - `find_finished_request` for polling request-state evidence.
- Migrated the first response-semantics slice: compact path forwarding, request content-encoding normalization/passthrough/rejection, compact unsupported diagnostics, non-retryable 400/404/client-error failover checks, model support skipping, and model mapping.
- Assertions stayed close to each test: hit counts, response status/body, content encoding, upstream body bytes, model mapping, and request-state evidence remain visible in test bodies.

Commands:

```powershell
cargo fmt --check
# Result: PASS

cargo check -p codex-helper-core --tests
# Result: PASS

cargo nextest run -p codex-helper-core proxy::tests::failover::response_semantics --no-fail-fast
# Result: PASS — 20 passed, 574 skipped
```

Broader package gate not run for CAD-050 because the migrated harness is exercised by the full `response_semantics` module gate. Package-level `cargo nextest run -p codex-helper-core` remains reserved for CAD-070 closeout.

### 2026-05-20 — CAD-060 Codex Patch Plan Seam

Claim: Codex client patching now has a pure `CodexPatchPlan` policy seam, while TOML/auth/switch-state writes are performed by explicit execution adapters.

Implementation notes:

- Added `crates/core/src/codex_patch_plan.rs` as the policy module for:
  - `CodexPatchMode` and `CodexSwitchOptions`;
  - provider identity (`codex-helper` vs `OpenAI`);
  - TOML bool patch decisions for `requires_openai_auth` and `supports_websockets`;
  - auth patch strategy (`restore original`, ChatGPT bridge, imagegen facade);
  - switch-on side-effect order (`config -> auth -> state` vs `state -> config -> auth`);
  - bridge runtime readiness requirement.
- `codex_integration.rs` now re-exports the public mode/options types and delegates switch-on calculation to `CodexPatchPlan`.
- TOML mutation is isolated in `switch_on_codex_toml_with_plan`.
- Auth side effects are isolated in `auth_edit_for_switch_on_plan` and reuse a pure-ish current-text baseline helper so already-patched facade states preserve the original auth baseline.
- Switch-state/config/auth write ordering is isolated in `apply_switch_on_effects`.
- `codex_capability_profile.rs` derives provider/auth capability shape from `CodexPatchPlan`, and relay capability expected-profile building now passes the patch config through the plan seam.
- Existing config aliases and presets still parse through the same config-storage tests; no `/responses/compact` fallback or relay feature synthesis was added.

Characterization tests added:

- `codex_patch_plan_chatgpt_bridge_keeps_account_auth_shape_and_safe_write_order`
- `codex_patch_plan_official_relay_uses_openai_identity_without_auth_facade`
- `codex_patch_plan_official_imagegen_combines_openai_identity_and_auth_facade`
- `codex_patch_plan_rejects_websocket_transport_without_official_identity`
- `codex_switch_on_toml_is_driven_by_patch_plan`
- `codex_auth_edit_for_switch_on_plan_restores_prior_facade_from_current_text`

Commands:

```powershell
cargo fmt --check
# Result: PASS

cargo check -p codex-helper-core --tests
# Result: PASS

cargo nextest run -p codex-helper-core codex_patch_plan --no-fail-fast
# Result: PASS — 4 passed, 596 skipped

cargo nextest run -p codex-helper-core codex_switch codex_bridge codex_client_patch --no-fail-fast
# Result: PASS — 51 passed, 549 skipped
```

Note: the final two nextest commands were launched in parallel; both printed temporary Cargo file-lock waits before compiling/running and then passed. Package-level `cargo nextest run -p codex-helper-core` remains reserved for CAD-070 closeout.

### 2026-05-20 — CAD-070 Verification And Closeout

Claim: all five architecture-deepening slices are complete, behavior-preserving gates pass, and the workstream can close without splitting a required follow-on.

Closeout diagnosis note:

- First full-package run failed in `state::tests::finish_request_estimates_cost_and_rolls_up_cost`.
- Cause: CAD-020 added `session_identity_source` to `ProxyState::begin_request`; several state tests had inserted the new `None` at the wrong position, shifting `model` into `cwd` and making pricing unknown.
- Fix: realigned state test call sites so `model` remains the `model` argument, then reran the narrowed repro and full package gate.

Commands:

```powershell
cargo nextest run -p codex-helper-core state::tests::finish_request_estimates_cost_and_rolls_up_cost begin_and_finish_requests_keep_trace_id usage_rollup_view_scores_entities_inside_selected_window --no-fail-fast
# Result: PASS — 3 passed, 597 skipped

cargo nextest run -p codex-helper-core
# First result: FAIL — 547 passed, 1 failed, 52 not run due fail-fast
# Final result after fix: PASS — 600 passed, 0 skipped

cargo fmt --check
# Result: PASS
```

Closeout decision:

- Required lane scope is complete.
- No required follow-on split.
- Optional future cleanup: migrate more proxy integration tests to `proxy/tests/harness.rs` selectively, and consider typed builders for long `begin_request` test call sites to avoid positional-argument drift.
- `cargo clippy -p codex-helper-core --all-targets -- -D warnings` was not run because this lane did not add unsafe code or materially change public trait/unsafe surfaces; the full package nextest gate plus targeted gates cover the changed behavior.

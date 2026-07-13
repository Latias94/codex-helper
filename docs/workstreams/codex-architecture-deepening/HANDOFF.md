# Codex Architecture Deepening — Handoff

Status: Complete
Last updated: 2026-05-20

> Historical status (superseded 2026-07-12): this handoff records an intermediate architecture. The client patch-plan seam, preset/auth-facade behavior, remote-control integration, and compatibility readers described below were removed by canonical relay/runtime modernization. The current local switch only updates the helper selector/stanza in Codex `config.toml`; provider capability comes from the captured provider contract.

## Current State

Goal is ready to mark complete. CAD-020, CAD-030, CAD-040, CAD-050, CAD-060, and CAD-070 are implemented and verified. The prior protocol-normalization workstream is complete and committed.

CAD-020 added optional `session_identity_source` metadata for:

- request logs and split debug logs;
- active/finished request state and replay from request logs;
- session stats and session identity cards;
- route affinity records;
- HTTP request preparation and Responses WebSocket first-frame preparation.

Compatibility note: `session_id` values and routing keys are unchanged. New source fields are optional and omitted for legacy/unknown rows.

CAD-030 extracted `request_preparation::prepare_common_request` for HTTP and Responses WebSocket. Transport adapters now retain only transport-specific concerns:

- HTTP: request body read limits, content-encoding normalization, request-flavor detection, and early body error logging.
- Responses WebSocket: first-frame `response.create` validation and WebSocket handshake details.
- Shared: session identity, bindings/overrides, body rewrite, begin-request, route selection, retry plan, cooldown backoff, and preview setup.

CAD-040 introduced relay diagnostic registries:

- capability probes are registered as `CodexRelayProbeCase` entries for `model_catalog`, `responses`, and `remote_compaction_v1`;
- live-smoke cases are registered as `CodexRelayLiveSmokeCaseDescriptor` entries for compact, hosted image generation, and Responses WebSocket;
- compact remains the only default live-smoke case, while image/WebSocket remain explicit-only with the same warnings;
- all live-smoke cases still require the existing acknowledgement token before upstream I/O.

CAD-050 added the first proxy integration test harness:

- `crates/core/src/proxy/tests/harness.rs` owns proxy/upstream test server lifecycle wrappers, default upstream config, JSON request helpers, and finished-request polling.
- The first migrated response-semantics slice reduced repeated proxy/upstream setup while leaving status/body/hit-count/model/affinity assertions in test bodies.
- Do not mass-migrate every test by default; extend the harness only where it makes tests read in domain terms.

CAD-060 added the Codex patch plan seam:

- `crates/core/src/codex_patch_plan.rs` owns pure patch-policy calculation for mode/options, provider identity, provider TOML bool patches, auth patch strategy, runtime readiness requirement, and switch-on effect ordering.
- `codex_integration.rs` re-exports the public mode/options types, applies TOML with `switch_on_codex_toml_with_plan`, resolves auth edits with `auth_edit_for_switch_on_plan`, and performs writes with `apply_switch_on_effects`.
- `codex_capability_profile.rs` and relay capability expected-profile construction now derive provider/auth capability shape from the same patch-plan policy seam.
- Behavior preserved: preset aliases, auth facade restoration safety, readiness diagnostics, WebSocket option validation, and remote-control code remain stable. No compact fallback was added.

## Closeout Verification

Final gates:

```powershell
cargo fmt --check
# PASS

cargo nextest run -p codex-helper-core
# PASS — 600 passed, 0 skipped
```

During closeout, the first package run exposed a state-test positional argument regression from adding `session_identity_source` to `begin_request`; it was fixed and the full package gate passed after the fix.

## Next Action

Ask the user whether to commit the completed lane. Do not commit without explicit confirmation.

## Important Constraints

- Do not implement `/responses/compact` fallback.
- Do not add relay features while refactoring unless a task explicitly scopes them.
- Preserve current public behavior and existing config compatibility.
- Keep tasks vertical and independently verified.
- Ask before committing.

## Useful Targeted Validation

```powershell
cargo nextest run -p codex-helper-core codex_switch codex_bridge codex_client_patch --no-fail-fast
```

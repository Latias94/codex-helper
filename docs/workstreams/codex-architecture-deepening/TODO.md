# Codex Architecture Deepening — TODO

> Historical artifact (superseded 2026-07-12): preset aliases, auth facades,
> compatibility readers, and remote-control mutations were removed by the
> canonical relay runtime modernization. The current remote control plane is
> GET/HEAD-only.

Status: Complete
Last updated: 2026-05-20

## M0 — Scope And Characterization

- [x] CAD-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-architecture-deepening]
  Goal: Freeze the five-slice architecture contract and baseline gates.
  Validation: workstream docs agree on scope, task order, and evidence expectations.
  Review: Ensure no task smuggles feature work beyond refactoring.
  Evidence: `DESIGN.md`, `MILESTONES.md`, `EVIDENCE_AND_GATES.md`, `WORKSTREAM.json`, `HANDOFF.md`.
  Handoff: Start with session identity because later preparation refactors depend on its vocabulary.

## M1 — Session Identity Semantics

- [x] CAD-020 [owner=main] [deps=CAD-010] [scope=crates/core/src/proxy/client_identity.rs,crates/core/src/proxy/request_context.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/route_affinity.rs,crates/core/src/state*.rs,crates/core/src/logging*.rs]
  Goal: Introduce an explicit session identity value/source so header session IDs and `prompt_cache_key` affinity fallback are distinguishable in logs and session identity cards while preserving routing keys.
  Validation: targeted identity/affinity tests plus `cargo nextest run -p codex-helper-core prompt_cache_key_affinity --no-fail-fast`.
  Review: Header identity priority and existing admin API compatibility must remain intact.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-020 section records `session_identity_source` examples and fresh gates.
  Handoff: API payloads remain backward compatible: `session_id` is unchanged, new `session_identity_source` fields are optional and omitted for legacy/unknown rows.

## M2 — Shared Codex Request Preparation

- [x] CAD-030 [owner=main] [deps=CAD-020] [scope=crates/core/src/proxy/request_context.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/request_preparation.rs,crates/core/src/proxy/selected_upstream_request.rs]
  Goal: Extract a deeper shared preparation Module used by HTTP requests and Responses WebSocket first frames.
  Validation: `cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity responses_websocket --no-fail-fast`.
  Review: Preserve request logging, model override, route selection, auth injection, and body rewrite behavior.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-030 section records shared preparation call graph and fresh gates.
  Handoff: HTTP still owns byte reading/content-encoding errors; WebSocket still owns first-frame validation and handshake details. Shared Module owns session identity, bindings/overrides, body rewrite, begin-request, route selection, retry plan, and preview setup.

## M3 — Relay Diagnostic Case Registry

- [x] CAD-040 [owner=main] [deps=CAD-010] [scope=crates/core/src/proxy/codex_relay_capabilities.rs,crates/core/src/proxy/codex_relay_live_smoke.rs,crates/core/src/proxy/codex_relay_probe.rs,crates/core/src/proxy/codex_relay_evidence.rs]
  Goal: Replace ad-hoc compact/image/websocket diagnostic branching with registered diagnostic/smoke cases.
  Validation: `cargo nextest run -p codex-helper-core relay_capabilities relay_live_smoke codex_live_smoke --no-fail-fast`.
  Review: Cost-bearing cases still require explicit acknowledgement and keep evidence semantics.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-040 section records probe/live-smoke registry shape and fresh gates.
  Handoff: Probe/live-smoke additions now start by adding a registry descriptor, then implementation/classification functions; public response/evidence payload shapes remain unchanged.

## M4 — Proxy Integration Test Harness

- [x] CAD-050 [owner=main] [deps=CAD-020,CAD-030] [scope=crates/core/src/proxy/tests]
  Goal: Extract reusable test harness Modules for upstream capture, route graph setup, encoding helpers, WebSocket helpers, and affinity assertions; migrate high-churn response semantics tests first.
  Validation: `cargo nextest run -p codex-helper-core proxy::tests::failover::response_semantics --no-fail-fast`.
  Review: Tests should read in domain terms, not hide important assertions behind opaque helpers.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-050 section records the first harness extraction and response-semantics gate.
  Handoff: The harness now covers proxy/upstream server lifecycle, default upstream config, JSON request helpers, and finished-request polling; future migrations should stay selective and keep behavior-specific assertions visible.

## M5 — Codex Patch Plan Seam

- [x] CAD-060 [owner=main] [deps=CAD-010] [scope=crates/core/src/codex_integration.rs,crates/core/src/config_storage.rs,crates/core/src/codex_capability_profile.rs]
  Goal: Split Codex client patching into a pure `CodexPatchPlan` calculation Module and execution Adapters for TOML/auth/switch-state side effects.
  Validation: `cargo nextest run -p codex-helper-core codex_switch codex_bridge codex_client_patch --no-fail-fast`.
  Review: Existing preset aliases, auth facade safety, readiness diagnostics, and remote-control separation must remain stable.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-060 section records patch-plan policy seams and fresh gates.
  Handoff: `CodexPatchPlan` is now the policy source for provider identity, TOML auth/websocket flags, auth patch strategy, runtime readiness, and switch-on effect ordering; execution adapters perform TOML/auth/state writes.

## M6 — Closeout

- [x] CAD-070 [owner=main] [deps=CAD-030,CAD-040,CAD-050,CAD-060] [scope=workspace]
  Goal: Run final verification, update docs, and close or split residual follow-ons.
  Validation:
  - `cargo fmt --check`
  - `cargo nextest run -p codex-helper-core`
  - optional `cargo clippy -p codex-helper-core --all-targets -- -D warnings` if public API/unsafe/traits changed materially
  Review: Use review-workstream and verify-rust-workstream before completion claims.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-20 CAD-070 section records final package gates and the one fixed closeout regression.
  Handoff: No split workstream required; residual optional migrations are documented as follow-ons, not blockers.

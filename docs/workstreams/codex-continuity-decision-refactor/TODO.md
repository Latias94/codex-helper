# Codex Continuity Decision Refactor - TODO

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Evidence Freeze

- [x] CDC-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-continuity-decision-refactor]
  Goal: Freeze the refactor problem, target state, domain inference stance, and evidence gates.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json exist and agree.
  Evidence: docs/workstreams/codex-continuity-decision-refactor/DESIGN.md
  Handoff: Planner confirmed this is a new lane because prior route-continuity lanes are closed and explicitly defer continuity-domain support.

## M1 - Shared Continuity Decision

- [x] CDC-020 [owner=codex] [deps=CDC-010] [scope=crates/core/src/proxy/request_body.rs,crates/core/src/proxy/request_preparation.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/provider_execution.rs]
  Goal: Introduce a shared continuity decision module and route HTTP plus WebSocket compact classification through it without changing fallback behavior yet.
  Validation: cargo nextest run -p codex-helper-core remote_compaction_v2 responses_websocket route_affinity --no-fail-fast
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Handoff: DONE - Added `request_continuity` module, routed HTTP remote-compaction marking through it, classified WebSocket first frames, and logged WebSocket v2 compact requests without changing fallback behavior.

- [x] CDC-030 [owner=codex] [deps=CDC-020] [scope=crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/tests/failover/response_semantics.rs]
  Goal: Make Responses WebSocket `compaction_trigger` state-bound with the same missing-affinity and single-endpoint bootstrap semantics as HTTP v2 compact.
  Validation: cargo nextest run -p codex-helper-core responses_websocket remote_compaction_v2 --no-fail-fast
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Handoff: DONE - WebSocket compact now fails closed with multiple endpoints and no route affinity, and single-endpoint WebSocket compact can bootstrap without prior affinity.

## M2 - Soft Affinity Escape And Domain Policy

- [x] CDC-040 [owner=codex] [deps=CDC-020] [scope=crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/route_affinity.rs,crates/core/src/routing_ir.rs]
  Goal: Ensure ordinary conversation affinity is soft: prefer the prior endpoint, but escape when it is unavailable and the request is not state-bound.
  Validation: cargo nextest run -p codex-helper-core session_route_affinity route_unavailable --no-fail-fast
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Handoff: DONE - Added a regression where `Hard` route affinity pins an ordinary session to an unavailable endpoint and the second ordinary turn escapes to another healthy endpoint instead of returning false 502. Provider-state-bound compact still uses the hard/configured selector.

- [x] CDC-050 [owner=codex] [deps=CDC-030,CDC-040] [scope=crates/core/src/config.rs,crates/core/src/config_storage.rs,crates/core/src/runtime_identity.rs,crates/core/src/proxy]
  Goal: Add explicit `continuity_domain` identity with provider-endpoint default and no relay auto-inference.
  Validation: cargo nextest run -p codex-helper-core continuity_domain route_affinity --no-fail-fast
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Handoff: DONE - Provider and endpoint config now accept explicit `continuity_domain`; unconfigured endpoints default to provider-endpoint identity, same base URL/domain is not inferred, state-bound fallback is allowed only inside an explicit shared continuity domain, and runtime identity migration resets state when continuity domain changes.

## M3 - Official OpenAI And Operator Diagnostics

- [x] CDC-060 [owner=codex] [deps=CDC-050] [scope=crates/core/src/codex_capability_profile.rs,crates/core/src/proxy/codex_relay_capabilities.rs,docs/CONFIGURATION.zh.md]
  Goal: Clarify official OpenAI direct vs relay behavior and surface continuity-domain recommendations in diagnostics.
  Validation: cargo nextest run -p codex-helper-core capabilities codex_capability_profile --no-fail-fast
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Handoff: DONE - Capability profile now states OpenAI identity selects the compact path but does not prove relay state sharing; relay capability diagnostics expose selected continuity domain, explicit-domain status, same-domain endpoint count, and operator warnings/recommendations.

## M4 - Verification And Closeout

- [x] CDC-070 [owner=codex] [deps=CDC-060] [scope=docs/workstreams/codex-continuity-decision-refactor]
  Goal: Verify targeted and broad gates, update evidence, and decide closeout or split follow-ons.
  Validation: verify-rust-workstream records fresh final gate evidence.
  Review: review-workstream has no blocking findings.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json
  Handoff: DONE - Final targeted regression, full core nextest, binary check, formatting, and whitespace gates passed. Lane closes with follow-ons recorded in HANDOFF.md.

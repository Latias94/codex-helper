# Codex Protocol Normalization And Affinity — TODO

Status: Complete
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

- [x] CPNA-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-protocol-normalization-affinity]
  Goal: Freeze the problem: helper should preserve sub2api-like relay capability by normalizing request
  encodings and using `prompt_cache_key` affinity, without inventing upstream features.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, HANDOFF.md exist and agree.
  Evidence: `docs/workstreams/codex-protocol-normalization-affinity/DESIGN.md`
  Handoff: Scope is ready for implementation.

## M1 — Request Content-Encoding Normalization

- [x] CPNA-020 [owner=main] [deps=CPNA-010] [scope=crates/core/src/proxy/request_context.rs,crates/core/src/proxy/mod.rs,crates/core/src/proxy/request_encoding.rs,crates/core/src/proxy/tests/failover/response_semantics.rs]
  Goal: Decode supported compressed HTTP request bodies before JSON inspection and upstream forwarding.
  Validation: `cargo nextest run -p codex-helper-core request_content_encoding --no-fail-fast`
  Review: Verify decoded body is used for overrides/logging/routing and stale `Content-Encoding` is not forwarded.
  Evidence: implemented `request_encoding` and integration tests; final evidence recorded in `EVIDENCE_AND_GATES.md`.
  Handoff: Corrupt or unsupported encodings return `400 BAD_REQUEST` before hitting upstream.

- [x] CPNA-030 [owner=main] [deps=CPNA-020] [scope=crates/core/src/proxy/request_encoding.rs,crates/core/src/config_storage.rs,README.md,README_EN.md,docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md]
  Goal: Add and document a safe escape hatch for raw request encoding passthrough.
  Validation: `cargo nextest run -p codex-helper-core request_content_encoding --no-fail-fast`
  Review: Default must remain normalization; passthrough must be explicit.
  Evidence: `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` is tested and documented.
  Handoff: Escape hatch is an environment variable rather than a persisted config key to avoid schema churn for a rare compatibility path.

## M2 — Prompt Cache Session Affinity

- [x] CPNA-040 [owner=main] [deps=CPNA-020] [scope=crates/core/src/proxy/client_identity.rs,crates/core/src/proxy/request_context.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/tests/failover/response_semantics.rs]
  Goal: Use decoded JSON `prompt_cache_key` as session identity fallback when headers do not provide one.
  Validation: `cargo nextest run -p codex-helper-core prompt_cache_key_affinity --no-fail-fast`
  Review: Header `session_id`/`conversation_id` priority must not change.
  Evidence: unit tests cover header priority; integration test proves `/responses` then `/responses/compact` share prompt-cache route affinity.
  Handoff: Confirm `/responses` and `/responses/compact` stick to the same route when prompt cache key matches.

## M3 — Integration And Docs

- [x] CPNA-050 [owner=main] [deps=CPNA-030,CPNA-040] [scope=README.md,README_EN.md,docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,CHANGELOG.md,crates/core/src/config_storage.rs]
  Goal: Explain that helper normalizes transport-level request encodings and mirrors sub2api-style
  prompt-cache affinity without requiring users to classify their relay.
  Validation: docs match config names and implemented defaults.
  Review: Avoid claiming helper adds missing upstream compact/WebSocket capability.
  Evidence: README/CONFIGURATION/CHANGELOG/config template document defaults, escape hatch, and non-goals.

## M4 — Verification And Closeout

- [x] CPNA-060 [owner=main] [deps=CPNA-050] [scope=workspace]
  Goal: Run fresh targeted and package gates, then close or split follow-ons.
  Validation:
  - `cargo fmt --check`
  - `cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity --no-fail-fast`
  - `cargo nextest run -p codex-helper-core`
  Review: `review-workstream` before closeout.
  Evidence: `EVIDENCE_AND_GATES.md` records `cargo fmt --check`, targeted nextest, and full
  `codex-helper-core` nextest passing on 2026-05-20.
  Handoff: Residual relay risk is limited to unusual upstreams requiring raw compressed bodies; use
  `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough`.

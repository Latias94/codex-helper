# Codex Remote Compaction V2 Continuity - TODO

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Evidence Freeze

- [x] CRC2-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-remote-compaction-v2-continuity]
  Goal: Freeze the provider-opaque v2 compact problem, target state, and gates.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Evidence: docs/workstreams/codex-remote-compaction-v2-continuity/DESIGN.md
  Handoff: Ready for implementation.

## M1 - V2 Compact Classification And Logging

- [x] CRC2-020 [owner=codex] [deps=CRC2-010] [scope=crates/core/src/proxy/request_body.rs,crates/core/src/proxy/request_preparation.rs,crates/core/src/logging.rs]
  Goal: Detect structured `compaction_trigger` bodies on Codex `/responses`
  requests and expose `remote_compaction_v2_request` in request logs.
  Validation: cargo nextest run -p codex-helper-core remote_compaction_v2 request_logs --no-fail-fast
  Review: Detection must not rely on relay identity or log sensitive request body content.
  Evidence: `cargo nextest run -p codex-helper-core remote_compaction_v2 request_logs --no-fail-fast`.
  Handoff: DONE. Body-aware request flavor now detects structured `compaction_trigger` under `input` and request logs expose `codex_bridge.remote_compaction_v2_request`.

## M2 - Continuity Policy For V2 Compact

- [x] CRC2-030 [owner=codex] [deps=CRC2-020] [scope=crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/tests]
  Goal: Apply provider-state-bound continuity and fail-closed behavior to v2
  compact using the existing provider-opaque route affinity contract.
  Validation: cargo nextest run -p codex-helper-core remote_compaction_v2 route_affinity --no-fail-fast
  Review: V2 compact must use known provider affinity, explicitly bootstrap under a tryable policy, or fail closed; it must not accidentally fallback across provider endpoints.
  Evidence: `cargo nextest run -p codex-helper-core remote_compaction_v2 route_affinity --no-fail-fast`; `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`.
  Handoff: DONE. Common remote-compaction policy covers v1 and v2. Later route-continuity work made missing-affinity behavior policy-sensitive: fallback-sticky can bootstrap; hard still fails closed.

## M3 - Documentation And Gates

- [x] CRC2-040 [owner=codex] [deps=CRC2-030] [scope=README.md,README_EN.md,docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,CHANGELOG.md,docs/workstreams/codex-remote-compaction-v2-continuity]
  Goal: Document v2 compact diagnostics, provider-opaque safety semantics, and
  residual continuity-domain follow-up.
  Validation: cargo fmt --all --check && cargo nextest run -p codex-helper-core
  Review: Docs must not imply the proxy can identify sub2api/new-api/OpenAI behind an opaque provider.
  Evidence: `cargo fmt --all --check`; `cargo nextest run -p codex-helper-core`.
  Handoff: DONE. Docs explain v2 compact `/responses` logging, policy-sensitive route continuity, and the fact that presets still do not enable v2 automatically.

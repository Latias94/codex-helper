# Codex Relay Capability Profile - TODO

Status: Complete
Last updated: 2026-05-19

## M0 - Scope And Evidence Freeze

- [x] RCP-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-relay-capability-profile]
  Goal: Freeze the problem, target state, non-goals, and evidence anchors for relay capability
  profiling.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json exist and agree.
  Evidence: docs/workstreams/codex-relay-capability-profile/DESIGN.md
  Handoff: Complete. First executable task is RCP-020.

## M1 - Static Capability Profile

- [x] RCP-020 [owner=codex] [deps=RCP-010] [scope=crates/core/src/codex_capability_profile.rs,crates/core/src/lib.rs,crates/core/src/proxy/models_compat.rs]
  Goal: Add a static `CodexCapabilityProfile` that explains what Codex should expose for each
  patch mode and model catalog shape.
  Validation: cargo nextest run -p codex-helper-core codex_capability_profile
  Review: review-workstream before accepting completion.
  Evidence: Unit tests covering remote compaction v1, image generation exposure, WebSocket disabled
  state, and missing model metadata.
  Handoff: DONE. Added a pure/static profile module and model translation integration test; no
  active network probes were added.

## M2 - Relay Probe Evidence

- [x] RCP-030 [owner=codex] [deps=RCP-020] [scope=crates/core/src/proxy,crates/core/src/config.rs]
  Goal: Add bounded relay probe primitives for `/models`, ordinary `/responses`, and
  `/responses/compact` without creating a retry storm or mutating scheduler state.
  Validation: cargo nextest run -p codex-helper-core codex_relay_probe
  Review: review-workstream for probe safety and route-selection behavior.
  Evidence: Tests for success, unsupported compact, malformed `/models`, and provider affinity.
  Handoff: DONE. Added single-upstream probe primitives, raw `/models` shape classification,
  validation-only endpoint probes, and bounded response reads. Hosted `image_generation` probe must
  remain a separate explicit action.

- [x] RCP-040 [owner=codex] [deps=RCP-030] [scope=crates/core/src/proxy,crates/core/src/bin-or-cli-modules]
  Goal: Expose probe/profile output through a CLI or admin API surface with mismatch reasons and
  confidence labels.
  Validation: cargo nextest run -p codex-helper-core codex_capabilities_api
  Review: review-workstream for operator clarity.
  Evidence: Snapshot or structured tests proving expected/observed/recommendation fields.
  Handoff: DONE. Added an admin API first surface at
  `/__codex_helper/api/v1/codex/relay-capabilities`; it reports selected station/upstream, static
  expected profile, active `/models` + `/responses` + `/responses/compact` probe results, and
  mismatch reasons. CLI/TUI views can reuse the same response later.

## M3 - Recommendations And Docs

- [x] RCP-050 [owner=codex] [deps=RCP-040] [scope=crates/core,docs]
  Goal: Add deterministic patch-mode recommendations for common relay capability combinations.
  Validation: cargo nextest run -p codex-helper-core codex_patch_mode_recommendation
  Review: review-workstream for false-positive risk.
  Evidence: Matrix tests for `default`, `imagegen-bridge`, `official-relay-bridge`, and
  `official-imagegen-bridge`.
  Handoff: DONE. Added a reusable recommendation model and surfaced it through Codex relay
  diagnostics. The matrix avoids official relay modes when `/responses/compact` is unknown or
  unsupported, and reports uncertainty/warnings instead of silently upgrading.

- [x] RCP-060 [owner=codex] [deps=RCP-050] [scope=docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,CHANGELOG.md]
  Goal: Document the capability profile, safe probes, relay-agnostic guidance, and known limits.
  Validation: cargo fmt --check
  Review: review-workstream for doc/code agreement.
  Evidence: Documentation diff and changelog entry.
  Handoff: DONE. English/Chinese configuration docs now explain the Codex relay capabilities admin
  API, safe active probes, sub2api and non-sub2api model catalog behavior, conservative
  recommendations, imagegen probe side effects, unsupported WebSocket relay, and remote compaction
  v2 limits. CHANGELOG.md has matching bilingual entries.

## M4 - Closeout

- [x] RCP-070 [owner=codex] [deps=RCP-060] [scope=docs/workstreams/codex-relay-capability-profile]
  Goal: Close the lane or split follow-ons for WebSocket relay, remote compaction v2, or richer
  hosted tool probes.
  Validation: verify-rust-workstream records fresh final gate evidence.
  Review: review-workstream has no blocking findings.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json, optional CLOSEOUT.md.
  Handoff: DONE. Closed the lane after targeted gates and full `cargo nextest run -p
  codex-helper-core` passed. Deferred WebSocket relay, remote compaction v2 enablement, and paid
  hosted-tool active probes to explicit follow-on lanes.

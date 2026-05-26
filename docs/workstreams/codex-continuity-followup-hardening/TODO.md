# Codex Continuity Follow-Up Hardening - TODO

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Release Boundary

- [x] CCFH-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-continuity-followup-hardening]
  Goal: Freeze the six requested follow-ups, target state, and validation gates.
  Validation: Workstream docs exist and agree.
  Evidence: DESIGN.md, WORKSTREAM.json.
  Handoff: Created from the user's request to finish the six continuity hardening follow-ups.

- [x] CCFH-020 [owner=codex] [deps=CCFH-010] [scope=Cargo.toml,dist-workspace.toml,apps/desktop/src-tauri/Cargo.toml,.github/workflows/release.yml]
  Goal: Make release planning publish CLI artifacts only and stop cargo-dist from building the desktop Tauri package or requiring desktop sidecars.
  Validation: `dist host --steps=create --tag=v0.17.0 --output-format=json > plan-dist-manifest.json`; inspect manifest for no `codex-helper-desktop` artifact.
  Review: Confirm desktop still has local/CI sidecar preparation outside release publish.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - Added explicit cargo-dist package metadata, marked Tauri desktop as non-dist, set workspace default members to the root CLI, and enabled `precise-builds = true` so cargo-dist builds `--package=codex-helper` instead of `--workspace`.

## M1 - Continuity Topology And Regression Shape

- [x] CCFH-030 [owner=codex] [deps=CCFH-010] [scope=crates/core/src/routing_ir.rs,crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/responses_websocket.rs,crates/core/src/proxy/codex_relay_capabilities.rs]
  Goal: Extract one topology helper for endpoint/domain counts and same-continuity-domain checks used by HTTP, WebSocket, and diagnostics.
  Validation: `cargo nextest run -p codex-helper-core continuity_domain route_affinity capabilities --no-fail-fast`.
  Review: Check that helper removes duplicate counting logic and keeps explicit-domain semantics unchanged.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - Added `RoutePlanContinuityTopology`; HTTP, WebSocket, and relay diagnostics now reuse it for configured endpoint counts, selected domain summary, and same-domain endpoint counts.

- [x] CCFH-040 [owner=codex] [deps=CCFH-030] [scope=crates/core/src/proxy/tests/failover/response_semantics.rs,crates/core/src/proxy/tests/failover/response_semantics_compact.rs,crates/core/src/proxy/tests/failover/response_semantics_websocket.rs]
  Goal: Split the large response semantics regression file by behavior while preserving test names and helpers.
  Validation: `cargo nextest run -p codex-helper-core response_semantics remote_compaction_v2 responses_websocket --no-fail-fast`.
  Review: Confirm no coverage is dropped and module filters remain usable.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - Split compact/continuity and Responses WebSocket regressions into sibling modules while keeping shared harness imports in the parent module.

## M2 - Operator Configuration Surfaces

- [x] CCFH-050 [owner=codex] [deps=CCFH-030] [scope=crates/core/src/dashboard_core,crates/core/src/proxy/providers_api.rs,crates/tui,crates/gui,apps/desktop/src,apps/desktop/src-tauri]
  Goal: Expose and preserve `continuity_domain` in provider/endpoint API DTOs, TUI displays, and desktop provider editing flows.
  Validation: `cargo nextest run -p codex-helper-core persisted_crud runtime_overrides --no-fail-fast`; `cargo nextest run -p codex-helper-desktop common_edit --no-fail-fast`; `cargo nextest run -p codex-helper-gui provider_editor format_attached_provider_endpoint_identity --no-fail-fast`; `cargo nextest run -p codex-helper-tui codex_relay_diagnostics provider_tags_brief --no-fail-fast`; `pnpm --dir apps/desktop test -- --run`.
  Review: Check edits do not drop existing provider fields and UI copy remains operator-focused.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - Added continuity-domain fields to provider endpoint DTOs, provider catalog tags, desktop edit payloads, Tauri provider commands, TUI summaries, and GUI provider editor/runtime displays.

## M3 - Diagnostics And Official OpenAI Stance

- [x] CCFH-060 [owner=codex] [deps=CCFH-030] [scope=crates/core/src/codex_capability_profile.rs,crates/core/src/proxy/codex_relay_capabilities.rs,docs/CONFIGURATION*.md]
  Goal: Make the official OpenAI direct heuristic stance conservative and testable: either implement a fully proven official-only rule or explicitly defer it with diagnostics and docs.
  Validation: `cargo nextest run -p codex-helper-core codex_capability_profile capabilities continuity_domain --no-fail-fast`.
  Review: Verify relay endpoints never gain shared continuity from domain/base URL similarity.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - Kept automatic official-domain grouping deferred; added default-compatible diagnostic structs and tests proving same official-looking host/base URL does not create shared continuity without explicit `continuity_domain`.

- [x] CCFH-070 [owner=codex] [deps=CCFH-050,CCFH-060] [scope=src,crates/tui,crates/gui,apps/desktop/src]
  Goal: Update capability diagnostic CLI/TUI/GUI displays for `expected.continuity` and selected `continuity` while remaining compatible with older responses.
  Validation: `cargo check -p codex-helper`; `cargo nextest run -p codex-helper-core capabilities --no-fail-fast`; `pnpm --dir apps/desktop test -- --run`.
  Review: Check absent continuity fields degrade gracefully.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE - CLI and TUI diagnostics now print expected continuity support, selected domain details, failover eligibility, and continuity warnings while defaulting absent response fields.

## M4 - Final Verification

- [x] CCFH-080 [owner=codex] [deps=CCFH-020,CCFH-040,CCFH-070] [scope=repo]
  Goal: Run focused and broad gates, update evidence, and prepare a conventional commit.
  Validation: `cargo fmt --all --check`; `cargo nextest run -p codex-helper-core --no-fail-fast`; `cargo check -p codex-helper`; desktop tests where applicable; `git diff --check`.
  Review: Workstream review and final sanity check.
  Evidence: EVIDENCE_AND_GATES.md, HANDOFF.md.
  Handoff: DONE - Focused release, topology, response semantics, persistence, desktop, GUI, TUI, CLI, and broad core gates passed; workstream evidence and handoff were updated before commit.

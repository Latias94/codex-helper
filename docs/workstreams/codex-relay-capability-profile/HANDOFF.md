# Codex Relay Capability Profile - Handoff

Status: Complete
Last updated: 2026-05-19

> Historical status (superseded 2026-07-12): patch-mode profiles and deterministic preset recommendations were replaced by provider-owned expected decisions, captured catalog revisions, request dialects, observations, continuity, and mismatches. Capability diagnostics are explicit process-local CLI/TUI actions; the remote control plane no longer exposes a diagnostic POST route.

## Current State

The workstream has been opened as a follow-on to the completed official relay and official imagegen
bridge lanes. RCP-020 is complete: a pure static capability profile now models Codex client-side
official feature exposure from patch mode and model catalog metadata.

RCP-030 is complete: bounded relay probe primitives can classify `/models`, `/responses`, and
`/responses/compact` for one explicit upstream without going through normal routing, retry,
request-ledger, affinity, or passive-health paths.

RCP-040 is complete: operators can now call an admin API endpoint to compare the static Codex
capability profile against active relay probe observations for one selected upstream.

RCP-050 is complete: diagnostics now include deterministic patch-mode recommendations derived from
ordinary `/responses` support, `/responses/compact` support, and selected model image capability.

RCP-060 is complete: configuration docs and changelog now describe the diagnostics endpoint,
recommendation matrix, model catalog translation, and known limits.

RCP-070 is complete: the lane is closed after targeted gates and the full core package gate passed.

## Active Task

None. This workstream is complete.

## Completed Tasks

- Task ID: RCP-070
- Owner: codex
- Files: `docs/workstreams/codex-relay-capability-profile`
- Validation: `cargo nextest run -p codex-helper-core`; `cargo fmt --check`
- Status: DONE
- Review: closeout review fixed one consistency issue: omitted `patch_mode` now defaults from the
  current Codex switch status before helper config/default.
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

- Task ID: RCP-060
- Owner: codex
- Files: `docs/CONFIGURATION.md`, `docs/CONFIGURATION.zh.md`, `CHANGELOG.md`
- Validation: `cargo fmt --check`
- Status: DONE
- Review: self-review found no blocking doc/code mismatch. The docs explicitly keep imagegen
  active probes, WebSocket relay, and remote compaction v2 out of the enabled behavior.
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

- Task ID: RCP-050
- Owner: codex
- Files: `crates/core/src/codex_capability_profile.rs`,
  `crates/core/src/proxy/control_plane/codex_capabilities.rs`,
  `crates/core/src/proxy/tests/api_admin/capabilities.rs`
- Validation: `cargo nextest run -p codex-helper-core codex_patch_mode_recommendation`;
  `cargo nextest run -p codex-helper-core codex_capability_profile`;
  `cargo nextest run -p codex-helper-core codex_capabilities_api`;
  `cargo nextest run -p codex-helper-core codex_relay_probe`; `cargo fmt --check`
- Status: DONE
- Review: self-review found no blocking findings. Residual risk: recommendations intentionally do
  not prove hosted image generation entitlement; they only prove client-side exposure from model
  metadata and warn that imagegen active probes remain explicit.
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

- Task ID: RCP-040
- Owner: codex
- Files: `crates/core/src/proxy/control_plane/codex_capabilities.rs`,
  `crates/core/src/proxy/codex_relay_probe.rs`,
  `crates/core/src/proxy/control_plane_manifest.rs`,
  `crates/core/src/dashboard_core`
- Validation: `cargo nextest run -p codex-helper-core codex_capabilities_api`;
  `cargo nextest run -p codex-helper-core codex_relay_probe`;
  `cargo nextest run -p codex-helper-core codex_capability_profile`; `cargo fmt --check`
- Status: DONE
- Review: self-review found no blocking findings. Residual risk: the endpoint is deliberately
  active and should remain POST/opt-in because it sends validation-only requests to the selected
  upstream.
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

- Task ID: RCP-030
- Owner: codex
- Files: `crates/core/src/proxy/codex_relay_probe.rs`, `crates/core/src/proxy/models_compat.rs`,
  `crates/core/src/proxy/mod.rs`
- Validation: `cargo nextest run -p codex-helper-core codex_relay_probe`; `cargo fmt --check`
- Status: DONE
- Review: self-review found and fixed one important observability risk: using the normal
  `/models` compatibility helper hid whether the relay returned raw OpenAI `data` or Codex
  `models`. Probe classification now decodes compressed bodies without translating them.
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

- Task ID: RCP-020
- Owner: codex
- Files: `crates/core/src/codex_capability_profile.rs`, `crates/core/src/lib.rs`,
  `crates/core/src/proxy/models_compat.rs`
- Validation: `cargo nextest run -p codex-helper-core codex_capability_profile`; `cargo fmt --check`
- Status: DONE
- Review: self-review found and fixed mode-derived auth/provider coupling before completion
- Evidence: `docs/workstreams/codex-relay-capability-profile/EVIDENCE_AND_GATES.md`

## Decisions Since Last Update

- Do not reopen completed bridge workstreams; this lane owns capability profile/probe/recommendation.
- Keep WebSocket forwarding out of scope.
- Keep remote compaction v2 diagnostic-only.
- Keep hosted image generation active probes explicit because they may have side effects.
- RCP-020 stays pure/static; relay probes start in RCP-030.
- RCP-030 probes one explicit upstream at a time; RCP-040 should decide how operator-facing surfaces
  select providers/endpoints to probe.
- RCP-040 chose admin API first, not CLI first, because existing GUI/TUI/remote attach flows already
  discover operator features through `/__codex_helper/api/v1/capabilities` and operator summary
  links.
- RCP-050 recommendations are conservative: unknown `/responses` or `/responses/compact` support
  never upgrades to an official relay mode.
- RCP-060 documents the admin API as `POST` because it performs active validation probes, not just
  a passive read.

## Blockers

None.

## Next Recommended Action

Commit the completed lane. Recommended follow-ons, if desired:

- Add TUI/GUI controls that call the existing admin API instead of duplicating capability logic.
- Open a separate WebSocket relay lane only if helper will actually forward Responses WebSocket.
- Open a separate remote compaction v2 lane only after Codex/relay semantics are stable.
- Add explicit, paid hosted-tool probes only behind a clear operator confirmation.

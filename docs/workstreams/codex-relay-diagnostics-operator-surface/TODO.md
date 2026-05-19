# Codex Relay Diagnostics Operator Surface — TODO

Status: Implemented
Last updated: 2026-05-19

## M0 — Scope And Evidence Freeze

- [x] RDO-010 [owner=codex] [deps=none] [scope=docs/workstreams/codex-relay-diagnostics-operator-surface]
  Goal: Freeze problem, target state, non-goals, and evidence anchors.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json exist and agree.
  Evidence: `docs/workstreams/codex-relay-diagnostics-operator-surface/DESIGN.md`
  Handoff: TUI Settings is the first visible surface; GUI/CLI are follow-ons.

## M1 — Reusable Core Diagnostic Contract

- [x] RDO-020 [owner=codex] [deps=RDO-010] [scope=crates/core/src/codex_capability_profile.rs,crates/core/src/proxy]
  Goal: Make Codex relay capability diagnostics callable from core code, not only from a private HTTP handler.
  Validation: `cargo nextest run -p codex-helper-core codex_capabilities_api`
  Review: Verify the HTTP response shape stays compatible and private route code is now a thin adapter.
  Evidence: `crates/core/src/proxy/control_plane/codex_capabilities.rs`
  Handoff: Added public DTO exports and `ProxyService::codex_relay_capabilities`; HTTP route now delegates to the service method.

## M2 — TUI Settings Diagnostic Surface

- [x] RDO-030 [owner=codex] [deps=RDO-020] [scope=crates/tui/src/tui]
  Goal: Add a manual TUI Settings action that runs the diagnostic asynchronously and renders result summary.
  Validation: `cargo nextest run -p codex-helper-tui codex_relay_diagnostics`
  Review: Confirm no periodic probe, no UI blocking, and no automatic patch-mode mutation.
  Evidence: `crates/tui/src/tui/view/pages/settings.rs`
  Handoff: Settings `C` triggers an async one-shot probe and renders target, patch mode, model, observed endpoint support, mismatches, recommendation, and warnings.

## M3 — Docs And Closeout

- [x] RDO-040 [owner=codex] [deps=RDO-030] [scope=docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,CHANGELOG.md]
  Goal: Document the TUI path alongside the existing admin API path.
  Validation: `rg "relay diagnostics|能力诊断|Settings" docs/CONFIGURATION.md docs/CONFIGURATION.zh.md CHANGELOG.md -n`
  Review: Confirm docs do not imply hosted image generation is actively probed.
  Evidence: Documentation diff and changelog entry.
  Handoff: Docs and changelog now describe TUI Settings `C` as diagnostic-only; GUI and CLI remain explicit follow-ons.

- [x] RDO-050 [owner=codex] [deps=RDO-040] [scope=workspace]
  Goal: Run final gates, update evidence, close or hand off the lane.
  Validation: `cargo fmt --check`; targeted nextest gates; package gates if shared contracts changed.
  Review: Fresh review-workstream/verification before completion.
  Evidence: `docs/workstreams/codex-relay-diagnostics-operator-surface/EVIDENCE_AND_GATES.md`
  Handoff: Final gates pass; GUI/CLI remain follow-on surfaces, and no live relay smoke was run.

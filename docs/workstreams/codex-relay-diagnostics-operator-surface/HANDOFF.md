# Codex Relay Diagnostics Operator Surface — Handoff

Status: Implemented
Last updated: 2026-05-19

> Historical status (superseded 2026-07-12): the manual TUI diagnostic remains, but it now renders provider contract decisions, observations, continuity, and mismatches without patch-mode recommendations. Capability diagnostics are process-local and are not exposed through a remote mutation endpoint.

## Current State

Core diagnostics are reusable through `ProxyService::codex_relay_capabilities`. TUI Settings now exposes a manual `C` action that runs a bounded async diagnostic and renders expected/observed/mismatch/recommendation details. Docs and changelog have been updated, and final targeted plus package gates pass.

## Active Task

- Task ID: none
- Owner: codex
- Files: workspace, `docs/workstreams/codex-relay-diagnostics-operator-surface/*`
- Validation: `cargo fmt --check`; `cargo nextest run -p codex-helper-core codex_capabilities_api`; `cargo nextest run -p codex-helper-tui codex_relay_diagnostics`; `cargo nextest run -p codex-helper-core`; `cargo nextest run -p codex-helper-tui`
- Status: COMPLETE
- Review: closeout review found no blocking issues
- Evidence: `EVIDENCE_AND_GATES.md`

## Decisions Since Last Update

- TUI Settings is the first visible surface because it already owns Codex patch-mode actions.
- The probe remains manual, not periodic.
- TUI should call a reusable `ProxyService` method instead of loopback HTTP.
- GUI and CLI are follow-ons.
- TUI action is manual and diagnostic-only; it does not auto-apply the recommendation.

## Blockers

- None known.

## Concerns

- GUI and CLI still rely on the admin API/curl path; they are explicit follow-ons, not part of this lane.
- No live relay smoke was run in this automated pass.

## Next Recommended Action

- Commit the completed TUI-first operator surface, then decide whether GUI/CLI or live relay smoke automation should be the next workstream.

# Codex Relay Live Smoke Diagnostics — Handoff

Status: Active
Last updated: 2026-05-19

## Current State

The workstream is open. Core live smoke and the admin API surface are implemented and validated. Capability diagnostics remain validation-only and should not be changed into live checks.

## Completed Tasks

- Task ID: RLS-020
- Owner: codex
- Files: `crates/core/src/proxy`
- Validation: `cargo nextest run -p codex-helper-core codex_relay_live_smoke`
- Status: DONE
- Review: opt-in guard, one selected upstream, one request per case, no scheduler state pollution
- Evidence: `EVIDENCE_AND_GATES.md`

- Task ID: RLS-030
- Owner: codex
- Files: `crates/core/src/proxy/control_plane*`, `crates/core/src/dashboard_core`
- Validation: `cargo nextest run -p codex-helper-core codex_live_smoke_api`
- Status: DONE
- Review: admin route must stay protected, manifest/operator-summary should advertise the endpoint, and missing acknowledgement must fail before upstream IO.
- Evidence: admin API tests plus `EVIDENCE_AND_GATES.md`

## Active Task

- Task ID: RLS-040
- Owner: codex
- Files: `crates/tui/src/tui`
- Validation: `cargo nextest run -p codex-helper-tui codex_relay_live_smoke`
- Status: READY
- Review: no single accidental key starts live smoke; compact-only and image-explicit flows should be distinguishable.
- Evidence: TUI tests plus `EVIDENCE_AND_GATES.md`

## Decisions Since Last Update

- Core live smoke is implemented as a separate service method from capability diagnostics.
- Acknowledgement string is `run-live-codex-relay-smoke`.
- Default live smoke should avoid image generation unless explicitly requested.
- Real image artifacts are not stored by helper; returned image-generation items are classified and summarized only.
- Codex relay target selection is shared between capability diagnostics and live smoke.
- Admin API endpoint is `POST /__codex_helper/api/v1/codex/relay-live-smoke`.

## Blockers

- None known.

## Concerns

- Hosted image generation is model-mediated; a successful `/responses` call with the hosted tool may not always produce an `image_generation_call`.
- Real relay smoke may cost money and should stay manual.

## Next Recommended Action

- Implement RLS-040 TUI Settings trigger and rendering using `CodexRelayLiveSmokeRequest` / `CodexRelayLiveSmokeResponse`.

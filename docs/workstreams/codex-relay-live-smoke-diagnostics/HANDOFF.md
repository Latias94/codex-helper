# Codex Relay Live Smoke Diagnostics — Handoff

Status: Complete
Last updated: 2026-05-19

## Current State

The workstream is closed. Core live smoke, the admin API surface, the TUI Settings operator flow, and user-facing docs are implemented and validated. Capability diagnostics remain validation-only and should not be changed into live checks.

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

- Task ID: RLS-040
- Owner: codex
- Files: `crates/tui/src/tui`
- Validation: `cargo nextest run -p codex-helper-tui codex_relay_live_smoke`
- Status: DONE
- Review: no single accidental key starts live smoke; compact-only and image-explicit flows should be distinguishable.
- Evidence: TUI tests plus `EVIDENCE_AND_GATES.md`

## Completed Closeout Task

- Task ID: RLS-050
- Owner: codex
- Files: `docs`, `CHANGELOG.md`
- Validation: `cargo fmt --check`; targeted nextest gates from `EVIDENCE_AND_GATES.md`
- Status: DONE
- Review: docs must describe live smoke as manual/cost-bearing, not a free health check.
- Evidence: docs/changelog plus closeout gates.

## Decisions Since Last Update

- Core live smoke is implemented as a separate service method from capability diagnostics.
- Acknowledgement string is `run-live-codex-relay-smoke`.
- Default live smoke should avoid image generation unless explicitly requested.
- Real image artifacts are not stored by helper; returned image-generation items are classified and summarized only.
- Codex relay target selection is shared between capability diagnostics and live smoke.
- Admin API endpoint is `POST /__codex_helper/api/v1/codex/relay-live-smoke`.
- TUI Settings uses `X` double-confirm for compact-only smoke and `Y` double-confirm for compact+image smoke.

## Blockers

- None known.

## Concerns

- Hosted image generation is model-mediated; a successful `/responses` call with the hosted tool may not always produce an `image_generation_call`.
- Real relay smoke may cost money and should stay manual.

## Next Recommended Action

- Split real paid relay smoke logs into a follow-on evidence lane only if the user wants manual
  upstream validation against specific relay accounts.

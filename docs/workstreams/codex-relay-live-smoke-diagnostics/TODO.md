# Codex Relay Live Smoke Diagnostics — TODO

Status: Active
Last updated: 2026-05-19

## M0 — Scope And Safety Contract

- [x] RLS-010 [owner=codex] [deps=none] [scope=docs/workstreams/codex-relay-live-smoke-diagnostics]
  Goal: Freeze live-smoke scope, opt-in contract, non-goals, and safety invariants.
  Validation: Workstream docs exist and agree.
  Review: Confirm live smoke is separate from validation-only capability diagnostics.
  Evidence: `docs/workstreams/codex-relay-live-smoke-diagnostics/DESIGN.md`
  Handoff: Live smoke requires `run-live-codex-relay-smoke`; default cases avoid image generation.

## M1 — Core Live Smoke Contract

- [x] RLS-020 [owner=codex] [deps=RLS-010] [scope=crates/core/src/proxy]
  Goal: Add reusable core DTOs, request builders, classifiers, and service method for one-upstream live smoke.
  Validation: `cargo nextest run -p codex-helper-core codex_relay_live_smoke`
  Review: Verify missing acknowledgement makes zero upstream requests, selected cases send one request each, and executor bypasses normal routing state.
  Evidence: `crates/core/src/proxy/codex_relay_live_smoke.rs`
  Handoff: DONE 2026-05-19; API/TUI should consume the core DTOs rather than duplicating JSON shapes.

## M2 — Admin API Surface

- [x] RLS-030 [owner=codex] [deps=RLS-020] [scope=crates/core/src/proxy/control_plane*,crates/core/src/dashboard_core]
  Goal: Expose live smoke through an admin API and manifest/operator-summary links.
  Validation: `cargo nextest run -p codex-helper-core codex_live_smoke_api`
  Review: Confirm endpoint is admin protected and opt-in rejection happens before upstream IO.
  Evidence: Admin API tests.
  Handoff: DONE 2026-05-19; API route should be documented as cost-bearing and manual-only.

## M3 — TUI Operator Flow

- [ ] RLS-040 [owner=codex] [deps=RLS-030] [scope=crates/tui/src/tui]
  Goal: Add a TUI Settings live-smoke trigger with a deliberate confirmation flow and result rendering.
  Validation: `cargo nextest run -p codex-helper-tui codex_relay_live_smoke`
  Review: Confirm no single accidental key starts live smoke and loading/stale results are handled.
  Evidence: TUI tests.
  Handoff: TUI should reuse model inference from capability diagnostics where possible.

## M4 — Docs, Evidence, Closeout

- [ ] RLS-050 [owner=codex] [deps=RLS-040] [scope=docs,CHANGELOG.md]
  Goal: Document live smoke semantics, risks, examples, and close the lane with fresh gates.
  Validation: `cargo fmt --check`; targeted nextest gates; package gates if contracts changed.
  Review: Confirm docs do not imply live smoke is free, automatic, or a health check.
  Evidence: `docs/workstreams/codex-relay-live-smoke-diagnostics/EVIDENCE_AND_GATES.md`
  Handoff: Split real relay manual smoke logs into follow-on evidence if not run in automation.

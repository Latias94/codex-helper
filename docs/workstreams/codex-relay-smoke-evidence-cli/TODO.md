# Codex Relay Smoke Evidence CLI — TODO

Status: Complete
Last updated: 2026-05-19

## M0 — Scope And Evidence Contract

- [x] RSE-010 [owner=codex] [deps=none] [scope=docs/workstreams/codex-relay-smoke-evidence-cli]
  Goal: Freeze evidence-store scope, safety invariants, CLI surface, and non-goals.
  Validation: Workstream docs exist and agree.
  Review: Confirm evidence is not routing health and live smoke remains opt-in.
  Evidence: `DESIGN.md`
  Handoff: Evidence JSONL is separate from `requests.jsonl`.

## M1 — Core Evidence Store

- [x] RSE-020 [owner=codex] [deps=RSE-010] [scope=crates/core/src/proxy]
  Goal: Add Codex relay evidence DTOs, JSONL append/read helpers, and service integration for
  capability diagnostics and live smoke.
  Validation: `cargo nextest run -p codex-helper-core codex_relay_evidence`
  Review: Missing live-smoke acknowledgement must not append evidence; append errors must be
  non-fatal for diagnostics.
  Evidence: Core tests plus `EVIDENCE_AND_GATES.md`
  Handoff: DONE 2026-05-19; CLI reads evidence through core exports and service methods append
  evidence after successful response construction.

## M2 — CLI Operator Surface

- [x] RSE-030 [owner=codex] [deps=RSE-020] [scope=src/cli_types.rs,src/cli_app.rs,src/commands]
  Goal: Add terminal-first commands for relay capability diagnostics, live smoke, and evidence
  listing.
  Validation: `cargo nextest run -p codex-helper codex_relay_cli`
  Review: Live smoke requires explicit acknowledgement; default live smoke is compact-only.
  Evidence: CLI tests/manual command output plus `EVIDENCE_AND_GATES.md`
  Handoff: DONE 2026-05-19; `codex-helper codex relay-*` uses local `ProxyService` calls and does
  not require a running admin listener.

## M3 — Docs, Gates, Closeout

- [x] RSE-040 [owner=codex] [deps=RSE-030] [scope=docs,CHANGELOG.md]
  Goal: Document CLI usage, evidence path, safety boundary, and close the lane with fresh gates.
  Validation: `cargo fmt --check`; targeted nextest gates; package gates if public contracts changed.
  Review: Docs must not imply evidence automatically changes routing or patch mode.
  Evidence: `EVIDENCE_AND_GATES.md`
  Handoff: DONE 2026-05-19; split WebSocket/v2 smoke or evidence UI into follow-on lanes.

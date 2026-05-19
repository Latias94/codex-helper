# Codex Relay Smoke Evidence CLI — Handoff

Status: Complete
Last updated: 2026-05-19

## Current State

The workstream is closed. Core evidence storage, service integration, CLI operator commands, docs,
changelog, and closeout gates are complete.

## Completed Tasks

- Task ID: RSE-020
- Owner: codex
- Files: `crates/core/src/proxy`
- Validation: `cargo nextest run -p codex-helper-core codex_relay_evidence`
- Status: DONE
- Review: evidence must be separate from routing health and request ledger.
- Evidence: `EVIDENCE_AND_GATES.md`

- Task ID: RSE-030
- Owner: codex
- Files: `src/cli_types.rs`, `src/cli_app.rs`, `src/commands/codex.rs`
- Validation: `cargo nextest run -p codex-helper codex_relay_cli`
- Status: DONE
- Review: live smoke requires explicit acknowledgement and defaults to compact-only.
- Evidence: `EVIDENCE_AND_GATES.md`

- Task ID: RSE-040
- Owner: codex
- Files: `docs`, `CHANGELOG.md`
- Validation: targeted gates, package gates, `cargo fmt --check`
- Status: DONE
- Review: docs keep evidence diagnostic-only.
- Evidence: `EVIDENCE_AND_GATES.md`

## Decisions

- Evidence file: `~/.codex-helper/logs/codex_relay_evidence.jsonl`.
- Evidence payloads use existing summarized response DTOs; no raw images, base64, credentials, or
  request bodies.
- CLI should call local `ProxyService` directly instead of requiring a running admin server.
- Live smoke acknowledgement remains `run-live-codex-relay-smoke`.

## Blockers

- None known.

## Next Recommended Action

- Use the new CLI against real relay accounts only when manual paid smoke evidence is desired.
  Evidence UI, WebSocket smoke, and remote compaction v2 smoke should be split into follow-on lanes.

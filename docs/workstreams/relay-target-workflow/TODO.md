# Relay Target Workflow - TODO

Status: Complete
Last updated: 2026-05-31

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## M0 - Scope And Evidence Freeze

- [x] RTW-010 [owner=codex] [deps=none] [scope=docs/workstreams/relay-target-workflow]
  Goal: Freeze the relay target UX, workstream scope, evidence anchors, and validation plan.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, HANDOFF.md, and CONTEXT.jsonl exist and agree.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Workstream docs and scope are aligned.

## M1 - Shared Target And Control Plane

- [x] RTW-020 [owner=codex] [deps=RTW-010] [scope=crates/core/src,src/cli_types.rs,src/cli_app.rs]
  Goal: Add relay target config types, target resolution, and shared admin/control-plane client primitives.
  Validation: `cargo nextest run --locked -p codex-helper-core relay_target control_plane --no-fail-fast`.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Relay target config, resolution, and shared control-plane client are implemented.

## M2 - Daily Relay CLI

- [x] RTW-030 [owner=codex] [deps=RTW-020] [scope=src/cli_types.rs,src/cli_app.rs,crates/core/src]
  Goal: Implement `ch relay add/list/status/off/<target>` with built-in `local`, named remote targets, `--no-tui`, and `--attach-only`.
  Validation: `cargo nextest run --locked -p codex-helper relay_cli --no-fail-fast`; `cargo check --locked -p codex-helper`.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. `ch relay` target commands are implemented while `switch on` remains compatible.

## M3 - Remote Attached TUI

- [x] RTW-040 [owner=codex] [deps=RTW-020,RTW-030] [scope=crates/tui/src/tui,src/cli_app.rs]
  Goal: Refactor attached TUI to use resolved admin base URLs and support remote relay target observation without local loopback assumptions.
  Validation: `cargo nextest run --locked -p codex-helper-tui attached --no-fail-fast`; `cargo check --locked -p codex-helper-tui`.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Attached TUI now uses resolved admin URLs and preserves observer semantics.

## M4 - Docs, Compatibility, And Closeout

- [x] RTW-050 [owner=codex] [deps=RTW-030,RTW-040] [scope=README.md,README_EN.md,docs/DOCKER_COMPOSE.md,docs/CONFIGURATION*.md,docs/workstreams/relay-target-workflow]
  Goal: Document daily local and NAS relay workflows, run final gates, and close or split follow-ons.
  Validation: `cargo fmt --all -- --check`; focused checks/tests; Docker compose config check if deployment docs change.
  Evidence: EVIDENCE_AND_GATES.md.
  Handoff: DONE. Docs and validation evidence are recorded; commit is ready.

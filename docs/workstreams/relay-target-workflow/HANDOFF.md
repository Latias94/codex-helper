# Relay Target Workflow - Handoff

Status: Complete
Last updated: 2026-05-31

## Current State

The lane is complete. The shipped product shape is target-first daily use:

- `ch` stays the existing local foreground shortcut.
- `ch relay local` selects the built-in local target.
- `ch relay <name>` selects a named remote relay target, switches Codex to it, and attaches TUI unless flags say otherwise.
- `ch relay add/list/status/off/use` provide target management without moving client patching into the container/server runtime.
- Remote attached TUI uses the target admin URL and optional admin token env var.

## Required Context

Read `DESIGN.md`, `TODO.md`, `EVIDENCE_AND_GATES.md`, `CONTEXT.jsonl`, ADR-0001, and the runtime-boundary-refactor workstream before editing.

## Guardrails

- Preserve existing `ch`, `serve`, and `switch` behavior.
- Do not store admin tokens in config files.
- Do not claim host-local transcript/session-file access for remote targets.
- Keep container server client patching outside `crates/server`.

## Follow-On Notes

- The attached TUI still builds provider option labels from local config. Remote runtime pages and control-plane status use the remote admin API; making provider editing fully remote should be split into a dedicated TUI/control-plane lane.
- Live NAS smoke was not run in this lane; the Docker compose config gate passed.

# Resident Proxy And Attach-First Operator Consoles — Handoff

Status: Closed
Last updated: 2026-05-20

## Current State

The resident runtime seam, resident CLI UX, GUI attach-first/proxy watchdog path, explicit TUI
attach path, supervisor crash-marker path, docs, and targeted verification are complete.

## Active Task

- Task ID: RPAF-080
- Owner: planner
- Files: `docs/workstreams/resident-proxy-attach-first`, final validation commands
- Validation: fresh verification and close/split follow-ons.
- Status: DONE
- Review: self-review completed; no blocking findings remain
- Evidence: core attach/resident runtime, CLI daemon, GUI attach-first support, TUI attached
  dashboard, and supervisor crash markers

## Decisions Since Last Update

- Keep full OS service installation out of scope for this lane.
- Keep legacy ephemeral `serve` behavior unless explicitly changed later.
- Prefer child-process supervisor/watchdog over in-process restart because allocator aborts cannot
  be caught reliably in-process.
- GUI-owned proxy now uses the shared runtime seam and can attach-first to an already-local helper
  port.
- Attached TUI is deliberately read-only in this lane. It uses admin snapshot/status APIs and makes
  `q`/Ctrl-C safe: console exit does not stop the resident proxy.
- Supervisor records last unexpected child exit as a lightweight JSON marker, but durable OS-level
  service installation remains a follow-on.

## Blockers

- None known.

## Next Recommended Action

- Commit the lane after user confirmation.
- If users still hit allocator aborts under whole-machine memory pressure, split an OS-service or
  external process-manager follow-on rather than trying to catch OOM inside the same process.

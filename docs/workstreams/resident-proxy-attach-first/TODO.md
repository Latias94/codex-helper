# Resident Proxy And Attach-First Operator Consoles — TODO

Status: Closed
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

- [x] RPAF-010 [owner=planner] [deps=none] [scope=docs/workstreams/resident-proxy-attach-first]
  Goal: Freeze the lifetime-mode design, task split, and evidence gates.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json agree.
  Evidence: docs/workstreams/resident-proxy-attach-first/DESIGN.md
  Handoff: Planner owns this before implementation tasks start.

## M1 — Resident Runtime Seam

- [x] RPAF-020 [owner=unassigned] [deps=RPAF-010] [scope=src/cli_app.rs,src/cli_types.rs,crates/core/src/proxy]
  Goal: Extract proxy/admin listener startup into a reusable runtime seam with explicit ephemeral vs resident lifetime policy.
  Validation: cargo nextest run -p codex-helper-core proxy::tests::api_admin; cargo check -p codex-helper
  Review: review-workstream before accepting completion.
  Evidence: runtime seam tests and CLI compile evidence.
  Handoff: Preserve current `codex-helper serve` behavior while adding resident internals.

- [x] RPAF-030 [owner=unassigned] [deps=RPAF-020] [scope=src/cli_types.rs,src/cli_app.rs,README.md,README_EN.md]
  Goal: Ship user-facing resident proxy CLI UX (`serve --resident` or equivalent) with clear status/stop semantics.
  Validation: cargo nextest run -p codex-helper listener_bind_help_tests; cargo check -p codex-helper
  Review: review-workstream before accepting completion.
  Evidence: CLI parser tests/docs.
  Handoff: Keep legacy ephemeral serve as the default unless explicitly changed in design notes.

## M2 — Attach-First Consoles

- [x] RPAF-040 [owner=unassigned] [deps=RPAF-030] [scope=crates/gui/src/gui/proxy_control,crates/gui/src/gui/app.rs,crates/gui/src/gui/config.rs]
  Goal: Make GUI startup prefer attach/resident start over in-process ownership while preserving manual integrated start where still useful.
  Validation: cargo nextest run -p codex-helper-gui; cargo check -p codex-helper-gui
  Review: review-workstream before accepting completion.
  Evidence: GUI proxy lifecycle tests.
  Handoff: UX must not require users to understand whether the target is new-api/sub2api or resident/integrated.

- [x] RPAF-050 [owner=codex] [deps=RPAF-030] [scope=crates/tui/src/tui,src/cli_app.rs,crates/core/src/proxy/control_plane*]
  Goal: Add a first TUI attach path for core observability and safe exit semantics.
  Validation: cargo nextest run -p codex-helper-tui --no-fail-fast; cargo check -p codex-helper
  Review: review-workstream before accepting completion.
  Evidence: `codex-helper tui --codex/--claude` attaches read-only through admin snapshot/runtime APIs; q/Ctrl-C exit only the console and never sends runtime shutdown.
  Handoff: Start with read-mostly observability if write actions need follow-up. Attached TUI intentionally avoids write controls for this lane.

## M3 — Supervisor / Watchdog

- [x] RPAF-060 [owner=codex] [deps=RPAF-030] [scope=src/cli_types.rs,src/cli_app.rs,crates/core/src/proxy]
  Goal: Add lightweight supervisor/watchdog that starts resident proxy as a child process, probes health, restarts with bounded backoff, and records crash markers.
  Validation: cargo nextest run -p codex-helper supervisor; cargo check -p codex-helper
  Review: review-workstream before accepting completion.
  Evidence: `daemon supervise` runs resident child with bounded exponential backoff and writes crash markers under `~/.codex-helper/run/`; supervisor backoff tests cover restart delay bounds.
  Handoff: Do not implement Windows Service/systemd in this task.

- [x] RPAF-070 [owner=codex] [deps=RPAF-060] [scope=crates/gui/src/gui,README.md,README_EN.md,docs/CONFIGURATION*.md]
  Goal: Surface resident/supervised status and recovery hints in GUI/docs without exposing secrets.
  Validation: cargo nextest run -p codex-helper-gui; cargo fmt --check
  Review: review-workstream before accepting completion.
  Evidence: GUI runtime summary now distinguishes shutdown API availability for attached mode; TUI Settings/header show integrated vs attached, shutdown availability, and safe-exit copy; docs cover resident/attach/supervise flows.
  Handoff: Keep messages understandable: Running / Attached / Restarting / Crashed.

## M4 — Verification And Closeout

- [x] RPAF-080 [owner=codex] [deps=RPAF-040,RPAF-050,RPAF-070] [scope=docs/workstreams/resident-proxy-attach-first]
  Goal: Run final verification, update evidence, and close or split follow-ons.
  Validation: cargo fmt --check; cargo check --workspace; targeted nextest for core admin, TUI, GUI, and CLI resident/supervisor parser/backoff tests.
  Review: self-review against DESIGN/TODO/EVIDENCE found no blocking workstream or code-quality findings; durable OS-service installation remains an explicit follow-on.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json
  Handoff: Deferred OS-service installation, richer attached-TUI mutations, and full workspace nextest remain optional follow-ons.

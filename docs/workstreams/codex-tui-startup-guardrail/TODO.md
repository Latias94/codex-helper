# Task Ledger

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- [x] Keep the scope to interactive TUI startup only.
- [x] Reuse existing Codex diagnostics rather than duplicating file parsing in the UI.
- [x] Make the alert dismissible.
- [x] Keep noninteractive CLI paths quiet.

## Open Questions

- [x] Should the first implementation use a modal, a banner, or a toast-first fallback?
  - Accepted: use a startup modal for actionable warnings; keep the footer
    close hint visible.
- [x] Should the alert snooze for the current session after dismissal?
  - Accepted: dismissal clears the report for the current TUI session only.
- [x] Which diagnostics are severe enough to justify a modal instead of a passive toast?
  - Accepted: client config/auth changed on startup, switch failure, local proxy
    mismatch, missing switch state, remote-control config/db/log warnings, and
    diagnostic read failures.

## WS0 - Baseline And Probe Shape

- [x] STG-000 Map the current direct-start entry points and the diagnostics they already expose.
  - `run_server` is the direct interactive entry point; it already applies the
    Codex client patch before spawning `tui::run_dashboard`.
- [x] STG-001 Define the startup-readiness data shape and the conditions that should trigger it.
  - Added `CodexStartupReadiness`, issue kinds, severity, and input shape in
    `codex_integration`.
- [x] STG-002 Decide the alert presentation contract for wide, narrow, and noninteractive terminals.
  - First slice uses an interactive TUI modal. Noninteractive paths do not call
    the TUI startup surface.

## WS1 - Core Probe And TUI Surface

- [x] STG-010 Add a core readiness probe that reuses Codex switch/remote-control diagnostics.
  - `codex_tui_startup_readiness` reuses switch status, remote-control status,
    and remote-control log scan helpers.
- [x] STG-020 Add a TUI startup alert surface and dismissal behavior.
  - Added `Overlay::StartupAlert`, modal rendering, footer hint, and
    Enter/Esc dismissal.
- [x] STG-030 Wire the probe into interactive `serve` startup.
  - `run_server` detects Codex config/auth changes during switch-on and passes
    the readiness report into `run_dashboard`.

## WS2 - Tests And Polish

- [x] STG-040 Add tests for ready, warning, and quiet startup cases.
  - Core tests cover quiet ready state, client state changed, missing switch
    state, and remote-control log unconfirmed.
  - TUI tests cover modal rendering and Enter dismissal.
- [x] STG-041 Add narrow-terminal coverage for the alert copy and dismissal path.
  - Added a 64-column render test that keeps the startup guardrail title, core
    warning copy, and close hint visible.
- [x] STG-050 Add a manual smoke checklist for direct TUI startup.
  - Added `SMOKE.md` covering ready state, client config changes,
    remote-control follow-up, and narrow terminal verification.

## Candidate First Slice

Recommended first implementation goal:

1. define the readiness probe and trigger rules;
2. add a TUI alert surface with dismiss support;
3. wire the alert into interactive startup;
4. add tests for ready and warning cases;
5. smoke test the direct-start flow.

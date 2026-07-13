# Handoff

> Historical ownership note (updated 2026-07-12): startup readiness no longer reads Codex auth, model cache, SQLite, or remote-control state. The retained guardrail reports helper config/runtime readiness and explicit local switch-journal conflicts; the implementation inventory below records the earlier broader probe.

Historical state: the startup guardrail implementation and documented smoke checklist were in place for this workstream.

Implemented:

- `CodexStartupReadiness` and `codex_tui_startup_readiness` in core.
- Interactive TUI startup modal through `Overlay::StartupAlert`.
- Enter/Esc dismissal that clears the alert for the current TUI session.
- `run_server` wiring that detects Codex config/auth changes during switch-on.
- Focused core and TUI tests for ready/warning/dismissal behavior.
- Narrow-width render coverage for the startup alert.
- `SMOKE.md` for real-terminal direct-start verification.

Remaining next steps:

- none required for this slice. Manual direct-start smoke is documented in
  `SMOKE.md` and was explicitly skipped before commit.

Known follow-ons that stay out of scope here:

- capability matrix work;
- response fixer / protocol normalization;
- GUI wizard UX;
- provider hard-stop semantics.

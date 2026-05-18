# TODO

- [x] COIB-010 [owner=main] [deps=none] [scope=docs/workstreams/codex-official-imagegen-bridge]
  Record Codex source findings and mixed-mode target behavior.
  Validation: docs exist with concrete source-level conditions.
  Handoff: `DESIGN.md` records why the mode is feasible.

- [x] COIB-020 [owner=main] [deps=COIB-010] [scope=crates/core/src/codex_integration.rs,src/*,crates/core/src/config_storage.rs]
  Add `official-imagegen-bridge` to patch mode parsing, config generation, auth patch handling,
  status inference, CLI output, and auth stripping.
  Validation: targeted core tests compile and pass.
  Handoff: Added the mode across core, CLI args, config parsing, state inference, and auth stripping.

- [x] COIB-030 [owner=main] [deps=COIB-020] [scope=crates/gui,crates/tui,README*,docs/CONFIGURATION*]
  Expose and document the new mode in operator surfaces.
  Validation: text references include when to choose the hybrid mode and how it differs from the
  existing bridge modes.
  Handoff: CLI, GUI setup buttons, TUI setting hotkey `V`, configuration docs, README, and changelog mention the hybrid mode.

- [x] COIB-040 [owner=main] [deps=COIB-020,COIB-030] [scope=workspace]
  Run formatting and focused verification.
  Validation: `cargo fmt --check` and targeted `cargo nextest` commands pass.
  Handoff: Evidence recorded in `EVIDENCE_AND_GATES.md`.

# TODO

- [x] RCV2LS-010 [owner=main] [deps=none] [scope=crates/core/src/proxy/codex_relay_live_smoke.rs,src/cli_types.rs,src/commands/codex.rs,docs/CONFIGURATION*.md]
  Goal: Add an explicit `remote_compaction_v2` live-smoke case that proves the Codex v2 compaction
  streaming shape.
  Validation: DONE. `cargo nextest run -p codex-helper-core --no-fail-fast`;
  `cargo nextest run -p codex-helper live_smoke_cases codex_relay_cli_parses_live_smoke --no-fail-fast`;
  `cargo nextest run -p codex-helper-tui codex_relay_live_smoke codex_relay_live_smoke_lines_show_confirmation_and_results --no-fail-fast`;
  `cargo fmt --all --check`.
  Evidence: `EVIDENCE_AND_GATES.md`.
  Handoff: DONE. The case remains explicit-only and acknowledgement-gated.

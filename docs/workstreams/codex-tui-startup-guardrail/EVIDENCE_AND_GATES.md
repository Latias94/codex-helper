# Evidence And Gates

> Historical artifact (superseded 2026-07-12): startup readiness no longer reads
> Codex auth, model cache, SQLite, or remote-control state. The retained guardrail
> reports helper config/runtime readiness and explicit local switch-journal
> conflicts. The remote control plane is GET/HEAD-only.

## Planned Gates

- `cargo fmt --all --check`
- `cargo nextest run -p codex-helper-tui`
- targeted core tests for the startup-readiness probe, once added
- manual interactive TUI smoke on direct startup

## Evidence Log

- 2026-05-18: workstream created from the operator feedback that direct TUI starts are the common path, so the verification step should be surfaced in the TUI instead of relying on a separate manual command.
- 2026-05-18: `cargo nextest run -p codex-helper-core codex_tui_startup_readiness` passed: 4 tests run, 4 passed.
- 2026-05-18: `cargo nextest run -p codex-helper-tui startup_alert` passed: 2 tests run, 2 passed.
- 2026-05-18: `cargo check --bins` passed after wiring startup readiness into `src/cli_app.rs`.
- 2026-05-18: added `SMOKE.md` for manual direct-start and remote-control startup guardrail checks. Manual smoke still requires a real interactive terminal.
- 2026-05-18: `cargo nextest run -p codex-helper-tui startup_alert` passed after adding narrow-width coverage: 3 tests run, 3 passed.
- 2026-05-18: `cargo fmt --all --check` passed.
- 2026-05-18: `cargo clippy --workspace --all-targets -- -D warnings` passed. Manual TTY smoke was explicitly skipped by operator decision.
- 2026-05-18: while preparing the same commit, adjusted the TUI Providers page KPI layout so the `Usage / Balance` card gets more width and uses a brief refresh/error summary; full latest balance errors remain in provider detail.
- 2026-05-18: `cargo nextest run -p codex-helper-tui stats_` passed after the Providers page layout change: 5 tests run, 5 passed.
- 2026-05-18: `cargo nextest run -p codex-helper-tui startup_alert` passed after the Providers page layout change: 3 tests run, 3 passed.
- 2026-05-18: reran `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`; both passed after the Providers page layout change.

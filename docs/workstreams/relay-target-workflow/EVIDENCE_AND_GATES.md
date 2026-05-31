# Relay Target Workflow - Evidence And Gates

Status: Complete
Last updated: 2026-05-31

## Evidence Log

| Date | Task | Evidence | Result |
| --- | --- | --- | --- |
| 2026-05-31 | RTW-010 | Workstream docs created and aligned. | PASS. |
| 2026-05-31 | RTW-020 | `cargo nextest run --locked -p codex-helper-core relay_target control_plane --no-fail-fast` | PASS. 11 focused tests passed. |
| 2026-05-31 | RTW-020 | `cargo nextest run --locked -p codex-helper-core v2_schema v4_schema --no-fail-fast` | PASS. 44 config schema and migration tests passed. |
| 2026-05-31 | RTW-030 | `cargo nextest run --locked -p codex-helper relay_cli --no-fail-fast`; `cargo check --locked -p codex-helper` | PASS. 10 focused CLI tests passed; CLI package compiled. |
| 2026-05-31 | RTW-040 | `cargo nextest run --locked -p codex-helper-tui attached --no-fail-fast`; `cargo check --locked -p codex-helper-tui` | PASS. 4 focused TUI tests passed; TUI package compiled. |
| 2026-05-31 | RTW-050 | `cargo fmt --all -- --check`; `cargo check --locked -p codex-helper-core`; `cargo check --locked -p codex-helper-tui`; `cargo check --locked -p codex-helper`; `git diff --check` | PASS. Formatting, compile checks, and whitespace checks are clean. |
| 2026-05-31 | RTW-050 | `docker compose --env-file deploy/compose/.env.example -f deploy/compose/codex-helper.yml config --quiet` | PASS. Compose still resolves after deployment doc updates. |

## Planned Gates

- `cargo fmt --all -- --check`
- `cargo check --locked -p codex-helper`
- `cargo check --locked -p codex-helper-core`
- `cargo check --locked -p codex-helper-tui`
- `cargo nextest run --locked -p codex-helper-core relay_target control_plane --no-fail-fast`
- `cargo nextest run --locked -p codex-helper relay_cli --no-fail-fast`
- `cargo nextest run --locked -p codex-helper-tui attached --no-fail-fast`

## Caveats

- Full GUI redesign is outside this lane. GUI code may be touched only when extracting shared control-plane logic is low risk.
- Live NAS smoke requires a reachable deployment and is optional unless the user starts the container during this lane.

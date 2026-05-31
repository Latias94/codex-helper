# Codex Server Container Deployment - Evidence And Gates

Status: Complete
Last updated: 2026-05-31

## Evidence Log

| Date | Task | Evidence | Result |
| --- | --- | --- | --- |
| 2026-05-31 | CSC-010 | Workstream and ADR created. | PASS |
| 2026-05-31 | CSC-020 | `cargo check --locked -p codex-helper-server`; `cargo run --locked -p codex-helper-server -- --help`; `cargo nextest run --locked -p codex-helper-core host_local admin_discovery proxy_api_v1_capabilities_and_overrides_work --no-fail-fast` | PASS. New server crate compiles, CLI is exposed, explicit admin discovery behavior and host-local capability seam are covered. |
| 2026-05-31 | CSC-030 | `cargo test --locked -p codex-helper-server --no-fail-fast`; `cargo nextest run --locked -p codex-helper-core host_local admin_discovery proxy_api_v1_capabilities_and_overrides_work --no-fail-fast` | PASS. `server.toml` parsing and disabled-by-default server host-local policy are covered without changing local CLI auto-detection. |
| 2026-05-31 | CSC-040 | `docker compose --env-file deploy/compose/.env.example -f deploy/compose/codex-helper.yml config --quiet` | PASS. Compose file resolves with sanitized env values. |
| 2026-05-31 | CSC-040 | `docker build --target runtime -t codex-helper:local .` | SKIPPED. Docker CLI is installed, but the Docker Desktop Linux daemon is not running: `failed to connect to the docker API at npipe:////./pipe/dockerDesktopLinuxEngine`. |
| 2026-05-31 | CSC-050 | `actionlint -version`; `go version`; Python/PyYAML parse of `.github/workflows/*.yml`; workflow action source audit | PARTIAL PASS. `actionlint` and Go are unavailable locally; all workflow YAML files parse, and action majors were updated from GitHub release-page audit. |
| 2026-05-31 | CSC-060 | `cargo fmt --all -- --check`; workstream review pass over task ledger, scope, docs, and evidence | PASS. Formatting is clean after implementation; no blocking workstream review findings remain. |

## Planned Gates

### Rust

- `cargo nextest run --locked -p codex-helper-core host_local admin_discovery proxy_api_v1_capabilities_and_overrides_work --no-fail-fast`
- `cargo test --locked -p codex-helper-server --no-fail-fast`
- `cargo check --locked -p codex-helper-server`
- `cargo fmt --all -- --check`

### Docker

- `docker build --target runtime -t codex-helper:local .`
- `docker run --rm codex-helper:local --help`
- `docker compose -f deploy/compose/codex-helper.yml config`

### CI

- `actionlint .github/workflows/*.yml` when available.
- YAML parse fallback when `actionlint` is unavailable.
- GitHub Releases API audit for action latest majors.

## Known Environment Caveats

- Docker CLI and Compose are installed, but the Docker Desktop Linux daemon was unavailable on 2026-05-31. Compose syntax was verified locally; image build must be verified in CI or after starting the daemon.
- Release publishing cannot be tested end-to-end locally; workflow validation and smoke command design are the local gates.
- `actionlint` and Go are not installed locally, so workflow validation used PyYAML parsing and action version/source audit as the fallback.

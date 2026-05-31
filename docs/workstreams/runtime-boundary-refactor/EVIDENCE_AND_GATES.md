# Runtime Boundary Refactor - Evidence And Gates

Status: Complete
Last updated: 2026-05-31

## Evidence Log

| Date | Task | Evidence | Result |
| --- | --- | --- | --- |
| 2026-05-31 | RBR-010 | `docker build --target runtime -t codex-helper:local .` | PASS after Dockerfile repair. |
| 2026-05-31 | RBR-010 | `docker run --rm codex-helper:local --help` | PASS after entrypoint repair. |
| 2026-05-31 | RBR-010 | `docker compose --env-file deploy/compose/.env.example -f deploy/compose/codex-helper.yml config --quiet` | PASS with sanitized env values. |
| 2026-05-31 | RBR-020/RBR-030 | `cargo nextest run --locked -p codex-helper-core host_local admin_discovery proxy_api_v1_capabilities_and_overrides_work --no-fail-fast` | PASS. 7 tests passed, covering advertised admin discovery, host-local policy helpers, and capability response compatibility. |
| 2026-05-31 | RBR-040 | `cargo test --locked -p codex-helper-server --no-fail-fast` | PASS. 3 tests passed for server config parsing, effective config merge, URL normalization, and remote admin token validation. |
| 2026-05-31 | RBR-050 | `cargo check --locked -p codex-helper` | PASS. Local CLI refactor compiles without behavior-facing API changes. |
| 2026-05-31 | RBR-060 | `cargo fmt --all -- --check`; `cargo check --locked -p codex-helper-server`; `git diff --check` | PASS. Formatting, server compile, and whitespace checks are clean. |
| 2026-05-31 | RBR-060 | `docker build --target runtime -t codex-helper:local .`; `docker run --rm codex-helper:local --help`; `docker compose --env-file deploy/compose/.env.example -f deploy/compose/codex-helper.yml config --quiet` | PASS. Final Docker image build and smoke gates pass. |

## Planned Gates

- `cargo fmt --all -- --check`
- `cargo check --locked -p codex-helper`
- `cargo check --locked -p codex-helper-server`
- `cargo test --locked -p codex-helper-server --no-fail-fast`
- `cargo nextest run --locked -p codex-helper-core host_local admin_discovery proxy_api_v1_capabilities_and_overrides_work --no-fail-fast`
- `docker build --target runtime -t codex-helper:local .`
- `docker run --rm codex-helper:local --help`
- `docker compose --env-file deploy/compose/.env.example -f deploy/compose/codex-helper.yml config --quiet`

## Caveats

- Full workspace GUI checks remain outside this lane unless the local CLI split touches GUI-owned surfaces.

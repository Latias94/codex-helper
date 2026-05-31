# Runtime Boundary Refactor - TODO

Status: Complete
Last updated: 2026-05-31

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Tasks

- [x] RBR-010 [scope=Dockerfile,deploy/compose]
  Goal: Repair and verify Docker build/smoke now that the daemon is available.
  Evidence: EVIDENCE_AND_GATES.md.
  Notes: Fixed cargo-chef recipe generation and image entrypoint behavior.

- [x] RBR-020 [scope=crates/core/src/runtime_host.rs,crates/server]
  Goal: Replace runtime_host parameter growth with runtime options and add advertised admin URL support.
  Validation: `cargo nextest run --locked -p codex-helper-core admin_discovery runtime_options --no-fail-fast`.
  Evidence: EVIDENCE_AND_GATES.md.

- [x] RBR-030 [scope=crates/core/src/host_local.rs,crates/core/src/proxy]
  Goal: Make host-local capability policy runtime-local instead of process-global.
  Validation: `cargo nextest run --locked -p codex-helper-core host_local capabilities --no-fail-fast`.
  Evidence: EVIDENCE_AND_GATES.md.

- [x] RBR-040 [scope=crates/server/src/config.rs,crates/server/src/main.rs,deploy/container/server.toml]
  Goal: Move CLI/file merge into an effective server config type and validate admin exposure.
  Validation: `cargo test --locked -p codex-helper-server --no-fail-fast`.
  Evidence: EVIDENCE_AND_GATES.md.

- [x] RBR-050 [scope=src/cli_app.rs]
  Goal: Split local `run_server` orchestration into smaller lifecycle steps without changing behavior.
  Validation: `cargo check --locked -p codex-helper`.
  Evidence: EVIDENCE_AND_GATES.md.

- [x] RBR-060 [scope=docs/workstreams/runtime-boundary-refactor]
  Goal: Run final gates, update evidence, review, commit, and close the workstream.
  Validation: `cargo fmt --all -- --check`; targeted Rust and Docker gates pass.
  Evidence: EVIDENCE_AND_GATES.md.

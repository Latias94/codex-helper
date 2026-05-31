# Codex Server Container Deployment - TODO

Status: Complete
Last updated: 2026-05-31

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- Server/container runtime does not patch local Codex client config by default.
- Central relay mode shares observed control-plane data, not client-local transcript files.
- Docker release images target GHCR and release-tagged versions first.

## Open Questions

- `[x]` Should the first server runtime be a new crate or a new binary in the root package? Chosen: new `crates/server` package, so the container entrypoint does not pull local CLI/TUI lifecycle behavior.
- `[x]` Should proxy traffic have its own access token, or should deployment rely on LAN/Tailscale/firewall policy for the first release? Chosen: no proxy token in this slice; admin API keeps token enforcement, proxy exposure remains a deployment network policy.
- `[x]` Should Docker publish run as a separate tag workflow or as a job attached to the cargo-dist release workflow? Chosen: separate `.github/workflows/docker-publish.yml`, while cargo-dist release assets stay unchanged except action major updates.

## M0 - Scope And Evidence Freeze

- [x] CSC-010 [owner=planner] [deps=none] [scope=docs/adr,docs/workstreams/codex-server-container-deployment]
  Goal: Freeze server/container problem statement, ADR, target state, and task ledger.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and CONTEXT.jsonl exist and agree.
  Evidence: docs/workstreams/codex-server-container-deployment/DESIGN.md
  Context: docs/workstreams/codex-server-container-deployment/CONTEXT.jsonl
  Handoff: DONE in this planning pass.

## M1 - Server Runtime

- [x] CSC-020 [owner=codex] [deps=CSC-010] [scope=Cargo.toml,crates/server,crates/core/src/runtime_host.rs,crates/core/src/proxy/admin.rs]
  Goal: Add a server/container runtime path that starts the Codex proxy without local client patching and with explicit proxy/admin bind behavior.
  Validation: cargo nextest run --locked -p codex-helper-core server_runtime admin
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Context: CONTEXT.jsonl plus ADR-0001.
  Handoff: DONE. Implemented as `codex-helper-server` in `crates/server`; existing local `serve` behavior remains on the root CLI path.

- [x] CSC-030 [owner=codex] [deps=CSC-020] [scope=crates/server/src/config.rs,crates/core/src/host_local.rs,crates/core/src/proxy/control_plane.rs,crates/core/src/proxy/control_plane/capabilities.rs]
  Goal: Add explicit server deployment configuration and host-local capability policy.
  Validation: cargo nextest run --locked -p codex-helper-core capabilities session
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Context: CONTEXT.jsonl plus CENTRAL_RELAY.md.
  Handoff: DONE. No migration required; server config is a new deployment file and route graph config stays in `CODEX_HELPER_HOME/config.toml`.

## M2 - Container Assets

- [x] CSC-040 [owner=codex] [deps=CSC-020,CSC-030] [scope=Dockerfile,.dockerignore,deploy/compose,deploy/container,docs]
  Goal: Add cargo-chef based Dockerfile, Synology-friendly Compose sample, env sample, container config sample, and local healthcheck path.
  Validation: docker build --target runtime -t codex-helper:local .; docker compose -f deploy/compose/codex-helper.yml config
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Context: CONTEXT.jsonl plus repo-ref/nako Docker/Compose files.
  Handoff: DONE with caveat. Compose config validates; image build could not run locally because Docker Desktop Linux daemon is unavailable.

## M3 - Release And CI

- [x] CSC-050 [owner=codex] [deps=CSC-040] [scope=.github/workflows,dist-workspace.toml,docs]
  Goal: Add Docker publish CI for GHCR release images and update impacted GitHub Actions majors.
  Validation: actionlint .github/workflows/*.yml if available; otherwise YAML parse plus workflow source audit.
  Review: review-workstream before accepting completion.
  Evidence: EVIDENCE_AND_GATES.md
  Context: CONTEXT.jsonl plus GitHub Releases API version audit.
  Handoff: DONE. Uses checkout v6, upload-artifact v7, download-artifact v8, setup-qemu v4, setup-buildx v4, login v4, metadata v6, build-push v7.

## M4 - Verification And Closeout

- [x] CSC-060 [owner=planner] [deps=CSC-020,CSC-030,CSC-040,CSC-050] [scope=docs/workstreams/codex-server-container-deployment]
  Goal: Run final targeted gates, update evidence, and either close the lane or split follow-ons.
  Validation: verify-rust-workstream records fresh final gate evidence.
  Review: review-workstream has no blocking findings.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json
  Handoff: DONE. Residual risk is Docker image build not executed on this Windows host because the Docker Linux daemon is not running; CI workflow will run the build.

## Closeout

Implementation and targeted verification are complete. The only skipped local gate is Docker image build because the Docker Desktop Linux daemon is not running in this workspace.

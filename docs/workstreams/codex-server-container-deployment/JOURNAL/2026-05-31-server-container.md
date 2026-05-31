# 2026-05-31 - Server Container Deployment

Implemented the container-first runtime as a new `codex-helper-server` crate. This keeps the existing local `codex-helper serve` lifecycle intact while giving Docker a small entrypoint that starts proxy/admin listeners without patching local Codex client files.

Core changes:

- added explicit admin bind support in `runtime_host`;
- added host-local session history policy in `host_local`;
- connected control-plane capability reporting to the policy seam;
- added `server.toml` parsing with CLI override behavior.

Deployment changes:

- added cargo-chef Dockerfile and `.dockerignore`;
- added Synology-friendly Compose stack, `.env.example`, `server.toml`, and initial route graph config;
- documented NAS client configuration in `docs/DOCKER_COMPOSE.md`;
- added GHCR Docker publish workflow and updated cargo-dist release artifact actions.

Verification notes:

- Rust formatting, server crate check/tests, targeted core nextest gates, Compose config, and workflow YAML parsing passed.
- Docker image build was skipped locally because Docker Desktop's Linux daemon was not running.

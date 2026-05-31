# Codex Server Container Deployment - Handoff

Status: Complete
Last updated: 2026-05-31

## Current State

The container deployment lane is implemented. ADR-0001 records the central relay runtime decision. `crates/server` provides the container-oriented `codex-helper-server` entrypoint, Docker/Compose assets exist, and Docker publish CI is present.

## Next Task

Before merging, run a Docker image build on a host with a running Linux Docker daemon or let `.github/workflows/docker-publish.yml` validate it in CI.

## Required Context

Read `CONTEXT.jsonl`, `DESIGN.md`, `TODO.md`, and ADR-0001 before editing code.

## Notes

- Do not rewrite the existing local `serve` behavior unless CSC-020 proves a shared helper is needed.
- Preserve user or parallel-agent changes.
- Fresh local evidence is recorded in `EVIDENCE_AND_GATES.md`.
- Local Docker image build was skipped because Docker Desktop's Linux daemon was not running.

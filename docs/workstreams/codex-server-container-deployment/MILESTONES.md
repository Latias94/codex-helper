# Codex Server Container Deployment - Milestones

Status: Complete
Last updated: 2026-05-31

## M0 - Planning Ready

Exit criteria:

- ADR-0001 exists.
- Workstream docs agree on target state and non-goals.
- Task ledger has independently validatable slices.

Status: done.

## M1 - Server Runtime Ready

Exit criteria:

- A server/container entrypoint starts without local Codex client patching.
- Admin/proxy bind behavior is explicit and test-covered.
- Host-local capability defaults are not inferred accidentally in container mode.

Status: done.

## M2 - Container Runtime Usable

Exit criteria:

- Dockerfile builds a server runtime image.
- Compose sample validates and documents required env/volumes.
- A healthcheck proves the container process is reachable.
- Synology docs explain how each client points Codex at the central relay.

Status: done with local build caveat. Compose validates locally; Docker image build is deferred to CI or a host with a running Docker Linux daemon.

## M3 - Release Images

Exit criteria:

- GHCR workflow builds multi-arch images from release tags.
- Image labels include source, revision, created timestamp, and version.
- Smoke checks run before push.
- Current action major versions are used or intentionally pinned with evidence.

Status: done. Docker publish workflow is separate from cargo-dist release artifacts and uses current major action pins.

## M4 - Closeout

Exit criteria:

- Targeted Rust tests pass.
- Docker/Compose gates are recorded or explicitly skipped with environment reason.
- Docs and release workflow are consistent.
- Follow-on scope, if any, is split out.

Status: done with caveat. No blocking follow-on is required for Synology Compose support; optional future work is proxy authentication or a remote companion for client-local transcripts.

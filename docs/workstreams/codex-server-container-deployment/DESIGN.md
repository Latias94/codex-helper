# Codex Server Container Deployment

Status: Complete
Last updated: 2026-05-31

## Why This Lane Exists

`codex-helper` already supports a strong local proxy and a documented central relay product shape, but it does not yet have a container-first runtime for Synology or Docker Compose. The existing `serve` command still carries local-client behavior such as Codex config patching, TUI lifecycle assumptions, and loopback-only admin binding.

## Relevant Authority

- ADRs:
  - `docs/adr/0001-central-relay-container-runtime.md`
- Existing docs:
  - `docs/workstreams/codex-control-plane-refactor/CENTRAL_RELAY.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
- Related workstreams:
  - `docs/workstreams/codex-control-plane-refactor`
  - `docs/workstreams/desktop-lifecycle-owner`
- Reference repos:
  - `repo-ref/nako/Dockerfile`
  - `repo-ref/nako/deploy/compose`
  - `repo-ref/nako/.github/workflows/docker-publish.yml`
  - `repo-ref/cargo-chef/docker`

## Problem

Running `codex-helper serve --host 0.0.0.0 --resident` in a container is mechanically possible but semantically wrong:

- it still attempts to switch the local Codex client config inside the container;
- admin/control-plane exposure is not modeled for Docker networking;
- host-local session history can be inferred from the wrong filesystem;
- the root binary pulls TUI/GUI-oriented dependencies that the container runtime does not need;
- there is no Dockerfile, Compose sample, healthcheck, or GHCR publishing workflow.

## Target State

- A server/container runtime starts a Codex relay without local client patch side effects.
- Server deployment configuration owns bind addresses, admin exposure, data directories, and host-local capability policy.
- Central relay APIs remain honest: observed sessions and request history are shared, while transcript/cwd/session-file enrichment is opt-in host-local behavior.
- Docker assets build a small server image with cargo-chef caching and Synology-friendly Compose examples.
- CI publishes multi-arch GHCR images from release tags with current GitHub Actions majors and smoke checks.

## In Scope

- Add or reshape a server-only binary/crate if that gives a smaller, deeper interface than extending the current CLI.
- Add container/server config loading where needed.
- Gate host-local capabilities explicitly for server mode.
- Add Dockerfile, `.dockerignore`, Compose samples, `.env.example`, and deployment docs.
- Add Docker publish CI and update affected action majors.
- Add focused tests and command evidence.

## Out Of Scope

- Building a web UI.
- Implementing a remote companion that uploads client-local transcripts.
- Public internet exposure hardening beyond token/exposure-mode guardrails.
- Replacing the existing local CLI/TUI/desktop lifecycle.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Synology users need amd64 or arm64 Linux images. | High | Synology Docker deployments commonly run on these platforms. | CI platform matrix may need expansion. |
| Central relay mode should not patch client config on the relay host. | High | `CENTRAL_RELAY.md` defines shared relay semantics; ADR-0001 records the decision. | Container startup could mutate meaningless or mounted client config. |
| Observed session control remains valid without host-local transcript access. | High | Session identity and request history are derived from proxy traffic. | Host-local gating would remove too much functionality. |
| cargo-chef is appropriate for dependency caching. | High | `repo-ref/nako/Dockerfile` uses the same pattern successfully. | Docker builds may be slower but still correct. |

## Architecture Direction

Create a deep server runtime module with a small interface: "load deployment config, build proxy runtime, expose proxy/admin surfaces, report capability truth." Keep existing local CLI behavior behind the current `serve` path. The server runtime should reuse `codex-helper-core` for routing, state, admin APIs, and request execution, but avoid depending on TUI/GUI ownership semantics.

Host-local enrichment must be an explicit adapter at a seam: disabled in container mode by default, enabled only when the runtime has a deliberate local sessions mount. This keeps locality around capability truth and prevents remote clients from depending on files that live on another device.

Docker publishing should be a release-adjacent lane: release tags produce OCI images with semver tags and smoke evidence, without modifying cargo-dist artifacts.

## Closeout Condition

This lane can close when:

- server/container runtime behavior is implemented and tested;
- host-local capability reporting is explicit in server mode;
- Docker/Compose assets can build and start the relay locally;
- CI can publish GHCR images from release tags;
- documentation explains Synology deployment and client configuration;
- final verification evidence is recorded.

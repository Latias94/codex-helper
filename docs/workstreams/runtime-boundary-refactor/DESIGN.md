# Runtime Boundary Refactor

Status: Complete
Last updated: 2026-05-31

## Why This Lane Exists

The server container lane created a clean `codex-helper-server` entrypoint, but it also exposed the next architecture boundary: proxy runtime construction still uses widening function signatures, host-local capability policy is process-global, and the local CLI `run_server` function still owns too many lifecycle concerns in one place.

## Target State

- `runtime_host` accepts a compact options/config object instead of adding more positional parameters.
- Admin discovery distinguishes bind address from advertised URL, so Docker deployments can publish a usable admin URL without lying about `0.0.0.0`.
- Host-local capability policy is runtime-local and attached to the proxy/control-plane context, not a process-global switch.
- `codex-helper-server` resolves an effective config with validation before starting the runtime.
- The local CLI server path keeps local client patching/TUI behavior, but the orchestration is split into named steps with smaller interfaces.

## Non-goals

- Replacing existing local CLI behavior.
- Adding public internet proxy authentication in this lane.
- Building a web UI or remote transcript companion.
- Changing route graph semantics.

## Refactor Brief

Intent: remove accidental coupling between local desktop lifecycle, server/container runtime, and capability truth before more deployment features are layered on.

Scope: `crates/core/src/runtime_host.rs`, `crates/core/src/host_local.rs`, proxy control-plane capability plumbing, `crates/server/src/config.rs`, `crates/server/src/main.rs`, and the local `src/cli_app.rs` server orchestration.

Deletion plan: remove runtime_host overload growth, remove process-global host-local mode as the primary capability source, and move repeated config merge logic out of `main.rs`.

Boundary plan: introduce runtime options and runtime-local control-plane policy; keep local client patching owned by root CLI, not core or server crate.

Testing plan: focused core tests for runtime options/admin discovery/host-local policy, server config tests, local CLI compile checks, Docker build/smoke, and targeted nextest gates.

Risk plan: preserve default local CLI behavior; keep container defaults conservative; treat Docker and compose gates as required before closeout.

Workflow plan: durable workstream with bounded tasks.

Scale plan: workstream, not a one-off direct edit.

# Relay Target Workflow

Status: Active
Last updated: 2026-05-31

## Why This Lane Exists

Container server support made a NAS-hosted codex-helper relay practical, but the local daily UX still assumes that the operator runs a local proxy. The `ch` shortcut starts the local flow, `switch on` patches Codex to a local proxy, and attached TUI code hard-codes loopback admin access.

Remote relay use should not be a collection of advanced flags. Users should be able to choose a named relay target such as `local` or `nas`, have Codex point to the target proxy, and inspect the same runtime through TUI without understanding admin port derivation.

## Relevant Authority

- ADRs:
  - `docs/adr/0001-central-relay-container-runtime.md`
- Existing docs:
  - `README.md`
  - `README_EN.md`
  - `docs/DOCKER_COMPOSE.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
- Related workstreams:
  - `docs/workstreams/codex-server-container-deployment/DESIGN.md`
  - `docs/workstreams/runtime-boundary-refactor/DESIGN.md`
  - `docs/workstreams/resident-proxy-attach-first/DESIGN.md`
  - `docs/workstreams/tauri-desktop-client/HANDOFF.md`

## Problem

The product has two runtime shapes now, local desktop proxy and remote central relay, but the CLI/TUI model still treats local loopback as the default truth. This makes Synology usage awkward, duplicates admin client behavior across UI surfaces, and keeps `switch on` as the only obvious way to connect Codex to a proxy.

## Target State

- `ch` remains the existing local foreground shortcut.
- `ch relay local` and `ch relay <name>` become the day-to-day target selection flow.
- Relay targets are stored by name, with proxy URL, optional admin URL, service, client preset, WebSocket option, and admin token env var.
- The built-in `local` target keeps local behavior without requiring config.
- A remote target can be added from a proxy URL and can discover the advertised admin URL.
- `ch relay <name>` can switch Codex to the target proxy and attach TUI to the target admin API.
- `ch relay <name> --no-tui` switches only; `--attach-only` observes only.
- TUI attached mode accepts an admin base URL and no longer assumes `127.0.0.1`.
- Shared admin/control-plane request logic is centralized instead of repeated in CLI, TUI, and GUI.
- Remote attached mode keeps host-local transcript/session-file behavior disabled or clearly gated.

## In Scope

- Relay target config schema and load/save helpers.
- `RelayTargetResolver` and shared `ControlPlaneClient`.
- `relay` CLI command family for add, list, status, off, and target use.
- Codex client patching for remote proxy targets.
- Attached TUI refactor from local port to target admin URL.
- Documentation and focused tests.

## Out Of Scope

- Public internet exposure support beyond existing admin token policy.
- Storing admin tokens directly in config files.
- A remote transcript companion/uploader.
- Replacing existing provider/routing graph configuration.
- Removing `switch on`, `serve`, or the current `ch` shortcut.
- Full GUI remote target redesign in this lane; GUI should benefit from shared primitives where low risk.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| `ch` is the same CLI as `codex-helper`. | High | `src/bin/ch.rs` calls `codex_helper::run_cli()`. | Relay UX must be implemented in the main CLI parser, not in a separate binary. |
| Existing users expect plain `ch` to preserve local foreground behavior. | High | README documents `codex-helper serve` and current default CLI flow. | Changing default `ch` behavior would be a breaking UX change. |
| Remote admin calls can use the existing admin token env/header policy. | High | `crates/core/src/proxy/admin.rs` and runtime-boundary-refactor tests. | A new auth model or ADR would be required. |
| TUI can render most useful remote state from admin API responses. | Medium | `crates/tui/src/tui/attached.rs` already reads runtime status, snapshot, profiles, and routing. | Additional admin endpoints may be needed before closeout. |
| GUI attached discovery has reusable logic. | High | `crates/gui/src/gui/proxy_control/attached_discovery.rs`. | Duplicating TUI behavior would increase drift. |

## Architecture Direction

Introduce a target-first control layer:

- `RelayTarget` describes what the user wants to use.
- `RelayTargetResolver` resolves built-in local targets, named config targets, and optional discovery.
- `ControlPlaneClient` owns admin base URL, token header injection, JSON request helpers, discovery, capabilities, runtime status, and snapshot calls.
- CLI commands operate on target intent: switch-only, attach-only, or switch-and-attach.
- TUI consumes an already resolved attached runtime target instead of reconstructing local admin ports.

This keeps the container server runtime separate from local client patching, while giving users one daily verb for both local and remote relay use.

## Closeout Condition

This lane can close when:

- `ch relay local` and `ch relay <named>` are implemented and documented,
- remote target use can patch Codex and attach TUI,
- local `ch`, `serve`, and `switch` behavior remains compatible,
- focused tests and validation gates pass,
- and follow-on GUI or remote transcript work is explicitly split or deferred.

# Design: Codex Relay Diagnostics Operator Surface

Status: Implemented
Last updated: 2026-05-19

## Why This Lane Exists

The previous relay capability work made Codex official relay and image generation behavior diagnosable through an admin API, but the normal operator path is still the built-in TUI. Users should not need to hand-write `curl` to understand why Codex sees or misses remote compaction, hosted image generation, model metadata, or a patch-mode recommendation.

## Relevant Authority

- Existing docs:
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
  - `CHANGELOG.md`
- Related workstreams:
  - `docs/workstreams/codex-relay-capability-profile/`
  - `docs/workstreams/codex-tui-operator-polish/`
  - `docs/workstreams/codex-tui-startup-guardrail/`

## Problem

The diagnostic contract exists, but it is too hidden. The TUI already exposes patch-mode toggles on Settings, while the capability probe and recommendation live behind `POST /__codex_helper/api/v1/codex/relay-capabilities`. This creates a split-brain operator loop: change mode in the UI, diagnose mode outside the UI.

## Target State

- The core capability diagnostic is a reusable service method, not only a private HTTP handler.
- The TUI Settings page can run a bounded single-shot Codex relay diagnostic from the current runtime.
- The result is visible in the TUI as expected capabilities, observed endpoint support, mismatches, warnings, and recommended patch mode.
- HTTP admin API behavior remains compatible.
- Documentation points users to both the TUI and curl paths.

## In Scope

- Move or expose Codex relay capability request/response DTOs from private route code to reusable core modules.
- Add `ProxyService` API for the diagnostic.
- Add TUI state, async task plumbing, keyboard shortcut, and Settings rendering for diagnostics.
- Focus tests on service reuse, TUI formatting/rendering, and shortcut behavior.
- Update configuration docs, changelog, and workstream evidence.

## Out Of Scope

- GUI implementation.
- CLI subcommand implementation.
- Periodic automatic probes.
- Hosted `image_generation` active probing.
- WebSocket relay support or remote compaction v2 enablement.
- Mutating patch mode automatically from the recommendation.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| TUI Settings is the right first operator surface. | High | Settings already owns Codex patch toggles and runtime config diagnostics. | If users need GUI first, this lane still leaves a reusable core method for GUI follow-up. |
| A manual key-triggered probe is safer than periodic probing. | High | Probe sends `/responses` and `/responses/compact` validation requests to upstreams. | If automatic probing is needed later, it should have rate limits and cache state. |
| The TUI can call `ProxyService` directly instead of loopback HTTP. | High | TUI runs in the same process and already uses `ProxyService` for runtime/profile controls. | If remote-attached TUI is added later, it can consume the same DTO over HTTP. |
| Current model can be inferred from selected/recent/default profile context. | Medium | TUI snapshot has recent and effective model fields; runtime status has profile data. | If inference is weak, users can still run the probe without a model and see catalog-level uncertainty. |

## Architecture Direction

The diagnostic implementation belongs below the transport surface:

- DTOs and recommendation result types are public core contract.
- `ProxyService::codex_relay_capabilities` owns target selection, patch-mode fallback, probing, mismatch construction, and recommendation.
- HTTP route code becomes a thin adapter from JSON to service call.
- TUI owns only request timing, result state, and concise rendering.

This keeps the capability contract stable for HTTP, TUI, future GUI, and potential CLI entry points. It also avoids routing the TUI through local HTTP, which would add token/header concerns without improving the local interactive flow.

## Closeout Condition

This lane can close when:

- TUI Settings has a visible diagnostic action and result block.
- The same core service method backs HTTP and TUI.
- Targeted TUI/core tests and formatting pass.
- Docs/changelog describe the TUI path.
- Follow-on GUI/CLI work is explicitly deferred or split.

# Codex Architecture Deepening — Milestones

> Historical artifact (superseded 2026-07-12): preset aliases, auth facades,
> compatibility readers, and remote-control mutations were removed by the
> canonical relay runtime modernization. The current remote control plane is
> GET/HEAD-only.

Status: Complete
Last updated: 2026-05-20

## M0 — Scope Freeze

Exit criteria:

- [x] The five refactor candidates are represented as vertical slices.
- [x] Non-goals explicitly prevent accidental feature work or compact fallback.
- [x] Gate strategy is written before code changes.

## M1 — Observable Session Identity

Exit criteria:

- [x] Session identity has an explicit source/value representation where source matters.
- [x] Existing routing keys remain compatible.
- [x] Logs or session cards can explain whether identity came from header or `prompt_cache_key`.
- [x] Header identity priority is tested.

## M2 — Shared Request Preparation

Exit criteria:

- [x] HTTP and Responses WebSocket first-frame preparation reuse common identity/override/routing/body logic.
- [x] Transport-specific details remain in small Adapters.
- [x] Existing request encoding, prompt-cache affinity, model mapping, and websocket tests pass.

## M3 — Case Registry Diagnostics

Exit criteria:

- [x] Relay capability and live-smoke cases are discoverable through a registry-like Module.
- [x] Compact, hosted image, and WebSocket behavior stays stable.
- [x] Cost-bearing and optional cases preserve acknowledgement semantics.

## M4 — Test Harness

Exit criteria:

- [x] High-churn proxy integration tests use reusable harness helpers.
- [x] Duplicate upstream capture/failover setup is reduced.
- [x] Assertions remain explicit and behavior-focused.

## M5 — Patch Plan Seam

Exit criteria:

- [x] Codex patch policy can be computed as a pure plan.
- [x] TOML/auth/switch-state writes are execution Adapters.
- [x] Existing preset, facade, readiness, and remote-control behavior remains stable.

## M6 — Verification And Closeout

Exit criteria:

- [x] Fresh targeted gates for each slice pass.
- [x] `cargo fmt --check` and `cargo nextest run -p codex-helper-core` pass.
- [x] Any incomplete candidate is split into a separate workstream with explicit rationale.
- [x] HANDOFF and evidence docs reflect final state.

No incomplete candidate remains in this workstream; no split was required.

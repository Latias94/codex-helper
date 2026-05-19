# Codex Relay Capability Profile

Status: Active
Last updated: 2026-05-19

## Why This Lane Exists

`codex-helper` now has several Codex patch modes that can expose official-like client behavior
through a local relay. The bridge modes are useful, but operators still have to infer whether a
given relay actually supports the official capabilities that Codex will try to use.

This lane turns that implicit knowledge into an explicit capability profile, active probes, and
mode recommendations that work for sub2api and non-sub2api relays.

## Relevant Authority

- Related workstreams:
  - `docs/workstreams/codex-official-relay-bridge/`
  - `docs/workstreams/codex-official-imagegen-bridge/`
- Existing docs:
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
  - `CHANGELOG.md`
- Reference source:
  - `repo-ref/codex/codex-rs/model-provider-info/src/lib.rs`
  - `repo-ref/codex/codex-rs/core/src/compact.rs`
  - `repo-ref/codex/codex-rs/core/src/compact_remote.rs`
  - `repo-ref/codex/codex-rs/tools/src/tool_config.rs`
  - `repo-ref/codex/codex-rs/login/src/auth/manager.rs`
  - `repo-ref/codex/codex-rs/protocol/src/openai_models.rs`

## Problem

The current bridge modes can make Codex expose official-like features, but capability discovery is
split across config patches, `/models` response translation, request-ledger symptoms, and operator
knowledge. This creates three failure modes:

- Codex may expose a tool because the client-side gates are satisfied, while the relay account does
  not actually support the upstream hosted capability.
- Codex may hide a tool because the relay returned an OpenAI-style `/models` list without Codex
  model metadata.
- Operators may choose a patch mode by relay implementation name instead of measured capability.

## Target State

`codex-helper` owns a small capability profile for Codex relays:

- It explains what Codex should expose locally for the selected patch mode and model metadata.
- It records what the selected relay appears to support through safe active probes and recent
  request evidence.
- It recommends the least surprising patch mode for the observed relay.
- It reports uncertainty explicitly instead of treating missing evidence as support.

## In Scope

- A core capability model for Codex client gates:
  - provider identity and remote compaction v1,
  - ChatGPT/Codex-backend auth shape and hosted `image_generation`,
  - model catalog fields that affect tool exposure,
  - WebSocket disabled state.
- Safe relay probes:
  - `/models` shape and Codex model metadata quality,
  - `/responses/compact` support,
  - normal `/responses` compatibility,
  - optional hosted `image_generation` probe only when explicitly requested.
- CLI/admin diagnostics that show expected client exposure, observed upstream support, and mismatch
  reasons.
- Mode recommendation for `default`, `imagegen-bridge`, `official-relay-bridge`, and
  `official-imagegen-bridge`.
- Documentation for sub2api and non-sub2api relays.

## Out Of Scope

- New patch modes unless the capability model proves the existing modes cannot represent a real
  Codex gate.
- WebSocket forwarding. Current bridge modes should continue writing `supports_websockets = false`
  until relay support exists.
- Default remote compaction v2 enablement. V2 is diagnostic-only until official and relay semantics
  stabilize.
- Faking official account entitlements. The profile may expose client-side gates, but upstream
  support must come from the relay account.
- Replacing runtime scheduler health, balance, or retry policy. Capability probes may inform
  diagnostics but must not become a per-request retry loop.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Remote compaction v1 is gated by provider identity and uses `/responses/compact`. | High | Official Codex `supports_remote_compaction()` and compact client source. | The official relay bridge would need a different trigger and docs would be wrong. |
| Hosted image generation is gated by Codex-backend auth shape plus image-capable model metadata. | High | Official tool config and auth manager source. | The empty auth facade would be insufficient or overly broad. |
| Non-sub2api relays can be supported by helper-side model translation and probes. | Medium | Current `models_compat` already translates OpenAI `/models` lists. | More relay-specific adapters may be needed. |
| Active image-generation probes can cost money or create artifacts. | High | Hosted image generation is a real model operation. | Image probes must stay explicit and opt-in. |
| Balance signals can be stale or relay-specific. | High | Existing route exhaustion work and operator observations. | Capability profile must use bounded confidence, not assume balance truth. |

## Architecture Direction

Create a deep core module around a `CodexCapabilityProfile` concept. The module should own the
mapping between Codex client gates, model catalog fields, and observed relay support. Callers should
not need to know which Codex source file contains a gate or which patch mode toggles it.

The first implementation should keep probes opt-in and bounded. It should prefer existing request
ledger evidence when available, then allow explicit active probes for uncertain capabilities. The
profile should distinguish:

- `expected`: what Codex will likely expose with the current patch mode and model catalog,
- `observed`: what the relay has actually accepted or rejected,
- `confidence`: whether the conclusion is static, request-ledger-derived, or actively probed,
- `recommendation`: what patch mode best matches the observed relay.

This keeps locality: Codex gate knowledge and relay probe interpretation live in one module instead
of leaking into patch mode code, request logging, CLI rendering, and docs independently.

## Closeout Condition

This lane can close when:

- a capability profile is available through at least one operator-facing surface,
- `/models` and `/responses/compact` are classified with tests,
- mode recommendations are deterministic and documented,
- image generation probing is explicit and safe,
- evidence gates pass,
- and WebSocket/v2/entitlement limitations are documented as follow-ons or non-goals.

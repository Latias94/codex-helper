# Codex Official Relay Bridge

Status: Active
Last updated: 2026-05-18

## Why This Lane Exists

Codex now has behavior that is only enabled for the official OpenAI/Azure provider path. `codex-helper`
currently makes OpenAI-compatible relay providers usable, but it does not expose all official Codex
experience features through those relays. The first concrete gap is context compaction: historical
logs show ordinary `/responses` traffic only, with no `/responses/compact` and no
`compaction_trigger` payloads.

## Relevant Authority

- Codex reference:
  - `repo-ref/codex/codex-rs/core/src/compact.rs`
  - `repo-ref/codex/codex-rs/core/src/compact_remote.rs`
  - `repo-ref/codex/codex-rs/core/src/compact_remote_v2.rs`
  - `repo-ref/codex/codex-rs/model-provider-info/src/lib.rs`
- sub2api reference:
  - `repo-ref/sub2api/backend/internal/service/openai_compact_probe.go`
  - `repo-ref/sub2api/backend/internal/handler/openai_gateway_handler.go`
  - `repo-ref/sub2api/backend/internal/service/openai_gateway_service.go`
  - `repo-ref/sub2api/backend/internal/service/openai_ws_forwarder.go`
- codex-helper implementation:
  - `crates/core/src/codex_integration.rs`
  - `crates/core/src/proxy/attempt_request.rs`
  - `crates/core/src/proxy/router_setup.rs`
  - `docs/CONFIGURATION.md`

## Problem

Relay users can send ordinary Codex `/responses` requests through `codex-helper`, but Codex treats
the helper provider as a non-official compatible provider. That prevents Codex from selecting remote
context compaction v1, even when the relay behind helper can support `/responses/compact`.

## Target State

First-stage target:

- `codex-helper` can install a Codex patch mode that advertises enough official-provider semantics for
  Codex to choose remote compaction v1.
- `codex-helper` preserves relay authentication behavior and safely forwards `/responses/compact` to
  existing upstream routes.
- Operators can diagnose whether compaction is using ordinary `/responses` fallback or official
  `/responses/compact`.
- Tests cover the patch mode and proxy routing behavior without requiring real OpenAI credentials.

Later stages:

- Capability-aware profile hints for relays that can or cannot handle `/responses/compact`.
- WebSocket relay support only after helper owns an HTTP upgrade path and can forward OpenAI Responses
  WebSocket traffic safely.
- Remote compaction v2 only after Codex enables/stabilizes the feature and target relays support
  `compaction_trigger` responses.

## In Scope

- New or refined Codex patch mode for official relay behavior.
- Config or internal metadata needed to gate remote compact behavior.
- HTTP proxy support and tests for `/responses/compact`.
- Request/control logging fields needed to identify compact requests.
- Documentation for relay operators.

## Out Of Scope

- Implementing helper WebSocket upgrade forwarding in the first slice.
- Reimplementing sub2api compact probing inside helper unless the first slice proves it is necessary.
- Enabling remote compaction v2 by default.
- Handling unofficial APIs that do not preserve OpenAI Responses semantics.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Codex chooses remote compact v1 only for providers where `supports_remote_compaction()` is true. | High | `compact.rs`, `model-provider-info/src/lib.rs` | Patch mode must emulate official provider identity or Codex source must be patched differently. |
| `sub2api` supports `/responses/compact` v1 for selected accounts. | High | `openai_compact_probe.go`, gateway compact tests | Helper can start with routing instead of inventing compact protocol handling. |
| Helper currently logs no historical `/responses/compact` traffic. | High | Local `requests*.jsonl` and control/runtime log search on 2026-05-18 | First slice should produce a visible diagnostic change. |
| Enabling `supports_websockets=true` without proxy upgrade support is unsafe. | High | No helper WebSocket proxy implementation found; sub2api has separate WS stack | WebSocket work must be a separate slice. |

## Architecture Direction

Treat helper as an "official relay bridge" rather than an OpenAI-compatible provider. Codex-facing
patching should make Codex select official code paths where the helper can faithfully forward the
protocol. Helper-facing routing should remain provider-config driven, so the same endpoint selection,
retry, logging, and auth stripping behavior applies to `/responses/compact` as to `/responses`.

The first implementation should prefer a narrow, testable patch mode over broad config mutation. If
the relay cannot support `/responses/compact`, operators should be able to stay on the existing
compatible mode and keep ordinary local compaction behavior.

## Closeout Condition

This lane can close when:

- remote compact v1 bridge behavior is implemented and tested,
- compact route observability is documented,
- WebSocket and remote compact v2 are either completed or split into follow-ons,
- evidence gates pass and are recorded,
- and `WORKSTREAM.json` reflects the final state.

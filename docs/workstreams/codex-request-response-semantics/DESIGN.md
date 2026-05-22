# Codex Request Response Semantics

Status: Complete
Last updated: 2026-05-22

## Why This Lane Exists

`codex-helper` already exposes official-like Codex relay modes, but a few Codex-specific request and
response semantics still lag behind mature relay behavior. The most visible gaps are session
continuity completion, stale `previous_response_id` recovery, clearer `service_tier` attribution,
and bounded response repair for relay encoding quirks.

## Relevant Authority

- Existing docs:
  - `README.md`
  - `README_EN.md`
  - `CHANGELOG.md`
- Related workstreams:
  - `docs/workstreams/codex-relay-capability-profile/`
  - `docs/workstreams/codex-relay-smoke-evidence-cli/`
- Reference source:
  - `repo-ref/codex/`
  - `repo-ref/aio-coding-hub/src-tauri/src/gateway/codex_session_id.rs`
  - `repo-ref/aio-coding-hub/src-tauri/src/gateway/proxy/handler/failover_loop/response/upstream_error.rs`
  - `repo-ref/aio-coding-hub/src-tauri/src/gateway/response_fixer/`

## Problem

Codex clients and OpenAI-compatible relays rely on small but important continuity hints. When those
hints are missing, stale, or encoded in a relay-specific way, requests can lose affinity, fail on a
dead `previous_response_id`, or hide actual fast-mode behavior from the operator.

## Target State

- Codex requests that fail because an upstream no longer knows `previous_response_id` are retried
  once without that field.
- Codex session identifiers are completed from already-present request evidence without overwriting
  user-provided identifiers.
- `service_tier` logging distinguishes requested, effective, and actual values for both streaming
  and non-streaming responses.
- Response repair is bounded and explicit: fix only known relay protocol defects, keep normal
  responses untouched, and strip stale encoding headers when bytes are repaired.

## In Scope

- Codex `/responses` and `/responses/compact` request body helpers.
- Same-upstream retry after confirmed stale `previous_response_id` errors.
- Header/body completion for `session_id`, `x-session-id`, and `prompt_cache_key` from existing
  request evidence.
- Actual `service_tier` extraction from JSON and SSE shapes.
- Non-streaming gzip response repair when the relay returns gzip bytes despite normal forwarding.
- Tests and documentation for the shipped behavior.

## Out Of Scope

- Direct ChatGPT backend upstream compatibility for `https://chatgpt.com/backend-api/codex`.
- Generating a synthetic session id from client fingerprint when the request has no session
  evidence.
- Broad JSON/SSE rewriting that changes model output payloads.
- Changing user-requested `model`, `reasoning.effort`, or `service_tier` unless a session/manual
  override explicitly applies.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Stale `previous_response_id` errors are distinguishable from ordinary client errors by status and body text. | High | AIO implements this as a targeted 400/404 retry. | The retry guard must stay conservative and disabled for ambiguous errors. |
| Completing missing session fields from an existing session hint preserves intent better than inventing a new id. | High | Current helper already uses `prompt_cache_key` as affinity when no header exists. | A generated-id follow-up would need a separate opt-in design. |
| `service_tier` can appear in root response objects, nested `response`, and SSE events. | High | Current extraction already supports those shapes. | Additional provider shapes can be added as test-driven follow-ons. |
| Some relays return gzip response bytes even when clients did not ask for compressed output. | Medium | AIO contains a response-side gzip workaround. | The fixer must be bounded and no-op on non-gzip bodies. |

## Architecture Direction

Keep Codex semantics in small proxy modules close to existing ownership:

- Request body parsing and field mutations live in `request_body.rs`.
- Request preparation completes session identifiers before routing and upstream setup.
- Retry recovery lives inside the existing selected-upstream attempt loop so it can reuse the same
  target, headers, route logs, and body previews.
- Response repair happens before classification, usage extraction, service-tier extraction, and
  final forwarding.

The boundary is intentional: Codex-specific repair may add missing continuity fields or remove a
confirmed-stale `previous_response_id`, but it must not silently rewrite model, effort, or tier.

## Closeout Condition

This lane can close when:

- both P1 features and both P2 features are implemented,
- focused tests and full core tests pass,
- docs and changelog describe the behavior,
- and any ChatGPT backend compatibility exploration is documented as a follow-on.

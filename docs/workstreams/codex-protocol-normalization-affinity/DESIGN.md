# Codex Protocol Normalization And Affinity — Design

Status: Complete
Last updated: 2026-05-20

## Why This Lane Exists

`codex-helper` should not implement missing upstream relay features, but it also must not become the
weakest board in the barrel when a capable relay such as `sub2api` sits behind it. Direct
Codex-to-sub2api traffic can already benefit from request body content-encoding decode, prompt-cache
session stickiness, `/responses/compact`, and Responses WebSocket v2 support. Helper should preserve
that experience while still remaining safe for less capable OpenAI-compatible relays.

## Relevant Authority

- Related workstreams:
  - `docs/workstreams/codex-official-relay-bridge`
  - `docs/workstreams/codex-responses-websocket-relay`
  - `docs/workstreams/codex-relay-capability-profile`
- Reference repos:
  - `repo-ref/codex`: Codex may send `Content-Encoding: zstd` request bodies for official OpenAI
    Responses traffic.
  - `repo-ref/sub2api`: decodes `zstd/gzip/deflate`, uses `prompt_cache_key` as an explicit session
    signal, and preserves Codex turn/session headers.
  - `repo-ref/new-api`: supports `gzip/br` request decode but not `zstd`; compact request conversion
    can drop Codex compact fields unless pass-through is enabled.

## Problem

Two helper-side gaps can degrade a relay that would work better when used directly:

1. Codex may send compressed HTTP request bodies, especially `Content-Encoding: zstd`. Helper reads
   request bytes before routing and override application, but currently treats those bytes as the
   upstream body. That means helper cannot inspect compressed JSON for model/effort/service-tier or
   `prompt_cache_key`, and downstream relays without zstd support can fail.
2. Helper session identity currently comes mostly from headers such as `session_id` and
   `conversation_id`. sub2api also treats body `prompt_cache_key` as an explicit session signal.
   Without the same fallback, helper route affinity can choose a different upstream for a
   session's `/responses` and `/responses/compact` traffic before the request even reaches sub2api.

## Target State

- Helper normalizes supported request content encodings by default:
  - decode `zstd`, `gzip`/`x-gzip`, `br`, and `deflate`;
  - remove `Content-Encoding` and stale `Content-Length` before forwarding decoded JSON;
  - keep a documented escape hatch for rare relays that require raw compressed bodies.
- Helper derives session identity from `body.prompt_cache_key` when no stronger header identity is
  available, so route affinity mirrors sub2api-like behavior.
- If request decode fails, helper returns a clear client error instead of silently forwarding broken
  bytes after it has committed to inspecting/normalizing the body.
- Existing `/responses/compact` semantics remain pass-through. Helper must not synthesize compact
  output or fake official state.

## In Scope

- HTTP request body content-encoding normalization for normal proxy paths.
- A request body encoding behavior switch with safe default behavior and a passthrough escape hatch.
- Session identity fallback from decoded JSON `prompt_cache_key`.
- Tests proving zstd and prompt-cache affinity behavior.
- Documentation for why this protects sub2api-like relays without requiring users to identify relay
  implementation details.

## Out Of Scope

- Implementing `/responses/compact` fallback or synthetic compaction.
- Adding missing compact/WebSocket/image-generation support to upstream relays.
- WebSocket frame compression or permessage-deflate parity.
- Vendor fingerprinting as a required user decision. Diagnostics may mention observed behavior, but
  users should not need to choose "new-api vs sub2api" manually.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| Most OpenAI-compatible relays accept uncompressed JSON when they also accept zstd. | High | HTTP content-encoding is transport-level; sub2api decodes then forwards JSON. | Keep per-upstream passthrough escape hatch. |
| Some relays do not support zstd and benefit from helper-side decode. | High | new-api middleware supports gzip/br only; Codex may emit zstd. | Default normalization avoids avoidable relay failures. |
| `prompt_cache_key` is a stable enough session signal for Codex routing affinity. | High | sub2api uses it after `session_id`/`conversation_id`; existing helper affinity is session-scoped. | Only use it as fallback when stronger headers are absent. |
| Decoding compressed body before model overrides is acceptable. | Medium | Helper already reads request bodies for routing/logging/overrides. | Configuration escape hatch can preserve raw bytes for special relays. |

## Architecture Direction

Add request normalization near the single HTTP request-preparation boundary, before body inspection and
override application. That keeps downstream code operating on canonical JSON bytes and avoids scattered
encoding checks in routing, logging, model override, and upstream request setup.

Represent the normalized request as both:

- body bytes to forward upstream, and
- request header changes required by normalization.

The first implementation should prefer a minimal runtime option:

- default: `auto` decode known encodings;
- escape hatch: `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` preserves original compressed body
  and header.

Persisted config schema churn is intentionally avoided for the first slice because passthrough is a
rare relay-compatibility escape hatch rather than a normal user-facing routing choice.

Session identity extraction should be extended to accept a body fallback rather than duplicating JSON
parsing across routing code. Header identity keeps priority; decoded `prompt_cache_key` is used only
when headers do not provide a session.

## Closeout Condition

This lane can close when:

- zstd/gzip/br/deflate request normalization is implemented and tested;
- passthrough escape hatch is documented and tested;
- body `prompt_cache_key` drives route affinity fallback and is tested with multiple upstreams;
- docs and evidence gates are updated;
- no helper-side compact fallback has been introduced.

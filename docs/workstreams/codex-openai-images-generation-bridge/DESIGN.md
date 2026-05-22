# Codex OpenAI Images Generation Bridge

Status: Complete
Last updated: 2026-05-22

## Why This Lane Exists

Codex-helper already routes Codex `/responses` traffic through local provider failover, but project-local image generation skills need a simple OpenAI-compatible `/v1/images/generations` endpoint that can reuse the same remaining provider chain. The built-in Codex hosted image generation path is useful, but brittle when a relay only partially matches Codex expectations.

## Relevant Authority

- Existing docs:
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
  - `README.md`
  - `README_EN.md`
- Related workstreams:
  - `docs/workstreams/codex-official-imagegen-bridge`
  - `docs/workstreams/codex-request-response-semantics`
  - `docs/workstreams/codex-relay-live-smoke-diagnostics`
- References:
  - `repo-ref/codex/codex-rs/skills/src/assets/samples/imagegen`
  - `C:/Users/Administrator/Downloads/Compressed/650e1017903a30ba9f9e59675506a2fe617d2088/imagegen`

## Problem

Agents and skills can call the regular OpenAI Images API shape more reliably than depending on Codex's hosted image-generation tool exposure. Today a request such as:

```http
POST /v1/images/generations
{
  "model": "gpt-image-2",
  "prompt": "一只猫在雨夜的霓虹灯下",
  "size": "3840x2160",
  "output_format": "png",
  "quality": "high"
}
```

either passes through as-is to one selected upstream or fails if the upstream only supports the Responses hosted `image_generation` tool shape. It also does not provide a project-owned skill that knows how to call codex-helper, save the returned image, and validate the new artifact.

## Target State

- codex-helper exposes `POST /v1/images/generations` and `/images/generations` on the proxy listener.
- The endpoint accepts a compact OpenAI Images-style JSON request and internally converts it to a non-streaming Responses request with an `image_generation` tool.
- The request still goes through existing routing, retry, failover, auth injection, request ledger, model mapping, and response-repair machinery.
- Successful Responses image-generation output is converted back to OpenAI Images-style JSON with `data[].b64_json`.
- A locally installed `ch-imagegen` skill calls the endpoint, computes valid `gpt-image-2` sizes, saves only new outputs, and reports deterministic JSON.

## In Scope

- Proxy route and request/response translation for image generations.
- Focused unit/proxy tests for translation, routing, and error behavior.
- Local `ch-imagegen` skill installation under the Codex skills directory.
- README/configuration/changelog notes for the new endpoint and skill path.

## Out Of Scope

- Multipart image editing (`/v1/images/edits`).
- Multiple generated images per request (`n > 1`) unless the upstream hosted tool contract gains a proven batch shape.
- Claiming that every relay supports hosted image generation; failures remain visible through normal upstream errors and request logs.
- Replacing Codex's built-in `image_generation` tool or the existing official-imagegen bridge mode.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| The most reliable shared upstream path is Responses + hosted `image_generation`, not direct Images API passthrough. | Medium | Existing reference skill calls `/v1/responses` with `tools: [{type: image_generation}]`. | Add an explicit direct-passthrough mode later if providers prefer Images API. |
| Existing proxy routing/failover should remain authoritative. | High | Provider execution already owns retries, auth, model mapping, request ledger, and cooldowns. | A bespoke image client would duplicate failover and drift from the rest of the proxy. |
| A one-image contract is sufficient for skill usage. | High | Provided curl omits `n`; Codex hosted image generation calls are item-oriented. | Add explicit multi-image fan-out later. |
| The local skill should not embed provider secrets. | High | Project rules avoid sensitive output; upstream auth already lives in codex-helper config/env. | Users pass real auth through codex-helper config or environment. |

## Architecture Direction

Implement an edge adapter at the proxy router boundary. The adapter normalizes an OpenAI Images-style request into a synthetic `/v1/responses` request and calls the existing `handle_proxy` path. This preserves routing semantics and avoids a parallel provider executor. After a successful upstream response, the adapter extracts the first completed `image_generation_call.result` base64 payload and emits an Images-compatible JSON response.

Keep the local `ch-imagegen` skill thin and deterministic: it computes safe dimensions, sends one JSON request to the local endpoint, decodes `data[0].b64_json`, writes a timestamped final file, and validates the newly written artifact.

## Closeout Condition

This lane can close when:

- the endpoint works through existing provider routing,
- unsupported or malformed inputs fail deterministically,
- `ch-imagegen` is installed and validates,
- Rust formatting and focused nextest gates pass,
- docs reflect the shipped behavior,
- and follow-on work is either split or explicitly deferred.

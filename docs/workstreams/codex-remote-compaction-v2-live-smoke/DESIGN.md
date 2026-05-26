# Codex Remote Compaction V2 Live Smoke

## Problem

Codex `remote_compaction_v2` is not a `/responses/compact` call. It is an ordinary streaming
`/responses` request whose `input` contains a `compaction_trigger` item. A proxy can keep this
request sticky to the same provider and still fail if the selected relay or upstream does not
understand the v2 compaction output contract.

## Decision

Add an explicit-only live-smoke case named `remote_compaction_v2` that sends a real
`POST /responses` request with:

- `stream: true`;
- one `input` item of type `compaction_trigger`;
- no tools;
- the `x-codex-beta-features: remote_compaction_v2` request hint used by Codex clients.

The case passes only when the response stream contains exactly one compaction output item event and
a `response.completed` event. Errors, JSON-only responses, missing completion, and duplicate
compaction output items are recorded distinctly so operators can tell whether the failure is relay
transport, upstream protocol support, or an upstream/model error.

## Constraints

- Do not assume the upstream is sub2api, new-api, OpenAI, or any particular relay implementation.
- Keep the default live smoke unchanged: no optional flag still runs only `responses_compact`.
- Require the existing live-smoke acknowledgement token before any upstream I/O.
- Keep image generation and WebSocket optional case behavior unchanged.

## Non-Goals

- Do not make v2 compaction the default live-smoke case.
- Do not emulate Codex's full compaction transcript or encrypted-content lifecycle.
- Do not use live-smoke output to alter routing state automatically.

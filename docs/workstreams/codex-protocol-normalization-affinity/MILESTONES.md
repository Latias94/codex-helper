# Codex Protocol Normalization And Affinity — Milestones

Status: Complete
Last updated: 2026-05-20

## M0 — Scope Freeze

Exit criteria:

- The lane states the boundary: preserve upstream capability; do not synthesize upstream features.
- The two concrete deliverables are request content-encoding normalization and prompt-cache affinity.
- Evidence gates are defined before code changes.

## M1 — Normalized Request Bodies

Exit criteria:

- Known encodings decode before body JSON inspection:
  - `zstd`
  - `gzip` / `x-gzip`
  - `br`
  - `deflate`
- Forwarded request headers no longer contain stale `Content-Encoding` or `Content-Length` after decode.
- Corrupt encoded bodies fail clearly.
- A passthrough escape hatch exists or is explicitly split before closeout.

## M2 — Prompt-Cache Affinity

Exit criteria:

- Header session signals keep priority.
- `prompt_cache_key` becomes the fallback session identity after successful decode.
- Route affinity tests prove repeated requests with the same prompt cache key stay on the same upstream.
- `/responses/compact` participates in the same affinity behavior.

## M3 — Operator Contract

Exit criteria:

- Docs explain default behavior and escape hatch.
- Docs avoid asking users to classify relays as sub2api/new-api.
- Docs avoid claiming helper implements compact/WebSocket support on behalf of upstream.

## M4 — Verification

Exit criteria:

- Targeted tests pass.
- Formatting passes.
- `codex-helper-core` package gate passes or a justified narrower gate is recorded.
- HANDOFF.md is updated with status and next action.

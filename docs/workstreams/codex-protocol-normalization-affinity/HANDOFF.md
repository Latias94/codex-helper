# Codex Protocol Normalization And Affinity — Handoff

Status: Complete
Last updated: 2026-05-20

## Current State

The lane is complete. CPNA-020, CPNA-030, CPNA-040, CPNA-050, and CPNA-060 are implemented and
verified in the working tree.

Implemented behavior:

- HTTP proxy request preparation decodes `Content-Encoding: zstd`, `gzip` / `x-gzip`, `br`, and
  `deflate` before body inspection, override application, routing context, and upstream forwarding.
- Successful decode removes stale `Content-Encoding` and `Content-Length`; corrupt or unsupported
  encoded requests return `400 BAD_REQUEST` before hitting upstream.
- `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` preserves the raw body and header for rare
  relays that require Codex's original compressed bytes.
- Header session identity still wins; decoded JSON `prompt_cache_key` becomes the fallback session
  identity for HTTP and the first Responses WebSocket `response.create` frame.
- Docs now describe the compatibility layer and explicitly avoid claiming helper implements missing
  upstream compact/WebSocket/hosted-tool features.

## Next Executable Task

No required work remains for this lane. Optional follow-ons should be split separately:

- A persisted config key for request-body encoding mode if env-only escape hatch proves too hidden.
- Additional live relay smoke coverage for vendor-specific edge cases.
- WebSocket frame compression/permessage-deflate parity if Codex starts requiring it.

## Important Constraints

- Do not implement `/responses/compact` fallback.
- Do not require users to know whether their relay is sub2api or new-api.
- Preserve a passthrough escape hatch for unusual relays that require raw compressed Codex bodies.
- Header session identity must keep priority over `prompt_cache_key`.

## Suggested Validation

Already recorded in `EVIDENCE_AND_GATES.md`:

```powershell
cargo fmt --check
cargo nextest run -p codex-helper-core request_content_encoding prompt_cache_key_affinity --no-fail-fast
cargo nextest run -p codex-helper-core
```

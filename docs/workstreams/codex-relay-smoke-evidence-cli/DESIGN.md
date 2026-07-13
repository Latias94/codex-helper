# Design: Codex Relay Smoke Evidence CLI

Status: Complete
Last updated: 2026-05-19

> Historical contract note (superseded 2026-07-12): the station/upstream selector and raw upstream URL fields described below were retired in favor of canonical `provider_id`/`endpoint_id`/`provider_endpoint_key` identity. The original text is retained as implementation history, not the current CLI or evidence schema.

## Why This Lane Exists

Codex relay capability diagnostics and live smoke now share safe core execution paths, but operators
still need a terminal-first way to run them against specific relay accounts and keep the results for
later comparison. Request logs intentionally do not record live smoke because these checks bypass
normal routing state. That leaves no durable trail for "this relay proved compact at this time" or
"this relay accepted hosted image generation but did not emit an image call".

## Problem

Current surfaces are split:

- Admin API is scriptable but requires hand-written JSON and a running proxy/admin listener.
- TUI is ergonomic but not easy to capture in bug reports or compare across relays.
- Capability diagnostics and live smoke intentionally avoid request ledger, passive health, balance,
  retry, and affinity state, so results are not persisted anywhere.

Operators need a diagnostic evidence store that is durable but explicitly non-authoritative for
routing.

## Target State

- A JSONL evidence store records successful capability diagnostics and live-smoke response summaries.
- Evidence entries include timestamp, kind, service, target station/upstream, model, upstream URL,
  and the sanitized result payload.
- The evidence store is separate from request ledger, route health, balance snapshots, retry state,
  and patch-mode state.
- CLI can run:
  - validation-only capability diagnostics,
  - compact-only live smoke,
  - compact + hosted image-generation live smoke,
  - recent evidence listing.
- CLI live smoke requires the same acknowledgement string as API/TUI before any upstream call.
- CLI can emit human-readable output by default and JSON for scripts.

## In Scope

- Core evidence DTOs, append path, recent reader, and filters.
- Service integration after successful capability diagnostics and live-smoke response construction.
- CLI command group for Codex relay diagnostics and evidence inspection.
- Tests for store append/read, service evidence writes, and CLI argument behavior where practical.
- Configuration docs and changelog.

## Out Of Scope

- Treating evidence as routing health or capability truth.
- Automatic patch-mode mutation based on evidence.
- Periodic relay probing.
- Storing raw image bytes, base64 image payloads, credentials, or request bodies.
- WebSocket or remote compaction v2 smoke.

## Safety Contract

- Evidence append failures must not fail the diagnostic itself.
- Missing live-smoke acknowledgement must still fail before upstream IO and before evidence append.
- Live smoke continues to send at most one request per selected case.
- Evidence records must stay sanitized and bounded to the already summarized response DTOs.
- Evidence readers are local-only file readers; they do not call upstreams.

## Architecture Direction

Keep the store under the Codex relay diagnostic domain, but physically separate it from
`requests.jsonl`:

```text
~/.codex-helper/logs/codex_relay_evidence.jsonl
```

Use core APIs for append/read so API, TUI, and CLI share one evidence format. The CLI should build a
local `ProxyService` and call the same service methods as the TUI/admin API instead of duplicating
HTTP or request-builder logic.

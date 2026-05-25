# Codex Remote Compaction V2 Continuity - Handoff

Status: Complete
Last updated: 2026-05-26

## Current Task

None. Workstream complete.

## Context

Codex `remote_compaction_v2=true` changes remote compact from
`POST /responses/compact` to a normal streaming `POST /responses` request with
a structured `compaction_trigger` input item. The proxy already has
provider-opaque state-bound continuity behavior for v1 compact, but v2 compact
is currently invisible to that path.

The implementation must not assume the upstream provider is OpenAI, sub2api,
new-api, or any specific relay. The only safe continuity unit currently owned
by the proxy is provider endpoint identity from the session route affinity
ledger.

## Next Step

No further work is required for this lane. Ask the user before committing.

## Validation

Passed gates:

- `cargo nextest run -p codex-helper-core remote_compaction_v2 request_logs --no-fail-fast`
- `cargo nextest run -p codex-helper-core remote_compaction_v2 route_affinity --no-fail-fast`
- `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`
- `cargo fmt --all --check`
- `cargo nextest run -p codex-helper-core`

## Completed

- CRC2-020: V2 compact body classification and request logging.
- CRC2-030: Provider-state-bound continuity policy for v2 compact.
- CRC2-040: Public docs, changelog, diagnostics wording, and evidence gates.

## Residual Risks And Follow-Ups

- State-bound v2 compact will fail closed when no route affinity is known.
  Operators who know two provider endpoints share upstream state still need a
  future explicit continuity-domain feature before cross-provider fallback can
  be made safe.
- This lane does not live-smoke v2 relay support.

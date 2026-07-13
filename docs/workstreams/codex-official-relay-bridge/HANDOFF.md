# Codex Official Relay Bridge — Handoff

Status: Complete
Last updated: 2026-05-18

> Historical status (superseded 2026-07-12): `official-relay-bridge` and all client patch presets/auth-facade behavior were removed. The local switch now changes only the helper selector/stanza in Codex `config.toml`; Responses and compact support are determined by the selected provider contract and bounded observations.

## Current State

The workstream is complete. CORB-020, CORB-030, CORB-040, and CORB-050 are implemented and verified:
`official-relay-bridge` installs a
Codex `codex_proxy` provider with `name = "OpenAI"`, `wire_api = "responses"`,
`supports_websockets = false`, and no `requires_openai_auth`, so current Codex builds select remote
compaction v1 while helper keeps relay credentials in its own routing layer.

The proxy already had wildcard routing; tests now prove `/responses/compact` reaches an upstream
`/v1/responses/compact` endpoint and remains visible in finished request paths for diagnostics.
The request ledger now supports a `path` filter, so operators can run
`codex-helper usage find --path responses/compact --limit 20` or query
`/__codex_helper/api/v1/request-ledger/recent?path=responses/compact` to distinguish official
compact traffic from ordinary `/responses` fallback.
Tests also prove an unsupported relay status such as 404 remains visible on the same compact path,
which is the first-release fallback signal before switching back to `default`.

## Final Task

- Task ID: CORB-050
- Owner: main
- Files: `docs/workstreams/codex-official-relay-bridge/*`
- Validation: `cargo nextest run --workspace`
- Status: DONE
- Review: no blocking, important, or minor findings recorded in `REVIEW.md`.
- Evidence: `docs/workstreams/codex-official-relay-bridge/EVIDENCE_AND_GATES.md`

## Decisions Since Last Update

- WebSocket forwarding is deferred until helper owns an upgrade path.
- Remote compaction v2 is deferred because Codex marks the feature under development and sub2api evidence points to v1 support, not stable v2 support.
- The first proof should be a user-selected official relay bridge mode rather than automatic probing.
- `official-relay-bridge` deliberately strips Codex client auth like other bridge modes and requires
  at least one helper-side upstream credential before enabling.
- The mode does not patch `auth.json`; switching to it restores any helper-managed auth facade from a
  prior bridge mode when safe.
- Capability hints/active compact probing are deferred. First release relies on explicit operator
  selection and documented fallback to `default` when a relay rejects `/responses/compact`.
- WebSocket forwarding is still unsafe to advertise because helper has no HTTP upgrade forwarding
  path; sub2api's WebSocket support is implemented as a separate gateway/forwarder stack.

## Blockers

- None currently.

## Follow-Ons

- WebSocket upgrade forwarding for OpenAI Responses after helper owns an HTTP upgrade path.
- Remote compaction v2 after Codex and target relays stabilize the `compaction_trigger` semantics.
- Optional active compact probing/capability hints if operators need automatic relay classification.

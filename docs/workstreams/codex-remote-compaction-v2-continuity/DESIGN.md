# Codex Remote Compaction V2 Continuity

Status: Draft
Last updated: 2026-05-26

Update, 2026-05-27: later route-continuity work made missing-affinity behavior
policy-sensitive. Under `fallback-sticky`, v2 compact without prior route affinity is tryable
through the configured route graph and records affinity after success. Under `hard` affinity and
legacy multi-upstream routing, missing state-bound affinity still fails closed.

## Why This Lane Exists

Codex `remote_compaction_v2` no longer uses `POST /responses/compact`. It sends
a normal `POST /responses` stream whose request input contains a
`compaction_trigger` item, then expects a compaction response item from the
stream. The proxy currently treats that request like an ordinary user turn, so
the route logs do not explain that it was compaction and the compact continuity
policy is not applied.

## Relevant Authority

- Upstream Codex source:
  - `repo-ref/codex/codex-rs/core/src/tasks/compact.rs`
  - `repo-ref/codex/codex-rs/core/src/tasks/compact_remote_v2.rs`
  - `repo-ref/codex/codex-rs/protocol/src/models.rs`
- Local source anchors:
  - `crates/core/src/proxy/request_body.rs`
  - `crates/core/src/proxy/request_preparation.rs`
  - `crates/core/src/proxy/provider_execution.rs`
  - `crates/core/src/logging.rs`
- Related workstream:
  - `docs/workstreams/codex-session-route-continuity`

## Problem

The proxy has a provider-opaque continuity policy for v1 remote compact
requests, but v2 compact is hidden inside the ordinary `/responses` route. This
creates three operator problems:

- logs show `/responses` rather than a compact classification;
- route selection can apply normal user-turn semantics instead of compact
  state-bound semantics;
- a failed v2 compact is harder to diagnose because the helper cannot say
  whether continuity, relay support, or upstream runtime failure was involved.

## Target State

- Detect Codex remote compaction v2 by parsing the JSON body for a structured
  `{"type":"compaction_trigger"}` item on `POST /responses`.
- Mark v2 compact in request flavor and request logs without storing sensitive
  body content.
- Treat v2 compact as provider-state-bound by default because the request is
  compacting the current session state and may depend on prior encrypted
  content.
- Reuse the provider-opaque continuity policy: use known route affinity when
  available; allow policy-controlled bootstrap when the active route policy is
  tryable; fail closed when hard/legacy state-bound affinity is missing; do not
  infer relay internals from provider name, base URL, balance probes, or 429.
- Keep v1 `/responses/compact` behavior intact.

## In Scope

- Add v2 compact body classification.
- Extend `RequestFlavor` and `CodexBridgeLog` with a v2 compact flag.
- Apply compact continuity and failover policy to v2 compact.
- Add targeted unit and integration coverage.
- Update public docs and this workstream evidence.

## Out Of Scope

- Enabling `remote_compaction_v2` automatically in Codex config.
- Implementing relay-specific behavior for sub2api, new-api, OpenAI, or any
  other intermediary.
- Proving a relay supports v2 compact with a live smoke test.
- Hard cross-provider fallback for state-bound compact without an explicit
  continuity domain. That remains controlled by operator configuration.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| V2 compact request bodies contain a structured `compaction_trigger` response input item. | High | Upstream `compact_remote_v2.rs` appends `ResponseItem::CompactionTrigger`. | Detection would miss v2 compact and logs would remain ambiguous. |
| V2 compact should be treated as provider-state-bound by default. | High | It compacts the active session and may include encrypted or provider-bound state. | The proxy may fail closed more often than necessary until continuity domains exist. |
| Provider endpoint identity is the safest continuity unit the proxy owns. | High | Existing session route continuity work persists provider endpoint affinity. | Operators needing shared upstream state must configure a future explicit domain. |
| The proxy cannot reliably know the relay implementation behind a provider. | High | User providers may point at OpenAI, sub2api, new-api, or opaque relays. | Relay-specific fallback rules would be brittle and unsafe. |

## Architecture Direction

Classify request shape in two phases:

- path/header classification before body parsing still recognizes v1 compact
  and ordinary `/responses`;
- body-aware finalization recognizes v2 compact only after the decoded body is
  available.

The body-aware finalization updates both execution semantics and log semantics.
Provider execution should consume a single "remote compaction request" predicate
covering v1 and v2, rather than duplicating v1-specific booleans.

V2 compact is state-bound by default. If a session already has route affinity,
the request stays on that provider endpoint. Under `fallback-sticky`, a
missing-affinity v2 compact request is intentionally tryable through the route
graph so the successful endpoint can become the session affinity. Under `hard`
affinity and legacy multi-upstream routing, missing affinity still returns a
continuity error instead of silently re-entering provider preference routing. If
the affinity endpoint fails, fallback behavior follows the active affinity
policy; hard cross-endpoint movement still requires a shared explicit
continuity domain.

## Closeout Condition

This lane can close when:

- v2 compact classification is implemented and logged,
- v2 compact uses the existing state-bound continuity policy,
- targeted tests prove routing and logging behavior,
- docs explain how v2 compact appears in logs,
- and validation gates pass.

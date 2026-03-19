# Backend Gap Matrix

> Quick read: the backend control plane is no longer missing the core operator primitives. The remaining gaps are mostly in productization, semantic closure, and external-client stability rather than raw CRUD coverage.

## Why This Document Exists

The current workstream already tracks design, milestones, and closeout status. This document narrows the question to one backend-facing concern:

- what operator capabilities already exist in the control plane
- what still remains partial
- what is still missing before the backend can be considered externally stable for a future WebUI or non-GUI client

This is intentionally backend-first. GUI convenience and layout concerns are out of scope except where they reveal a missing control-plane contract.

## Executive Summary

The backend is already strong enough for the current product shape:

- session identity and effective route are first-class
- session-scoped overrides cover `profile`, `station`, `model`, `reasoning_effort`, and `service_tier`
- provider/station/profile management is remotely controllable through the v1 control plane
- retry, breaker, drain, healthcheck, and guarded failover are already implemented
- LAN / Tailscale shared-relay constraints are explicitly modeled
- control-trace logging exists and is already consumable by the GUI

The main remaining backend gaps are:

1. broader client adoption of the documented operator-summary contract beyond the first built-in GUI attach consumer
2. stronger remote attach access control beyond the current lightweight token gate
3. longer-horizon audit/export semantics beyond runtime snapshot plus recent control-trace reads
4. final terminology closure so external clients do not need to reason about both `config` and `station`

## Capability Matrix

### 1. Runtime Identity and Session Targeting

Status: **ready for current scope**

Already in place:

- session identity cards are first-class API/runtime objects
- effective route attribution exists
- single-session and session-list operator flows already exist
- runtime session overrides are aggregated and visible

Why this matters:

- this closes the core product need of "what session am I actually controlling"
- this is the precondition for meaningful per-session override and profile application

Remaining gap:

- remote non-host devices still cannot magically access host-local Codex transcript/session files
- that limitation is intentional and should remain explicit unless a future companion/export mode is introduced

Recommendation:

- keep the current backend model
- avoid pretending that remote devices have host-local session-file parity

### 2. Fast Mode / Model / Reasoning / Service Tier Control

Status: **ready, with one semantic polish gap**

Already in place:

- profile CRUD and inheritance exist
- default profile override exists
- session-scoped overrides exist for `model`, `reasoning_effort`, and `service_tier`
- request/session observability already records requested vs effective vs actual `service_tier`
- GUI and control-trace summaries already treat priority service tier as the operator-facing "fast mode" signal
- backend summary/profile payloads now expose a lightweight `fast_mode` alias where it matters most:
  - `operator_summary.runtime.default_profile_summary.fast_mode`
  - `ControlProfileOption.fast_mode` on profile list/snapshot surfaces

Remaining gap:

- the backend still stores "fast mode" as `service_tier=priority`, not as an independent persisted control dimension
- that is acceptable today, but future external clients may still want a clearer runtime/session-level alias beyond profile-oriented summaries

Recommendation:

- do not add a separate storage model yet
- treat the current `fast_mode` alias fields as the presentation contract, and only widen them further if a future client needs a session/runtime-level shortcut

### 3. Provider / Station / Profile Registry Management

Status: **ready for operator use**

Already in place:

- persisted station config API exists
- persisted station structure API exists
- persisted provider structure API exists
- profile mutation API exists
- station-first snapshot/runtime payloads already exist
- `operator/summary` now carries top-level session/station/profile/provider catalogs plus lightweight aggregate counts for read-side clients

Remaining gap:

- write/edit flows are still intentionally split across normalized CRUD endpoints
- external clients still need deeper endpoints for editing, observability, and persisted structure management

Recommendation:

- keep the current normalized CRUD APIs
- treat `operator/summary` as the read-side home payload instead of asking each client to rebuild station/profile/provider runtime context

### 4. Retry / Failover / Breaker / Health

Status: **ready for current product target**

Already in place:

- persisted retry config CRUD exists
- resolved retry policy is exposed
- same-station failover is explicit
- cross-station failover before first output is implemented and guarded
- breaker/drain/half-open states exist
- active healthcheck and manual probe exist

Remaining gap:

- post-output cross-station failover is still intentionally unsupported
- advanced operator policy presets beyond the current retry profiles are still absent

Recommendation:

- treat this as intentionally complete for the current relay product
- only reopen it if a concrete operator scenario requires more aggressive automation

### 5. LAN / Tailscale Shared Relay and Remote Attach

Status: **ready for the current single-operator/shared-relay model**

Already in place:

- remote admin capability disclosure exists
- host-local capability disclosure exists
- lightweight remote-admin token gating exists
- the backend cleanly separates shared control-plane visibility from host-local transcript/history access

Remaining gap:

- access control is still intentionally minimal
- there is no per-device token, role model, or richer remote client identity policy yet

Recommendation:

- this is acceptable for the current personal/LAN product shape
- do not overbuild account/platform semantics yet
- if multi-device usage grows, the next backend step should be per-device identity and audit, not generic OAuth

### 6. Observability, Logging, and Control Trace

Status: **partial**

Already in place:

- control-trace logging exists
- GUI can read control-trace data from local file, attached API, or attached fallback-local path
- request-level summaries already expose retry and routing signals
- service-tier outcome is already preserved

Remaining gap:

- no durable long-horizon audit model beyond the current runtime-oriented surfaces
- no backend-exported operator digest tailored for external dashboards
- no stronger retention/rotation/query story for control-trace beyond the current local file plus recent reads

Recommendation:

- keep control-trace as the current source of truth
- add retention/export/query work only when a concrete external dashboard or ops workflow needs it

### 7. External Client / WebUI Readiness

Status: **partial but unblockable**

Already in place:

- `capabilities` and `snapshot` APIs already describe what a client can safely do
- `operator/summary` now provides a consolidated backend-facing operator home payload for runtime target, lightweight session identity cards, station/profile/provider catalogs, default profile summary, retry posture, lightweight health/failover posture, remote-admin status, and top-level counts
- `OPERATOR_SUMMARY_CONTRACT.md` now documents the read-side home payload shape, layering rules, and compatibility boundaries for future clients
- built-in GUI attach refresh now consumes `operator/summary.links` for follow-up snapshot/retry/spec/control-trace reads instead of rebuilding that follow-up map purely from hardcoded paths
- v1 attach behavior and compatibility expectations are now explicit
- remote-safe capability boundaries are already part of the contract

Remaining gap:

- more clients still need to combine `operator/summary` with deeper detail endpoints for full editing and observability flows
- `operator/summary` is now strong enough to be the top-level read entry point, but it is not intended to replace station/provider spec CRUD or deep request/session observability endpoints
- the remaining terminology edges are now mostly limited to compatibility-only aliases and historical docs/examples

Recommendation:

- before building a broader WebUI, treat `operator/summary` plus `OPERATOR_SUMMARY_CONTRACT.md` as the top-level entry point instead of letting each client rebuild the same runtime explanation logic
- finish station-first terminology closeout before calling the backend externally stable

## What Is Actually Left Before Backend Closeout

If we narrow the remaining backend work to the items that materially block semantic closeout, the list is short:

1. finish the remaining terminology cleanup from `config` to `station`
2. define whether "fast mode" needs an explicit summary alias in backend contracts
3. decide whether remote shared-relay use needs stronger per-device identity/auth
4. roll future clients onto the documented `operator/summary` layering contract without reintroducing duplicated runtime-explanation logic
5. decide whether control-trace needs durable retention/export semantics beyond the current local-file model

## Recommended Order

1. Finish terminology closeout first.
2. Adopt the documented operator-summary contract in future WebUI/backend clients.
3. Revisit remote auth and longer-horizon audit only when there is real multi-device/operator pressure.

## Bottom Line

The backend is already beyond "feature missing" territory.

The refactor is now mostly waiting on:

- semantic polish
- better backend composition for future clients
- a deliberate decision on how far remote/shared usage should go

That means the current best investment is not more scattered CRUD. It is:

- keeping the control-plane language coherent
- reducing duplication in how clients explain the current runtime target
- only adding new backend surfaces when they remove real client complexity

# Design: Codex Relay Live Smoke Diagnostics

Status: Active
Last updated: 2026-05-19

## Why This Lane Exists

Capability diagnostics now tell us what Codex should expect from a relay and what endpoint shapes are visible through validation-only probes. That is intentionally cheap and safe, but it cannot prove that an upstream relay can actually complete official-experience flows such as remote `/responses/compact` or hosted `image_generation`.

Operators need a stronger, explicitly acknowledged live check for the cases where a relay claims support but Codex still behaves differently. The check must stay manual and bounded so it does not become another retry storm, cost source, or hidden scheduler input.

## Problem

The current `POST /__codex_helper/api/v1/codex/relay-capabilities` path sends:

- `GET /models` read-only,
- `POST /responses` with `{}` validation-only,
- `POST /responses/compact` with `{}` validation-only.

This proves endpoint presence and response shape, not successful official behavior. It cannot answer:

- Can this relay actually return a compaction item from `/responses/compact`?
- Can this relay accept a Codex-shaped `/responses` request with hosted `image_generation` tooling?
- Did a live attempt fail because of auth, entitlement, relay schema translation, model mismatch, or transport?

## Target State

- A separate live-smoke contract exists beside capability diagnostics.
- The live-smoke request requires an explicit opt-in acknowledgement string before any upstream call is made.
- The service targets one selected upstream only.
- The service performs at most one request per selected smoke case, with no route executor, no request ledger write, no route affinity update, no passive health update, and no background retry loop.
- Remote compaction smoke can prove a real `/responses/compact` response shape.
- Image generation smoke can prove a hosted-tool `/responses` request is accepted, and can optionally classify a returned `image_generation_call` when a relay/model actually produces one.
- HTTP and TUI can consume the same core DTOs.
- Documentation names the cost/side-effect risk and makes the feature opt-in only.

## In Scope

- New core module for live smoke DTOs, request builders, response classifiers, and executor.
- Admin API endpoint under `codex` for live smoke.
- TUI Settings entry that requires a deliberate confirmation chord before live smoke starts.
- Tests proving:
  - missing opt-in is rejected without upstream hits,
  - one live compaction request is sent to the selected upstream,
  - hosted image-generation request includes Codex-shaped `image_generation` tool JSON,
  - results are classified without mutating normal routing state,
  - docs and capability manifest expose the new surface.
- Workstream evidence, changelog, and configuration docs.

## Out Of Scope

- Periodic smoke checks.
- Automatic patch-mode mutation.
- Automatic retry/failover across providers.
- Writing generated image artifacts to disk.
- Guaranteeing hosted image generation produces a real image in every smoke run; model behavior may decline to call the hosted tool.
- WebSocket relay smoke.

## Safety Contract

Live smoke is not a health check. It is an operator-triggered diagnostic.

Required invariants:

- `acknowledgement` must equal `run-live-codex-relay-smoke`.
- Default request cases must exclude expensive image generation; image smoke must be explicitly requested.
- Each selected smoke case sends one request only.
- No live smoke result may feed load-balancer state, passive health, route affinity, balance exhaustion, or request retry policy.
- Response bodies stored in DTOs must be summarized and bounded; raw image bytes or large base64 payloads must not be retained.

## Architecture Direction

Use the same upstream target selection as capability diagnostics, but keep live smoke in its own module:

- `codex_relay_capabilities`: cheap expected/observed/recommendation diagnostics.
- `codex_relay_live_smoke`: explicit cost-bearing live verification.

The executor should build direct `reqwest` calls from `UpstreamConfig`, matching Codex request shapes closely enough to reveal relay schema/auth problems. It should not call the normal proxy route executor because that would bring retry policy, request ledger, passive health, and affinity side effects.

## First Implementation Slice

Start with core/API and remote compaction plus hosted-tool request classification. TUI can then call the same service with a clear confirmation flow.


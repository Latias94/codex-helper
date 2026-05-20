# Codex Architecture Deepening — Design

Status: Complete
Last updated: 2026-05-20

## Why This Lane Exists

`codex-helper` now has enough Codex-official relay behavior that the next risk is not a single missing feature, but related protocol, routing, diagnostics, test, and patch semantics spreading across shallow Modules. This lane performs fearless refactoring to make the Codex compatibility layer deeper: smaller Interfaces, more Locality, and better test Leverage.

The five refactors are coordinated because they support one user promise: Codex should get the best local-proxy experience possible through relays without asking users to understand relay internals.

## Target State

1. Session identity semantics are explicit. Header-derived session identity and `prompt_cache_key` affinity fallback stay behavior-compatible, but logs/session cards/route affinity can explain the source.
2. HTTP and Responses WebSocket request preparation share a deeper Module for identity, overrides, body rewriting, route request context, and common failure handling. HTTP body and WebSocket first-frame details become small Adapters.
3. Relay capability and live-smoke checks are case-registry driven. Compact, hosted image, WebSocket, and future cases share metadata, execution, evidence, and recommendation behavior.
4. Proxy integration tests use a reusable harness for upstream capture, route graph setup, failover counters, encoding helpers, WebSocket helpers, and affinity assertions.
5. Codex client patching has a patch-plan seam. Preset/readiness decisions produce a pure patch plan; TOML/Auth/State filesystem effects are execution Adapters.

## In Scope

- Refactor production code and tests without preserving shallow legacy shapes when a deeper Module is clearer.
- Add or adjust data types for session identity source and route-affinity observability.
- Extract common preparation Modules across HTTP and Responses WebSocket while keeping current public behavior.
- Split relay live-smoke/capability logic into a case registry that is easy to extend.
- Build and migrate proxy integration tests onto a harness.
- Split Codex client patch logic into plan calculation and execution Adapters.
- Update docs/workstream evidence and any operator docs affected by changed terminology.

## Out Of Scope

- New user-facing relay features beyond behavior-preserving architecture work.
- Implementing `/responses/compact` fallback, remote compaction v2 support, or hosted tool synthesis.
- Changing default routing policy or retry semantics unless required to preserve existing behavior through a cleaner Interface.
- Rewriting the entire proxy or all tests in one unreviewable step.
- Committing without explicit user approval.

## Architecture Direction

Use vertical slices that each preserve external behavior and end with fresh evidence. Prefer deep Modules whose Interface carries a domain concept:

- `ClientIdentity` / `SessionIdentitySource` rather than loose `Option<String>` where source is semantically relevant.
- `PreparedCodexRequest` or equivalent shared preparation output rather than separate HTTP and WebSocket copies of the same steps.
- `RelayDiagnosticCase` / `RelaySmokeCase` registry rather than switch-heavy case orchestration.
- `ProxyTestHarness` builders for repeated integration setup.
- `CodexPatchPlan` plus filesystem/TOML/Auth execution Adapters.

Apply the deletion test to every extraction. If deleting the Module merely moves complexity into one caller, do not extract it. If deleting it would scatter protocol knowledge across HTTP, WebSocket, diagnostics, and tests, the Module is earning its keep.

## Assumptions

| Assumption | Confidence | Consequence if wrong |
| --- | --- | --- |
| Existing behavior is sufficiently covered by `codex-helper-core` nextest to support fearless internal refactors. | Medium | Add narrower characterization tests before changing a slice. |
| Session identity source can be made observable without changing routing keys. | High | Keep key strings compatible and add source as metadata only. |
| HTTP and Responses WebSocket preparation share enough semantics for a common Module. | High | If transport-specific differences dominate, split only shared identity/override/routing logic. |
| Relay live-smoke cases can be represented by a registry without losing per-case safety prompts. | High | Preserve acknowledgement and cost-bearing warnings as case metadata. |
| Codex patching can be split into pure plan and side-effect Adapters without changing switch behavior. | Medium | Start with characterization tests around current switch/readiness behavior. |

## Closeout Condition

This lane closes when all five refactor slices are implemented or explicitly split into documented follow-on lanes, fresh gates pass, docs/handoff are updated, and the final state gives maintainers more Locality and Leverage than the current shallow shapes.

## Closeout Summary

Closed on 2026-05-20. All five target refactors shipped in this lane:

- explicit session identity source semantics;
- shared HTTP/WebSocket Codex request preparation;
- relay capability/live-smoke case registries;
- first proxy integration-test harness extraction;
- pure Codex patch-plan seam with TOML/auth/state execution adapters.

No compact fallback, remote compaction v2 implementation, or hosted tool synthesis was added. Follow-on work should be opened as new lanes only if it has a narrower product or architecture objective.

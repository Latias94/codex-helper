# Fearless Refactor Workstream: Codex Operator Experience

> 中文速览：本目录承接 `codex-control-plane-refactor` 之后的下一层产品化工作。控制平面已经把 `station / profile / session binding / effective route` 的语义打稳；本 workstream 关注操作者每天真正感知到的体验：TUI 稳定性、GUI 信息架构、请求级可观测、价格/余额、以及直观且安全的自动切换策略。

## Purpose

This workstream turns `codex-helper` from a capable Codex-first relay/control plane into a polished local operator console.

The original product intent remains:

- Keep Codex traffic flowing through a local relay.
- Let users switch provider/station/profile without interrupting their Codex workflow.
- Preserve session continuity unless the operator deliberately chooses a riskier policy.
- Aggregate multiple relay vendors without becoming a generic account marketplace.

The next product step is not "copy `cc-switch` or `aio-coding-hub`". The right target is:

**A Codex-first local operator control plane with excellent observability, cost awareness, and safe route policy controls.**

## Relationship to the Existing Workstream

`docs/workstreams/codex-control-plane-refactor/` is the semantic foundation:

- stations
- profiles
- session bindings
- session-scoped overrides
- effective route source attribution
- health, drain, breaker, and failover semantics
- LAN / remote-safe capability boundaries

This workstream assumes those concepts are the canonical base. It should not rename or re-litigate them unless the existing semantic model is proven wrong.

## Document Map

- `FEARLESS_REFACTOR.md`
  - Refactor doctrine, deletion candidates, compatibility rules, and "do this right" boundaries.
- `DESIGN.md`
  - Target product architecture for request observability, pricing, balances, route decision chains, TUI/GUI parity, and policy UX.
- `MILESTONES.md`
  - Prioritized execution plan with P0/P1/P2 sequencing and acceptance gates.
- `GAP_MATRIX.md`
  - Current capability gap against `repo-ref/cc-switch`, `repo-ref/aio-coding-hub`, and the relevant Codex upstream semantics.

## Current Read

Current strengths:

- The core already has station/profile/session-control semantics.
- Recent request/session state exists in core and is surfaced in TUI/GUI.
- Request JSONL logging and control trace logging already exist.
- `service_tier` observability is stronger than a basic proxy: requested/effective/actual values are represented in request logs.
- Provider usage polling already exists in `crates/core/src/usage_providers.rs` and can mark upstreams as usage-exhausted.

Current weak points:

- TUI rendering has been hardened with full-clear invalidation on resize/page switch, Stations viewport synchronization, consistent table highlight spacing, and compact selected-page-aware header tabs; remaining risk is terminal-emulator-specific smoke coverage.
- Usage metrics and cost calculation now have a core cache-aware path, but price catalog sync / override UX still needs product polish.
- Balance/usage polling is now projected as first-class balance snapshots, but more provider adapters and policy weighting are still needed.
- Request logs and API DTOs expose route/cost/cache facts, and JSONL request log query semantics now live in core `request_ledger`; long-horizon audit/search still needs a durable ledger decision.
- GUI exists, but the operator experience still needs a clearer product contract for requests, costs, balances, and policy editing.

## Working Principle

Prioritize the layers in this order:

1. **Trustworthy display**
   - no TUI ghosts, no misleading status bars, no stale route labels
2. **Canonical observation schema**
   - one request/usage/trace DTO across state, logs, API, TUI, and GUI
3. **Cost and balance truth**
   - cache-aware usage, model pricing, provider multipliers, quota/balance snapshots
4. **Explainable switching**
   - operators can see why the system picked a station, skipped another one, or refused to fail over
5. **GUI/TUI parity**
   - GUI may be richer, but TUI must keep the critical operator loop usable

## Reference Projects

Reference these projects for proven product patterns, not for direct architecture cloning:

- `repo-ref/cc-switch`
  - Desktop provider management, provider presets, balance scripts, usage dashboard, request log table, model pricing tables.
- `repo-ref/aio-coding-hub`
  - Unified gateway, trace-first request logging, provider chain UI, circuit breaker/failover visualization, cost engine, model price sync.
- `repo-ref/codex`
  - Codex-native semantics for model provider, service tier, token usage, cached input tokens, and reasoning output tokens.

## Update Rules

- Keep implementation priority changes in `MILESTONES.md`.
- Keep product/architecture decisions in `DESIGN.md`.
- Keep deletion and compatibility decisions in `FEARLESS_REFACTOR.md`.
- Add new docs only when they reduce ambiguity for implementation.

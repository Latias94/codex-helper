# Workstream: Codex Routing Scheduler Observability Refactor

## Purpose

This workstream defines the next routing execution cleanup after the route
preference and provider concurrency workstreams.

The goal is to keep route graph policy stable while making scheduler runtime
state, upstream throttle outcomes, local concurrency saturation, request
observability, and session/operator metrics flow through one explicit boundary.

## Target Outcome

- Local concurrency saturation remains a scheduler skip, not an upstream
  failure.
- Upstream `429`, `503`, `529`, quota, rate-limit, and capacity responses
  become structured attempt outcomes.
- Retry and failover continue to follow route graph preference, affinity, pins,
  and max-attempt rules.
- Provider/endpoint views show configured limit, effective limit, limit group,
  active count, and saturation.
- TUI/GUI/API session views show token totals and output tokens per second from
  core session snapshots.

## Document Map

- `DESIGN.md`
  - problem, target architecture, diagrams, alternatives, risks, success
    metrics, and implementation plan.
- `TODO.md`
  - proposed implementation ledger.
- `MILESTONES.md`
  - phased delivery gates.
- `EVIDENCE_AND_GATES.md`
  - validation commands and future evidence log.
- `WORKSTREAM.json`
  - machine-readable workstream metadata.

## Boundary

This is a runtime-state and observability refactor. It is not a route graph
policy rewrite.

Any change that alters route preference semantics, fallback stickiness, manual
pin precedence, or v5 route graph compilation should move into a separate
workstream.

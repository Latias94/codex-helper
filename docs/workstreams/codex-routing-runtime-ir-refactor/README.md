# Workstream: Codex Routing Runtime IR

## Purpose

This workstream moves v4 routing from a config-time graph that is flattened into
one legacy `routing` station toward a runtime-first route plan model.

The target is a request-time `RoutePlan` / `RouteCandidate` IR that keeps enough
structure to explain and extend routing decisions without rewriting static
config or hiding provider intent behind station compatibility.

## Problem

The v4 route graph already gives users a better authoring model:

- providers are catalog entries;
- route nodes are named and reusable;
- strategies such as `ordered-failover`, `manual-sticky`, and `tag-preferred`
  are explicit.

The original runtime path compiled that graph through the legacy v2 shape and
then into a single `routing` station. P6 now keeps the loaded v4 graph beside a
direct runtime compatibility config and drives non-pinned v4 requests through
`RoutePlanExecutor`, but older station-oriented surfaces still need cleanup:

- route node paths are not preserved in request execution;
- provider identity is mostly carried as an upstream tag;
- endpoint health, cooldown, balance, and model capability are evaluated after
  graph structure has already been flattened;
- explain output cannot show a full node-by-node decision chain;
- future conditional routing would have to be bolted onto the old station model.

## Goal

Build a behavior-preserving migration path from v4 route graph expansion to a
runtime `RoutePlan` IR.

The end state should let the proxy answer:

1. Which route node selected this candidate?
2. Which provider and endpoint does the candidate represent?
3. Which static rule, runtime signal, or override changed the decision?
4. Why was each skipped candidate skipped?
5. Which compatibility behavior is still active only for legacy configs?

## Document Map

- `DESIGN.md`
  - runtime IR vocabulary, compilation pipeline, execution model, and
    compatibility plan.
- `FEARLESS_REFACTOR.md`
  - what should be removed, what should stay, risk controls, and cleanup rules.
- `MILESTONES.md`
  - phased delivery plan with acceptance gates, including the first
    no-behavior-change phase.

## First Phase Boundary

The first implementation phase must not change request routing behavior.

It should only:

- define the runtime IR types;
- build a shadow `RoutePlan` from the same v4 config inputs;
- prove parity with the current flattened provider order and candidate list;
- add tests and internal inspection helpers where useful;
- leave `lbs_for_request`, retry behavior, cooldown handling, balance demotion,
  model capability skips, and response semantics unchanged.

Any request-path behavior change belongs to a later milestone.

## Non-Goals

- Do not redesign public v4 config syntax in this workstream.
- Do not introduce conditional routing in the first phase.
- Do not remove v2 or legacy station compatibility until the runtime executor
  has parity coverage.
- Do not redesign retry, pricing, usage adapters, or request logging except
  where a route-plan decision needs clearer provenance.
- Do not expose secrets or auth material in route explanation output.

## Success Criteria

- v4 route graph structure survives into a runtime representation.
- The first phase can be merged with no route behavior change.
- Existing routing and failover tests still pass.
- New parity tests prove the IR produces the same candidate order as the
  current flattening path for supported v4 strategies.
- Later phases have a clear path to conditional routing and richer explain
  output without adding more station-only special cases.

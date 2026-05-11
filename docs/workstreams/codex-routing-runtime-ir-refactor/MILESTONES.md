# Milestones: Codex Routing Runtime IR

## P0 - Workstream Shape

- [x] Define the runtime IR refactor goal.
- [x] Separate runtime IR work from the existing v4 route graph config
  workstream.
- [x] Define the first no-behavior-change boundary.
- [x] Define phased milestones and acceptance gates.

Acceptance:

- the workstream explains why the existing v4 graph is not enough by itself;
- Phase 1 can be started without changing request routing behavior.

## P1 - Shadow RoutePlan IR, No Behavior Change

- [x] Add runtime IR types for `RoutePlanTemplate`, `RoutePlan`,
  `RouteNodePlan`, `RouteRef`, `RouteCandidate`, and `RouteDecisionTrace`.
- [x] Add a pure compiler from `ServiceViewV4` to `RoutePlanTemplate`.
- [x] Preserve provider id, endpoint id, route path, tags, supported models, and
  model mapping in candidates.
- [x] Add parity tests comparing IR candidate order to current
  `resolved_v4_provider_order` behavior.
- [x] Cover one provider, ordered chain, nested route nodes, manual sticky,
  tag-preferred continue, and tag-preferred stop.
- [x] Keep current request routing on the existing `lbs_for_request` path.
- [x] Add no persisted config, admin API, or request-path contract changes.

Acceptance:

- existing routing and failover tests pass unchanged;
- IR candidate order matches current flattened order for all currently supported
  v4 strategies;
- the IR can be inspected in tests without driving production routing;
- public config write output is unchanged.

## P2 - RoutePlanExecutor Parity

- [x] Add a read-only `RoutePlanExecutor` that can iterate `RouteCandidate`
  values without driving production routing.
- [x] Map each candidate to the existing `SelectedUpstream` compatibility shape.
- [x] Add a read-only attempt-order selector that can shadow unsupported-model
  skips and failed-attempt avoidance without sending requests.
- [x] Compare shadow attempt order against the legacy `LoadBalancer` path for
  failover avoidance, unsupported-model skip, all-unsupported exhaustion, and
  same-candidate retry boundaries.
- [x] Add an opt-in request-path shadow diff hook that compares the legacy
  `LoadBalancer` dry-run order with the route executor order without changing
  selected upstreams.
- [x] Add structured control-trace detail for route executor shadow mismatches.
- [x] Preserve current retry, cooldown, unsupported-model skip, and failover
  semantics.
- [x] Keep legacy station/upstream log fields while adding route metadata
  internally.
- [ ] Port or duplicate request-path failover tests to prove route executor
  parity once the executor is ready to drive attempts.

Acceptance:

- current request/response semantics remain unchanged;
- selected provider and upstream order match the legacy path;
- existing failover tests pass through the new executor path;
- route metadata is additive and does not break existing logs or UI.

## P3 - Runtime State Re-Keying

- [x] Introduce provider endpoint keys alongside station/upstream compatibility
  keys.
- [x] Associate passive health, cooldown, and balance summaries with candidates.
- [x] Keep compatibility reads for v2 and legacy station APIs.
- [x] Define migration behavior for existing in-memory state on config reload.

Acceptance:

- v4 runtime state can be explained by provider endpoint identity;
- legacy station state remains available during the migration window;
- no stale state survives provider endpoint layout changes.

## P4 - Decision Trace And Explain APIs

- [ ] Record structured skip reasons for capability mismatch, breaker open,
  runtime disabled, cooldown, usage exhaustion, and missing auth.
- [ ] Include route path in attempt logs and request history.
- [ ] Extend `routing explain` and admin APIs with selected route, candidates,
  and skip reasons.
- [ ] Update GUI/TUI only after the API shape is stable.

Acceptance:

- operators can see why a candidate was selected or skipped;
- explain output distinguishes static config intent from runtime state;
- no auth secrets are exposed.

## P5 - Conditional Routing

- [ ] Add a minimal `conditional` route strategy after executor parity is stable.
- [ ] Start with request fields that are already available before routing:
  model, service tier, reasoning effort, path, method, and headers.
- [ ] Require deterministic defaults and explicit fallback children.
- [ ] Add tests for match, no-match fallback, and invalid condition specs.

Acceptance:

- conditional routing composes with existing ordered fallback;
- conditions are explainable;
- unsupported or ambiguous conditions fail validation instead of being guessed.

## P6 - Legacy Flattening Cleanup

- [ ] Stop using v4-to-v2-to-runtime as the main v4 execution path.
- [ ] Demote the synthetic `routing` station to compatibility only.
- [ ] Remove v4-only UI and API assumptions that expose station concepts as the
  main provider routing model.
- [ ] Update docs and migration notes.

Acceptance:

- v4 runtime execution uses route plan IR directly;
- legacy configs still have a documented compatibility path;
- route graph, runtime execution, and explain output describe the same plan.

## Done When

- v4 route graph structure survives into request execution.
- Runtime decisions are explained from route nodes and provider candidates, not
  reconstructed from a flattened station.
- Existing behavior is preserved until an explicit milestone changes it.
- Conditional routing can be added without another station-model workaround.

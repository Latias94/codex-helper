# Design: Codex Routing Runtime IR

## Current Runtime Shape

The current v4 path is:

1. `ServiceViewV4` stores providers and route nodes.
2. `resolved_v4_provider_order` expands the route graph into a flat provider
   order.
3. `compile_v4_to_v2` creates one compatibility group named `routing`.
4. `compile_v2_to_runtime` creates a `ServiceConfigManager`.
5. `ProxyService::lbs_for_request` builds one or more `LoadBalancer` values from
   station candidates.
6. `execute_provider_chain` and `select_supported_upstream` choose upstreams,
   apply model mapping, and record attempts.

This works, but the route graph is gone before request execution. The runtime
sees stations and upstreams, not route nodes, provider leaves, and endpoint
candidates.

## Target Runtime Vocabulary

### RoutePlanTemplate

A config-derived, reusable route plan shape for a service.

It should be rebuilt when persisted config changes, not for every attempt.

Fields should include:

- service name;
- route graph entry;
- normalized route node table;
- provider catalog references;
- deterministic expanded candidates;
- compatibility metadata for legacy station surfaces.

### RoutePlan

A request-time snapshot produced from `RoutePlanTemplate` plus runtime inputs.

Runtime inputs include:

- session station/provider override;
- global override;
- session profile binding;
- request model and request flavor;
- provider balance snapshots;
- station and upstream runtime state overrides;
- passive health and cooldown state.

The first phase may build `RoutePlan` without using it to route traffic.

### RouteNodePlan

A normalized route node in the runtime tree.

Fields should include:

- route name;
- strategy;
- child refs;
- target ref for `manual-sticky`;
- tag filters for `tag-preferred`;
- `on_exhausted`;
- metadata.

### RouteRef

A typed reference to either a route node or provider leaf.

This should replace string-only references in the execution model after the
first phase.

### RouteCandidate

The atomic runtime candidate the executor can attempt.

Fields should include:

- provider id;
- provider alias;
- endpoint id;
- base URL;
- auth reference;
- provider tags;
- endpoint tags;
- supported models and model mapping;
- route path, for example `main -> monthly_pool -> input`;
- stable candidate index;
- compatibility station name and upstream index while legacy surfaces exist.

### RouteDecisionTrace

An ordered explanation of how the plan was evaluated.

It should record:

- selected route node;
- selected provider and endpoint;
- skipped candidates and skip reasons;
- runtime signals used, such as cooldown, breaker state, balance exhaustion, and
  capability mismatch;
- override source, such as session override, global override, profile default,
  or runtime fallback.

Skip reasons should be machine-readable strings, not formatted prose.

## Strategy Semantics

### ordered-failover

Preserve child order exactly. Nested route nodes expand depth-first in child
order.

Phase 1 parity requirement: the generated candidate order must match the
current `resolved_v4_provider_order` output.

### manual-sticky

Resolve `target` first. If `target` is absent, fall back to the first child only
where current behavior already does that.

Phase 1 parity requirement: disabled targets and missing targets must fail or
fallback exactly as they do today.

### tag-preferred

Split children into preferred and fallback groups based on provider tags.

Phase 1 parity requirement:

- `on_exhausted = continue` yields preferred candidates followed by fallback
  candidates;
- `on_exhausted = stop` yields only preferred candidates;
- missing preferred matches still fail where current validation fails.

### conditional

Conditional routing is not part of Phase 1.

The IR should reserve a strategy extension point so later work can evaluate
conditions over request metadata, such as model, path, headers, service tier,
reasoning effort, session metadata, or project identity.

## Compilation Pipeline

### Phase 1 Shadow Pipeline

The first phase should run next to the current path:

```text
ServiceViewV4
  -> existing resolved_v4_provider_order
  -> existing v2 compatibility station
  -> existing request routing behavior

ServiceViewV4
  -> RoutePlanTemplate
  -> RoutePlan parity checks and optional explain/debug data
```

The second branch must not influence request routing yet.

### Later Runtime Pipeline

The later target path should be:

```text
ServiceViewV4
  -> RoutePlanTemplate
  -> request-time RoutePlan
  -> RoutePlanExecutor
  -> selected RouteCandidate
  -> upstream request execution
```

Legacy v2 configs can continue to compile into a compatibility `RoutePlan`
template where each station is treated as a route candidate group.

## First Phase No-Behavior-Change Contract

Phase 1 must not change:

- selected provider order;
- selected upstream order;
- session or global override precedence;
- default profile binding behavior;
- model override and model mapping behavior;
- unsupported model skip behavior;
- balance exhaustion demotion behavior;
- runtime state override behavior;
- retry, cooldown, and failover behavior;
- response status and body semantics;
- public config write format.

Phase 1 may add:

- internal IR structs;
- pure conversion functions from v4 service view to route plan template;
- parity tests;
- debug-only or test-only explanation helpers;
- documentation and comments that clarify the migration path.

## Compatibility Strategy

The compatibility station should not disappear immediately.

Use it as a bridge until:

- the IR can reproduce current v4 provider order;
- the executor can attempt candidates from IR with the same observable behavior;
- control-plane and UI surfaces can show provider and route-node identity without
  relying on `station_name = "routing"`;
- request logs can retain existing fields while adding route path and provider
  fields.

For v2 and legacy configs, the runtime may synthesize route plans from station
groups. That compatibility path should be explicit and labeled as legacy.

## Candidate Identity

Candidate identity should be based on provider and endpoint, not on the flattened
station/upstream index alone.

Recommended stable key:

```text
service / provider_id / endpoint_id
```

During migration, also retain:

```text
service / station_name / upstream_index
```

This keeps existing logs and state maps usable while allowing future health and
balance state to move to provider endpoint keys.

P3 starts by materializing this as `ProviderEndpointKey`,
`LegacyUpstreamKey`, and `RuntimeUpstreamIdentity`. Each route candidate can now
report both its future provider-endpoint identity and its current compatibility
station/upstream identity before runtime state is re-keyed.

## Runtime Signal Layers

Route planning should keep these layers separate:

- static graph intent: route nodes, children, strategy, tags;
- request intent: model, service tier, reasoning effort, path, session;
- operator overrides: session override, global override, runtime provider state;
- passive health: failure counts, cooldown, breaker state;
- balance and quota: known exhaustion, stale, unknown, error;
- capability: supported models and model mapping;
- retry policy: whether to retry same upstream, same provider, or next provider.

No layer should rewrite another layer. For example, balance exhaustion may demote
or skip a candidate at runtime, but it must not rewrite route graph config.

## Explainability

The eventual `routing explain` and admin APIs should be able to return:

- route graph entry;
- node path for each candidate;
- candidate provider and endpoint;
- current runtime state summary;
- selected candidate;
- skipped candidates with structured skip reasons;
- fallback reason when moving to the next candidate.

Phase 1 only needs enough inspectability to prove parity.

## Testing Strategy

Phase 1 tests should cover:

- one provider;
- ordered provider chain;
- nested monthly pool before paygo;
- manual sticky target;
- tag preferred continue;
- tag preferred stop;
- disabled provider behavior;
- missing route reference;
- cycle detection remains handled by existing validation;
- duplicate provider leaf behavior remains deterministic;
- parity between current provider order and IR candidate order.

Later tests should cover:

- route executor parity with current failover tests;
- route decision trace contents;
- runtime state and balance skip reasons;
- model capability mismatch skip reasons;
- legacy v2 synthesized route plans.

## Open Questions

- Should duplicate provider leaves remain invalid forever, or should the IR allow
  named duplicate appearances with distinct route paths later?
- Should provider endpoint priority become a route-local policy or remain an
  endpoint ordering property?
- Should `manual-sticky` pin providers only, or should it be allowed to pin route
  nodes once the executor understands node paths?
- Should balance exhaustion demotion be a global candidate sort or a node-local
  policy in the final model?

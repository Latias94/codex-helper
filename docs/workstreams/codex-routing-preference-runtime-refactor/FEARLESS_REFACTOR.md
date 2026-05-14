# Fearless Refactor Notes: Codex Routing Preference Runtime

## What Should Go Away

The final runtime should stop relying on these ideas:

- automatic fallback success is equivalent to user preference;
- one synthetic `routing` station is a valid primary state key for v4 routing;
- `last_good_index` is enough to model route graph affinity;
- session affinity can ignore route-node preference boundaries;
- fallback reuse can be indefinite by default;
- request logs can make operators infer preference state from retry chains;
- compatibility station state should drive new route graph behavior.
- station identity is a public runtime abstraction after migration.

## What Should Stay

These behaviors should remain unless a milestone explicitly replaces them:

- provider catalog and route graph authoring;
- v4 config loading and migration;
- existing retry status and error-class policy;
- explicit manual pins and session/global overrides;
- passive health, cooldown, and usage exhaustion concepts;
- backward-compatible request log fields;
- migration readers for older station-shaped configs.

## Deletion Candidates

After the new executor and state model are proven:

- remove v4 dependence on `LoadBalancer` selection;
- remove v4 use of `LbState.last_good_index`;
- remove the synthetic `routing` station from v5 execution and new APIs;
- replace station/upstream affinity keys with provider endpoint affinity keys;
- replace station pins with route/provider/endpoint pins;
- remove station write APIs from the new public surface;
- remove docs that describe v4 session affinity as whole-route sticky by
  default;
- remove UI wording that presents fallback affinity as a pin.

## Compatibility Rules

- Existing v4 configs must keep loading.
- If a schema bump is used, `config migrate` must be deterministic and must
  write a backup before changing files.
- Old request log fields must remain readable.
- New route explanation fields must use provider, endpoint, route path, and
  preference group as canonical identity.
- Station APIs may remain only as migration commands or explicit removed API
  errors. They must not influence route execution.
- Old fallback-sticky behavior may remain behind an explicit opt-in mode.

## Risk Register

### Cache Locality Regression

Risk: moving back to preferred providers reduces upstream cache hits.

Guardrail:

- keep affinity inside the preferred group;
- allow fallback-sticky as explicit operator policy;
- expose selected affinity mode in explain output.

### Retry Semantics Drift

Risk: route group boundaries accidentally change retry behavior.

Guardrail:

- add parity tests for status retry, transport retry, unsupported model skip,
  cooldown, and usage exhaustion;
- keep retry profiles unchanged unless a milestone explicitly changes them.

### Config Churn

Risk: users see another schema migration without real value.

Guardrail:

- make `version = 5` carry the station retirement and affinity semantics
  change, not just syntax churn;
- preserve v4 route graph topology exactly.

### Compatibility State Leaks Back

Risk: new code continues to read `routing` station state for v5 decisions.

Guardrail:

- add tests that fail when selection changes because compatibility
  `last_good_index` points to a lower-preference provider;
- keep station code in migration-only modules;
- reject station identity in new runtime APIs.

## Refactor Principle

Preference is policy. Affinity is optimization.

The runtime can optimize only after it has preserved the policy chosen by the
route graph.

Station is migration input, not runtime architecture.

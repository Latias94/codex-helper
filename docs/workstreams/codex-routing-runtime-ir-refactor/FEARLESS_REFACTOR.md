# Fearless Refactor Notes: Codex Routing Runtime IR

## What Should Go Away

The v4 runtime should stop relying on these ideas as permanent design:

- v4 route graph semantics are fully represented by one flat provider order;
- all v4 providers belong inside one synthetic station named `routing`;
- `station_name` is the primary runtime identity for v4 provider decisions;
- `upstream_index` is enough to explain a v4 route decision;
- provider identity must be recovered from an upstream `provider_id` tag;
- route explanation can be reconstructed after the graph is flattened;
- future conditional routing can be added cleanly on top of the station-only
  execution path.

## What Should Stay

These parts should remain unless a later milestone proves a cleaner replacement:

- v4 public authoring shape;
- v4 migration from v3 route graph compatible inputs;
- v2 and legacy loading during the migration window;
- existing retry policy and response semantics;
- current passive health and cooldown behavior until route-candidate keyed state
  is ready;
- current balance adapter behavior and trust flags;
- existing request log fields for backward compatibility.

## Phase 1 Hard Boundary

Phase 1 is an IR shadow build only.

Allowed:

- add new pure data structures;
- add conversion from v4 service view to route plan template;
- add parity tests;
- add internal debug helpers;
- document future removal targets.

Not allowed:

- changing `lbs_for_request` selection behavior;
- changing failover ordering;
- changing retry behavior;
- changing cooldown thresholds or backoff behavior;
- changing usage exhaustion fallback behavior;
- changing public persisted config output;
- changing admin API response contracts except for additive internal-only test
  helpers.

## Deletion Candidates

After executor parity is proven, remove or demote these to compatibility code:

- v4-to-v2-to-runtime as the main v4 execution path;
- the synthetic `routing` station as the primary v4 runtime identity;
- route explanation code that only displays flattened order;
- provider runtime state APIs that cannot distinguish route provider identity
  from legacy station identity;
- any v4-only UI label that makes users edit station concepts instead of route
  nodes and provider leaves.

## Compatibility Rules

- Existing v4 configs must keep loading.
- Existing v2 and legacy configs must keep loading during the migration window.
- Existing route behavior must remain unchanged until a milestone explicitly
  switches the executor.
- Existing request logs must keep current fields even after new route fields are
  added.
- Any incompatible public API change needs a migration note and tests.

## Risk Register

### Route Order Drift

Risk: The IR compiler produces a subtly different provider order than the
current flattening path.

Guardrail:

- parity tests compare current provider order with IR candidate order for every
  supported strategy;
- no request path consumes the IR in Phase 1.

### State Key Drift

Risk: Health, cooldown, balance, and usage exhaustion state move to a new key too
early and break existing behavior.

Guardrail:

- keep station/upstream compatibility keys until the executor switch has full
  coverage;
- introduce provider endpoint keys as additive metadata first.

### Explain Output Overpromises

Risk: The UI shows route-node explanations before the executor actually uses
the route-node plan.

Guardrail:

- label Phase 1 output as plan preview or parity debug data;
- only promote it to runtime explanation after executor adoption.

### Conditional Routing Scope Creep

Risk: Conditional routing enters before the static IR and executor are stable.

Guardrail:

- keep conditional routing out of Phase 1;
- add it only after route candidate identity and decision trace are stable.

## Refactor Principle

Do not preserve a compatibility artifact as architecture.

The synthetic station can be a bridge, but the final v4 runtime model should be:

```text
route graph -> route plan -> route candidate -> attempt execution
```

not:

```text
route graph -> fake station -> upstream list -> reconstructed explanation
```

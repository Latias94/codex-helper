# Fearless Refactor Notes

> Historical note: this v3 routing-first refactor plan is superseded by
> `docs/workstreams/codex-routing-graph-refactor/`. Use the v4 route graph docs
> for current authoring and implementation decisions.

## What Should Go Away

The public config surface should stop asking users to reason about:

- `active_station` as the primary conceptual model;
- `level` as the main routing knob;
- station/group structure as the only way to express fallback;
- repeated nested upstream blocks for the common single-endpoint case;
- implicit “monthly” or “cost” inference from balance or vendor name.

## What Should Stay

- provider auth references;
- provider endpoint inventory;
- provider tags;
- named pool boundaries and explicit pool chains for grouped monthly providers;
- deterministic route order;
- explicit route policy;
- explicit fallback behavior;
- the existing runtime compiler, as the target for expansion.

## Compatibility Rules

- Old config files must still load during migration.
- Public writers should emit the new shape.
- Runtime logic should keep using a compiled internal model.
- If a field is ambiguous, prefer refusing to guess rather than silently inventing policy.

## Deletion Candidates

Once the new public surface is in place, the following should be deprecated from the authoring model:

- `active_station` as the main user-editable concept;
- `level` as a routing UI primary control;
- routing presets that only exist to paper over missing route semantics;
- any “monthly primary” behavior that is not backed by a real tag or explicit order.

## Refactor Boundary

This workstream should not:

- redesign request observability;
- redesign balance adapters;
- redesign pricing;
- change the runtime retry engine beyond what is needed to compile the new routing recipe.

Those belong to the operator-experience and control-plane workstreams.

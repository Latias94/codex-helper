# Fearless Refactor Notes

> Historical status (superseded 2026-07-12): this document records a pre-version-5 design and migration phase. The current helper uses `~/.codex-helper/config.toml` with `version = 5`; `codex-helper config migrate` now performs a one-time, validated conversion of supported v1/v2/v3/v4 or unversioned sources, while runtime routing has no legacy compatibility reader. See [current configuration](../../CONFIGURATION.md) and the [canonical modernization plan](../../plans/2026-07-10-002-refactor-canonical-relay-runtime-modernization-plan.md). The remaining content is archival.

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

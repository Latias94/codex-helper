# Fearless Refactor Notes: Codex Routing Graph

## What Should Go Away

The public authoring model should stop asking users to think in terms of:

- one flat `routing.order` as the whole routing story;
- `pool` as a special top-level semantic;
- `chain` and `pools` as final syntax;
- “monthly first” as an implicit tag trick with no route graph boundary;
- balance numbers as a routing policy by themselves;
- hidden fallback behavior that cannot be explained from the config.

## What Should Stay

- provider identity and auth references;
- provider tags and metadata;
- deterministic route expansion;
- explicit route strategy;
- runtime health, cooldown, ejection, and reprobe;
- explainable route decisions;
- config migration that refuses to guess when the graph is ambiguous.

## Compatibility Rules

- Old v3 configs should still load during migration.
- Public writers should emit v4 route graph config.
- The runtime may keep a compatibility loader for v3 during the migration window, but v4 should be the canonical authoring model.
- If the migration cannot infer a safe graph, it should warn or fail rather than inventing policy.

## Deletion Candidates

Once the v4 surface is in place, the following should be removed or downgraded to migration-only support:

- v3 `policy/order/target/prefer_tags` as the end-state authoring model;
- special-case `pool-fallback` syntax;
- `chain` / `pools` route semantics;
- any code path that flattens route structure into a single global provider list before validation;
- UI labels that make the route graph look like a station/proxy artifact instead of a user-authored plan.

## Refactor Boundary

This workstream should not:

- redesign request observability;
- redesign balance adapters;
- redesign pricing;
- redesign the runtime retry engine beyond what the graph compiler needs;
- invent new vendor-specific concepts just to avoid a clean route graph.

Those concerns belong to the operator-experience and control-plane workstreams.

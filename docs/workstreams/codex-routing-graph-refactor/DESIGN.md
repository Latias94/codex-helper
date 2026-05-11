# Design: Codex Routing Graph

## Problem Statement

The current routing surface still compresses too many meanings into one flat order:

- provider catalog;
- route preference;
- fallback chain;
- monthly pool grouping;
- pinning;
- tag preference;
- runtime health and exhaustion.

That shape is workable, but it is not the cleanest target for the local proxy.

The target should be a routing graph:

- providers remain leaves;
- route nodes are named and reusable;
- route nodes can point to providers or other route nodes;
- route strategy is explicit at the node level;
- runtime state stays separate from config.

This is a breaking change from v3, so the schema version should advance to `version = 4`.

## Design Goals

- Keep the public shape small and readable.
- Make route composition explicit.
- Keep provider identity separate from routing intent.
- Preserve deterministic expansion and explainability.
- Treat `unknown` balance as unknown, not as exhaustion.
- Treat 502/429-style failures as runtime health signals, not as permanent config mutations.
- Make the graph expressive enough for future conditional routing without inventing a separate special-case syntax for pools.

## Target Vocabulary

- `provider`
  - a leaf upstream account or endpoint group.
- `route`
  - a named node in the routing graph.
- `entry`
  - the root route name for a service.
- `strategy`
  - how a route node chooses among its children.
- `children`
  - ordered child names, which may reference providers or other routes.
- `metadata`
  - optional tags and hints used by routing strategies.
- `runtime state`
  - balance, quota, exhaustion, cooldown, health, reprobe eligibility.

## Recommended Shape

The route graph should look like this:

```toml
version = 4

[codex.providers.input]
base_url = "https://input.example/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.input1]
base_url = "https://input1.example/v1"
auth_token_env = "INPUT1_API_KEY"
tags = { billing = "monthly" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]
```

Rules:

- route names are user-authored identifiers;
- `children` may point to providers or other routes;
- cycles are invalid and must be rejected at load time;
- the compiler expands a route graph into an ordered candidate plan;
- route names such as `monthly_pool` are just names, not a special syntax class;
- if a provider appears in multiple branches, the compiler must either reject the graph or make the duplication rule explicit and deterministic.

## Common Strategies

### `ordered-failover`

Use for a strict priority list.

Good for:

- single provider pinning;
- monthly pool chains;
- paygo last-resort fallback.

### `manual-sticky`

Use for a temporary explicit pin.

Good for:

- debugging;
- incident response;
- “do not move me” sessions.

### `tag-preferred`

Use for semantic preference where tags matter more than names.

Good for:

- `billing=monthly`;
- `region=hk`;
- vendor-class or compliance metadata.

## Future Strategy

### `conditional`

Conditional routing is design intent, not part of the v0.14.0 copy-pasteable
config surface. Add it only after the core graph compiler, CLI, and UI can
explain existing route nodes clearly.

Use it when request metadata or request parameters should pick a branch before
fallback.

Good for:

- EU resident routing;
- model-family routing;
- paid-plan vs free-plan routing;
- test-environment steering.

## Scenario Matrix

| Scenario | Goal | Recommended Shape |
| --- | --- | --- |
| One relay | Keep config tiny | `manual-sticky` or one-node `ordered-failover` |
| Monthly pool + paygo | Keep monthly grouping explicit | nested `ordered-failover` nodes |
| Monthly-first by tag | Prefer metadata over names | `tag-preferred` |
| Region split | Different route by metadata | future `conditional` |
| Strict budget stop | Avoid spillover | route node with stop-on-exhausted behavior |
| Debug pin | Freeze one path temporarily | `manual-sticky` |

## Reference Model Takeaways

The route graph should borrow proven patterns from mature gateway products:

- [LiteLLM routing](https://docs.litellm.ai/docs/routing) shows that fallback, retry, timeout, cooldown, and load balancing belong to routing reliability, not to the provider catalog alone.
- [OpenRouter provider routing](https://openrouter.ai/docs/features/provider-routing) shows that provider ordering, provider allow/ignore lists, and fallback control are request-level routing concerns.
- [Portkey conditional routing](https://portkey.ai/docs/product/ai-gateway/conditional-routing) shows that conditional routing is a first-class graph problem: targets, conditions, and defaults compose cleanly.
- [Envoy outlier detection](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier) shows that outlier detection is runtime state: hosts are ejected, cooled down, and reprobed instead of being permanently rewritten out of config.

## Non-Goals

- Do not keep `pool` as a special top-level semantic.
- Do not keep v3 flattened `policy/order/target` as the public end state.
- Do not infer monthly/paygo from provider names or balance text.
- Do not move runtime health into static config.
- Do not make the graph so flexible that it becomes hard to explain.

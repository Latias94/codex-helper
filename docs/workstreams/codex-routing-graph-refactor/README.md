# Fearless Refactor Workstream: Codex Routing Graph

> 中文速览：本目录定义 `version = 4` 的目标路由图。它不再把 pool 当成特殊概念，而是把 `routing` 变成一张可组合的路由图：provider 仍然是叶子，route node 是内部节点，`entry` 指向根节点。这样可以同时表达单 provider、月包组、paygo 兜底、标签优先、条件分流和临时 pin。

## Purpose

This workstream defines the target public config shape, implementation plan, and refactor gates for the routing graph redesign.

The intended end state is:

- providers stay flat and easy to add;
- routing becomes a named graph of reusable nodes;
- route nodes can reference providers or other nodes;
- runtime health, cooldown, exhaustion, and reprobe remain runtime state;
- user intent is expressed directly instead of being flattened into one global order;
- `version = 4` is a deliberate breaking change with a clean migration path from v3.

## Common User Scenarios

- Single relay user: one provider and a pinned route.
- Monthly-heavy user: several monthly accounts in one reusable route node, with paygo as the last resort.
- Tag-aware user: prefer monthly or region tags without hardcoding vendor names.
- Budget-bound user: stop once the preferred path is truly exhausted.
- Debugging user: pin a known provider or route node for a short session.
- Future policy user: route by request metadata or model family before fallback once conditional routing is added.

## Document Map

- `CONFIGURATION.md`
  - target config recipes for the common user scenarios above.
- `DESIGN.md`
  - route graph model, semantics, validation rules, and reference-model analysis.
- `FEARLESS_REFACTOR.md`
  - deletion candidates, compatibility rules, and what must not survive as public authoring surface.
- `MILESTONES.md`
  - phased implementation order and acceptance gates.

## Reference Projects

Use these for proven patterns, not for direct cloning:

- LiteLLM Router and fallbacks
  - explicit routing, fallbacks, retries, cooldowns, and load balancing
- OpenRouter provider routing
  - request-level provider ordering, fallback control, and parameter filters
- Portkey conditional routing
  - composable targets, conditions, and defaults
- Envoy outlier detection
  - runtime ejection, cooldown, degraded state, and reprobe as operational behavior

## Working Principle

The routing graph should answer four questions cleanly:

1. Which route node is the entry point?
2. Which route node or provider comes next?
3. Which decisions are static config and which are runtime health state?
4. Why did the compiler choose that candidate?

## Update Rules

- Keep the canonical target shape in `DESIGN.md`.
- Keep user-facing recipes in `CONFIGURATION.md`.
- Keep deletion and compatibility decisions in `FEARLESS_REFACTOR.md`.
- Keep milestone priority changes in `MILESTONES.md`.
- Avoid adding syntax just to preserve old wording; the graph should stay honest and simple.

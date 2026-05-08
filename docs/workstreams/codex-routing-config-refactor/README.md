# Fearless Refactor Workstream: Routing Config Surface

> 中文速览：这个 workstream 负责把“用户手写的配置面”从“运行时展开模型”里剥离出来。目标不是再堆一个 `active provider` 字段，而是把“谁是当前激活对象”重定义为 `active routing`：默认路由、顺序、标签优先和兜底规则都收敛到一个更薄、更直观的 `routing` 配置块里。

## Purpose

This workstream defines the public configuration surface for provider routing.

The intended end state is:

- providers stay thin and easy to add;
- routing is explicit and readable;
- tags remain operator metadata, not hidden magic;
- migration from the current `active / level / station` shape is automatic and deterministic;
- GUI/TUI edit the same semantic model that config files use.

## Relationship To Existing Workstreams

- `docs/workstreams/codex-control-plane-refactor/`
  - owns the canonical runtime semantics for sessions, stations, profiles, and route attribution.
- `docs/workstreams/codex-operator-experience-refactor/`
  - owns observability, policy previews, balance/cost UX, and the operator-facing console.

This workstream sits between them:

- it simplifies the user-authored config shape;
- it compiles that shape into the current runtime routing model;
- it keeps the UI and the config file aligned.

## Document Map

- `DESIGN.md`
  - target config shape, example policies, and migration strategy.
- `CONFIGURATION.md`
  - user-facing routing-first configuration guide and practical recipes.
- `FEARLESS_REFACTOR.md`
  - deletion candidates, compatibility rules, and what must not survive in the public authoring model.
- `MILESTONES.md`
  - implementation order and acceptance gates.

## Current Read

Current runtime and persisted config already have useful building blocks:

- `providers` can hold auth, tags, and endpoint inventory.
- `stations` / `groups` can still represent routing candidates internally.
- `tags` already exist and can carry operator meaning such as `billing=monthly`.
- `preferred` already exists on grouped members, which makes fallback order deterministic.
- `active_station` is already a legacy runtime selector and should be treated as a migration source, not the public authoring model.

Current friction:

- a single provider with a single endpoint still takes too many nested fields;
- `active_station` plus `level` reads like an implementation detail, not a user policy;
- users have to think in terms of station grouping even when they only want “try this provider first, then fall back”.

## Target Outcome

The public config should feel like this:

- define providers once;
- mark them with tags;
- declare one routing recipe per service;
- choose a policy such as `manual-sticky`, `ordered-failover`, or `tag-preferred`;
- migrate the old `active provider` / `active station` concept into `active routing`;
- let the compiler expand that recipe into the existing runtime routing model.

## Update Rules

- Keep the authoritative design in `DESIGN.md`.
- Keep migration and deletion decisions in `FEARLESS_REFACTOR.md`.
- Keep milestone priority changes in `MILESTONES.md`.
- Do not duplicate the runtime routing implementation details here; this workstream is about the public config surface.

# Fearless Refactor Vocabulary Contract

> 中文速览：这份文档把控制平面的核心名词固定下来，用来关闭 `CP-000` 和 `CP-001`。之后无论是 API、GUI/TUI、配置模板还是迁移文档，都应该先服从这里的规则：哪些词是 canonical，哪些词只能作为兼容层或历史材料出现。

## Purpose

This document defines the control-plane vocabulary contract for the refactor.

It exists to:

- close `CP-000` by auditing the ambiguous terms that were overloaded in the legacy product shape
- close `CP-001` by mapping legacy wording onto explicit target concepts
- give API/UI/runtime/docs/template work a shared rulebook for future edits

This document does **not** mean every historical example must be rewritten immediately.
It defines what is canonical now, and what is only tolerated as compatibility material.

## Canonical Terms

| Term | Use it for | Notes |
| --- | --- | --- |
| `station` | A routing target that the operator can enable, disable, drain, prioritize, and fail over across. | This replaces most legacy runtime uses of `config`. |
| `provider` | Shared auth, tags, capabilities, and endpoint inventory. | Stations reference providers through station members. |
| `endpoint` / `upstream` | A concrete `base_url` target under a provider. | `upstream` is still acceptable in lower-level routing/logging language. |
| `profile` | Reusable session intent such as fast/deep/default routing behavior. | A profile may set `station`, `model`, `reasoning_effort`, or `service_tier`. |
| `session binding` | The sticky control-plane attachment of a session. | This is the main continuity concept for a session. |
| `session override` | An explicit per-session control value above defaults. | Runtime-scoped by policy unless stated otherwise. |
| `global override` | An explicit runtime override above persisted defaults for the whole service/runtime. | Example: global station pin/override. |
| `observed session` | Session data derived from proxy traffic only. | This is remote-safe and should exist for LAN/Tailscale clients. |
| `enriched session` | Optional host-local augmentation such as transcript path or local `cwd`. | Never assume this exists remotely. |
| `effective route` | The resolved station/upstream/model/service-tier/reasoning result plus source attribution. | This answers "what is this session actually using?" |
| `active_station` | The persisted default station for a service. | Prefer `configured_active_station` / `effective_active_station` when snapshot semantics matter. |
| `persisted config file` / `persisted config document` | The on-disk `config.toml` / `config.json` representation. | This is the main remaining valid use of bare `config`. |

## Compatibility-only or Restricted Terms

### `config`

Allowed uses:

- the literal persisted files `config.toml` / `config.json`
- the Config page/editor/workspace when it literally edits persisted configuration documents
- compatibility inputs/aliases such as:
  - `codex.configs.*`
  - `profile.config`
  - `/stations/config-active`
  - `station_persisted_config`
- historical docs/examples/tests that intentionally explain migration from the old layout

Do not use bare `config` for:

- new runtime model names
- new operator-facing route/state labels
- new canonical API field names
- new home/dashboard summaries where `station` is the real concept

When legacy `config` appears, map it to one of these explicit meanings:

- `station` when it means a routing target
- `profile` when it really means reusable control intent
- `persisted config file/document` when it literally means disk representation
- `legacy config` only when discussing migration or compatibility behavior

### `active`

Use bare `active` only when quoting or parsing a legacy field.

Prefer:

- `active_station`
- `configured_active_station`
- `effective_active_station`
- `enabled`

### `pinned`

`pinned` is still valid, but only for continuity/stickiness semantics.

Allowed uses:

- global pinned station
- pinned route/failover explanations
- session continuity explanations where a session should stay on its chosen route

Do not use `pinned` as a substitute object name for a station/profile/config.
If scope matters, prefer `session binding`, `global pinned station`, or `session station override`.

### `override`

Use `override` only for explicit higher-precedence control values.

Valid examples:

- session override
- global override
- runtime enabled override
- runtime level override

Do not use `override` for:

- persisted defaults
- profile inheritance
- ordinary config loading

### `session`

Use bare `session` only when the meaning is already obvious from context.
When ambiguity matters, qualify it:

- `observed session`
- `enriched session`
- `session binding`
- `session identity card`

## Legacy-to-target Mapping

| Legacy wording | Canonical target | Where legacy wording may still appear |
| --- | --- | --- |
| `config` / `configs` | `station` for routing targets; `persisted config file/document` for disk format | compatibility loaders, migration docs, historical examples |
| `active` | `active_station` or a more specific field such as `configured_active_station` / `effective_active_station` | legacy TOML input such as `active = "true"` |
| `pinned config` | `session binding`, `global pinned station`, or `session station override` depending on scope | historical docs and some continuity explanations |
| `session config override` | `session station override` | compatibility comments/tests only |
| `profile.config` | `profile.station` | deserialize-only compatibility for older files |
| `config-active` | persisted active-station compatibility alias | hidden compatibility routes only |

## Writing Rules for New Work

- New public APIs must emit station-first names even if they deserialize legacy aliases.
- GUI/TUI operator text should prefer `station`, `provider`, `profile`, `binding`, `override`, and `effective route`.
- Config templates may mention legacy `configs` only inside explicit compatibility or migration notes.
- Historical docs/examples that retain legacy wording should say that they are historical or compatibility-oriented.
- When a sentence could mean either runtime state or persisted file structure, prefer the explicit phrases:
  - `runtime station`
  - `persisted station settings`
  - `persisted config file`
  - `session binding`

## Closeout Note

This document closes the audit/mapping decision for `CP-000` and `CP-001`.

The remaining `P0` work is narrower:

- apply this contract consistently across the last compatibility-only wording/export surfaces
- keep legacy aliases explicit and hidden where appropriate
- finish the remaining docs/examples/template cleanup under `CP-002` / `CP-401`

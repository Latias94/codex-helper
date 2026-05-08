# Routing-First Configuration Guide

> This document describes the target authoring model for the routing refactor.
> The current branch can load and write `version = 3` routing-first config, `config init` emits a v3 template, and legacy station-first config remains readable for migration.
> The goal is simple: keep providers thin, make routing explicit, and let most users configure the system without learning internal station/group mechanics.

## Mental Model

- `providers` is the catalog: identity, auth, endpoint inventory, and tags.
- `routing` is the active route recipe: policy, order, and exhaustion behavior.
- tags are metadata, not hidden policy.
- order is deterministic fallback.
- policy decides how to interpret the routing inputs.

## Minimal Shape

```toml
version = 3

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"
tags = { billing = "paygo", region = "us" }

[codex.routing]
policy = "ordered-failover"
order = ["input", "backup"]
on_exhausted = "continue"
```

This is the default mental model we want:

1. define providers once;
2. give them useful tags;
3. declare a routing recipe;
4. let the compiler expand that recipe into the runtime routing model.

## Policies

### `ordered-failover`

Use this when you want the most predictable setup.

```toml
[codex.routing]
policy = "ordered-failover"
order = ["monthly", "paygo"]
on_exhausted = "continue"
```

Behavior:

- try providers in the order you wrote;
- move to the next provider when the current one is exhausted or unavailable;
- keep going until one works or the list is empty.

This should be the default for most users.

### `manual-sticky`

Use this when you want a pinned target and do not want automatic switching.

```toml
[codex.routing]
policy = "manual-sticky"
target = "input"
```

Behavior:

- always prefer the selected target;
- do not reorder around it automatically;
- if the target is unavailable, fail according to the exhaustion rule.

### `tag-preferred`

Use this when you want a semantic preference such as monthly-first routing.

```toml
[codex.providers.monthly]
base_url = "https://monthly.example/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "continue"
```

Behavior:

- prefer providers that match the requested tags;
- still keep a deterministic order;
- use explicit tags instead of inferring “monthly” from balance or vendor naming.

## Exhaustion Behavior

`on_exhausted` should stay explicit.

- `continue`: keep falling back to the next provider.
- `stop`: fail fast once the preferred set is depleted.

Recommended usage:

- `continue` for availability-first setups;
- `stop` for budget-bound monthly usage where a hard failure is better than silent drift.

## Provider Fields

| Field | Meaning | Notes |
| --- | --- | --- |
| `base_url` | Main endpoint for a single-endpoint provider | Use the inline shorthand unless you really need multiple endpoints. |
| `endpoints` | Additional endpoint inventory | Expand only when a provider genuinely has more than one target. |
| `auth_token_env` / `api_key_env` | Secret reference | Prefer environment variables over in-file secrets. |
| `tags` | Free-form metadata | Good for `billing`, `region`, `vendor`, and similar hints. |
| `enabled` | Provider availability | Useful for temporarily disabling a provider without deleting it. |
| `alias` | Optional display label | Keep it short; do not duplicate the provider key unless needed. |
| `supported_models` | Model allowlist | Advanced metadata only. |
| `model_mapping` | Model name translation | Advanced metadata only. |

## Routing Fields

| Field | Meaning | Notes |
| --- | --- | --- |
| `policy` | Routing policy | Required. Choose `manual-sticky`, `ordered-failover`, or `tag-preferred`. |
| `order` | Deterministic fallback order | Use explicit provider keys. |
| `target` | Pinned provider target | Used by `manual-sticky`. |
| `prefer_tags` | Preferred tag filters | Used by `tag-preferred`. |
| `on_exhausted` | Exhaustion behavior | Use `continue` or `stop`. |

## Common Recipes

### Single Provider

```toml
[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
policy = "manual-sticky"
target = "main"
```

Best when you only have one relay and want the smallest possible config.

### Primary + Backup

```toml
[codex.providers.primary]
base_url = "https://primary.example/v1"
auth_token_env = "PRIMARY_API_KEY"

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"

[codex.routing]
policy = "ordered-failover"
order = ["primary", "backup"]
on_exhausted = "continue"
```

Best when you want a first choice and a clear fallback path.

### Monthly First

```toml
[codex.providers.monthly]
base_url = "https://monthly.example/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "continue"
```

Best when monthly quota should be used first, but you still want a fallback.

### Hard Budget Stop

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "stop"
```

Best when you would rather fail than spill into a non-monthly provider.

## CLI Editing Notes

`provider` and `routing` are the canonical CLI surfaces for `version = 3` routing files.

Use `provider` for catalog edits: identity, auth reference, base URL, enabled state, and tags.

```bash
codex-helper provider list
codex-helper provider add input --base-url https://ai.input.im/v1 --auth-token-env INPUT_API_KEY --tag billing=monthly --tag region=hk
codex-helper provider show input
codex-helper provider enable input
codex-helper provider disable input
```

- `provider list` shows the v3 provider catalog in current routing order; `--json` emits machine-readable provider metadata without plaintext secrets.
- `provider add` writes `[codex.providers.<name>]` using the inline `base_url` shorthand and appends the provider to `routing.order`.
- `provider add --replace` overwrites an existing provider explicitly.
- `provider enable` marks the provider routeable and keeps it in the routing order.
- `provider disable` marks the provider unavailable for automatic routing; if it was the manual target, the command clears that target and returns to ordered failover.

Use `routing` for policy edits: pinning, fallback order, tag preference, and exhaustion behavior.

```bash
codex-helper routing show
codex-helper routing pin input
codex-helper routing order input backup
codex-helper routing prefer-tag --tag billing=monthly --order monthly,paygo --on-exhausted continue
codex-helper routing clear-target
```

- `routing show` prints the current policy, target, order, preferred tags, and provider references; `--json` returns the same structured shape as the v3 routing API.
- `routing pin input` writes `policy = "manual-sticky"` and `target = "input"`, while keeping the full provider order available for later unpinning.
- `routing order input backup` writes `policy = "ordered-failover"` and promotes the listed providers, then appends any remaining providers so they are not accidentally dropped.
- `routing prefer-tag --tag billing=monthly --order monthly,paygo` writes `policy = "tag-preferred"` and keeps fallback order explicit.
- `routing clear-target` removes the manual target and returns to ordered failover.
- `routing set` is the low-level patch command for advanced edits: `--policy`, `--target`, `--order`, `--prefer-tag`, `--clear-prefer-tags`, and `--on-exhausted`.

The old `station` CLI surface remains for migration and for listing/explaining older configs:

- `station list` shows v3 providers plus policy, target, order, and exhaustion behavior.
- `station explain` shows the v3 routing recipe; `--station <name>` is treated as a provider detail selector on v3 files.
- `station add`, `station set-active`, `station enable`, and `station disable` are rejected on v3 files; use `provider` and `routing` instead.
- `station set-level` is rejected for v3; provider priority is `routing.order`.

## Control Plane Editing Notes

Local GUI, remote attach clients, and TUI-backed admin flows should edit the same v3 document instead of writing a compacted v2 projection.

- provider spec writes update `[codex.providers.<name>]` directly;
- new providers are appended to an existing explicit `codex.routing.order`;
- `GET /__codex_helper/api/v1/routing` reads the v3 routing block plus provider references;
- `PUT /__codex_helper/api/v1/routing` is the canonical structured write path for `policy`, `order`, `target`, `prefer_tags`, and `on_exhausted`;
- station quick-switch and station settings APIs are v2-only for persisted station schema and are rejected on v3 files;
- profile CRUD writes `[codex.profiles]` and `default_profile` directly;
- station spec reads/writes are rejected on v3 files; use routing and provider specs instead.

## Migration From Legacy Config

The migration should be deterministic and boring.

Preview migration output:

```bash
codex-helper station migrate --to v3 --dry-run
```

Write the migrated TOML:

```bash
codex-helper station migrate --to v3 --write --yes
```

- `active_station` becomes the routing target for `manual-sticky`, or the first entry in `order` for `ordered-failover`.
- `level` becomes an initial ordering hint, not a user-facing primary control.
- `stations` / `groups` / `members` become provider entries plus routing order.
- `preferred` becomes the first item in the route order or the first item in a provider group.
- explicit tags are preserved.
- inferred business tags such as `billing=monthly` are never guessed.
- existing profile station references are mapped to the generated `routing` target during migration.
- warnings are printed to stderr when v2 station boundaries cannot be represented exactly:
  disabled inactive stations are omitted, disabled active stations stay routeable to preserve runtime fallback, repeated provider references are de-duplicated, and endpoint-scoped station members are called out because v3 routing order is provider-level.

## What We Intentionally Do Not Do

- do not require users to model station groups for one relay;
- do not infer monthly or paygo from balance data;
- do not make speed-based balancing the primary UX before we have real measurements;
- do not keep a per-provider clone of Codex config just to express routing;
- do not make `level` the main authoring knob.

## Recommended Default

For most users:

1. keep providers thin;
2. tag known monthly providers explicitly;
3. use `ordered-failover` as the default policy;
4. use `manual-sticky` only when you really need a pinned target;
5. use `stop` only when exceeding quota must be a hard failure.

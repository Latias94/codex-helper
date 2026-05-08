# Configuration Guide

This guide describes the public `version = 3` configuration model.

The short version: define providers once, then choose one routing recipe. You do not need one Codex config file per relay, and you do not need to model station groups for the common case.

## Mental Model

- `providers` are the catalog: relay identity, auth reference, base URL, optional endpoints, and tags.
- `routing` is the active route recipe: policy, fallback order, tag preference, and exhaustion behavior.
- `profiles` describe reusable session defaults such as model, reasoning effort, and service tier. Provider selection belongs to `routing`.
- `retry` controls how hard the proxy retries inside or across candidates before returning an error.

Runtime and UI screens can still use the word `station` because the internal proxy engine routes through station candidates. For hand-written config, think in `provider` plus `routing`.

## Minimal Config

```toml
version = 3

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo", vendor = "openai" }

[codex.routing]
policy = "ordered-failover"
order = ["input", "openai"]
on_exhausted = "continue"

[retry]
profile = "balanced"
```

This is the recommended default for most users: the monthly relay is tried first, OpenAI is the backup, and routing stays deterministic.

## Provider Fields

Use the inline shape for one endpoint:

```toml
[codex.providers.packy]
alias = "Packy monthly"
base_url = "https://codex-api.packycode.com/v1"
auth_token_env = "PACKYCODE_API_KEY"
tags = { billing = "monthly", vendor = "packy" }
```

Use explicit endpoints only when the same provider has multiple real targets:

```toml
[codex.providers.relay]
alias = "Relay account"
auth_token_env = "RELAY_API_KEY"
tags = { billing = "paygo", vendor = "relay" }

[codex.providers.relay.endpoints.hk]
base_url = "https://hk.relay.example/v1"
priority = 0
tags = { region = "hk" }

[codex.providers.relay.endpoints.us]
base_url = "https://us.relay.example/v1"
priority = 1
tags = { region = "us" }
```

Common fields:

| Field | Meaning | Recommendation |
| --- | --- | --- |
| `base_url` | Main OpenAI-compatible endpoint | Use this for single-endpoint providers. |
| `auth_token_env` | Environment variable for bearer auth | Prefer this over inline secrets. |
| `api_key_env` | Environment variable for `X-API-Key` auth | Use only for providers that require it. |
| `tags` | Free-form provider metadata | Use clear tags such as `billing`, `vendor`, `region`. |
| `enabled` | Whether the provider is routeable | Use `provider disable` instead of deleting temporary backups. |
| `supported_models` | Optional model allowlist | Advanced; leave empty unless the relay is model-limited. |
| `model_mapping` | Optional model name translation | Advanced; use only when a relay needs aliases. |

## Profiles

Profiles are optional. Use them when you want named request defaults without changing the routing policy.

```toml
[codex]
default_profile = "daily"

[codex.profiles.daily]
model = "gpt-5"
reasoning_effort = "medium"
service_tier = "auto"

[codex.profiles.deep]
extends = "daily"
reasoning_effort = "high"
```

Common fields:

| Field | Meaning | Recommendation |
| --- | --- | --- |
| `extends` | Inherit another profile, then override selected fields | Useful for `daily` plus `deep` variants. |
| `model` | Default model for sessions using the profile | Keep provider capability limits in mind. |
| `reasoning_effort` | Default reasoning effort | Use Codex-supported values only. |
| `service_tier` | Default service tier / fast mode intent | Leave empty unless you deliberately want a tier. |

Do not use profile-level provider selection in new v3 config. Legacy profile `station` bindings are migration-only; `routing` is the durable provider-selection surface.

## Routing Policies

### `ordered-failover`

Best for "try this relay first, then this backup".

```toml
[codex.routing]
policy = "ordered-failover"
order = ["monthly", "paygo", "openai"]
on_exhausted = "continue"
```

Evaluation:

| Dimension | Result |
| --- | --- |
| Clarity | Highest. The file order is the fallback order. |
| Availability | Good when backups exist. |
| Cost control | Explicit but not semantic; put cheaper providers first. |
| UI fit | Very easy: reorder a list. |

Use this as the default unless you need a pinned target or tag preference.

### `manual-sticky`

Best for "do not switch unless I manually change it".

```toml
[codex.routing]
policy = "manual-sticky"
target = "input"
order = ["input", "openai"]
```

Evaluation:

| Dimension | Result |
| --- | --- |
| Clarity | High. One visible target is active. |
| Availability | Lower. The pinned target can fail instead of silently moving. |
| Cost control | High if the target is deliberate. |
| UI fit | Easy: a provider picker with a clear "pinned" state. |

Use this for debugging, strict vendor selection, or temporary manual steering.

### `tag-preferred`

Best for "prefer all monthly providers, then use backups".

```toml
[codex.providers.monthly-a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly-b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly-a", "monthly-b", "paygo"]
on_exhausted = "continue"
```

Evaluation:

| Dimension | Result |
| --- | --- |
| Clarity | Good when tags are named honestly. |
| Availability | Good with `on_exhausted = "continue"`. |
| Cost control | Stronger than raw order because billing intent is explicit. |
| UI fit | Good: users choose a tag filter, then reorder fallback. |

Use this for monthly-first or region-first setups. Do not expect codex-helper to infer "monthly" from a provider name or balance API.

## Exhaustion Behavior

`on_exhausted` controls what happens after the preferred routing set is known to be depleted.

```toml
on_exhausted = "continue"
```

Use `continue` when availability matters more than strict budget isolation. Known fully exhausted candidates are demoted during automatic routing when their balance adapter is trusted.

```toml
on_exhausted = "stop"
```

Use `stop` when you would rather fail than spill into a non-preferred provider, for example monthly-only work.

Balance adapters default to trusting exhausted snapshots for routing. If a provider's balance endpoint is known to return misleading zeroes, set `trust_exhaustion_for_routing = false` in `~/.codex-helper/usage_providers.json`. The raw balance still appears in UI and logs, but it will not demote the route.

## Common Recipes

### One Provider

```toml
version = 3

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
policy = "manual-sticky"
target = "main"
order = ["main"]
```

### Monthly First, Pay-As-You-Go Fallback

```toml
version = 3

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

### Monthly Only, Stop On Exhaustion

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly", "paygo"]
on_exhausted = "stop"
```

This keeps `paygo` visible in the file for later use but prevents silent spillover while the stop rule is active.

### Ordered Relay Chain

```toml
[codex.routing]
policy = "ordered-failover"
order = ["work-relay", "personal-relay", "openai"]
on_exhausted = "continue"
```

This is the most direct replacement for older "priority level" setups.

## CLI Editing

Initialize or migrate:

```bash
codex-helper config init
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Manage providers:

```bash
codex-helper provider add input --base-url https://ai.input.im/v1 --auth-token-env INPUT_API_KEY --tag billing=monthly
codex-helper provider add openai --base-url https://api.openai.com/v1 --auth-token-env OPENAI_API_KEY --tag billing=paygo
codex-helper provider list
codex-helper provider show input
codex-helper provider disable input
codex-helper provider enable input
```

Manage routing:

```bash
codex-helper routing order input openai
codex-helper routing pin input
codex-helper routing prefer-tag --tag billing=monthly --order input,openai --on-exhausted continue
codex-helper routing set --policy ordered-failover --order input,openai
codex-helper routing clear-target
codex-helper routing show
codex-helper routing explain
```

Rules:

- `provider add` appends new providers to `routing.order`.
- `routing order a b c` writes an explicit ordered failover chain.
- `routing pin a` writes `manual-sticky` with target `a`.
- `routing prefer-tag --tag billing=monthly --order a,b` writes `tag-preferred`.
- `routing set` is the low-level patch command for advanced edits.

Use `--claude` on provider/routing commands when editing the Claude service instead of Codex.

`routing show` reads the persisted v3 route recipe. `routing list` and `routing explain` read the compiled runtime candidate view.

## GUI And TUI

Current UI behavior intentionally separates runtime controls from file editing.

- The GUI proxy settings screen accepts v3 routing-first TOML. Legacy station-first files should be migrated first.
- GUI form view summarizes providers, profiles, and routing; detailed edits currently go through raw TOML or CLI commands.
- TUI station switching is runtime-only. It pins or clears the active route for the running proxy and does not write the config file.
- Persistent provider and routing edits should use `provider`, `routing`, or the v3 raw config.

This keeps the UI mental model simple: temporary steering happens in runtime controls; durable intent lives in `[service.providers]` and `[service.routing]`.

## Migration Notes

Legacy v2 station/group files are still readable for migration.

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Migration is intentionally deterministic:

- old `active_station` becomes part of the initial routing order;
- old `level` becomes ordering input only, not a primary authoring knob;
- old station/group members flatten into provider entries and `routing.order`;
- provider tags are preserved;
- business tags such as `billing=monthly` are never guessed;
- endpoint-scoped station groups may warn because v3 routing is provider-level by default.

After migration, treat `provider` and `routing` as the public write surface.

## Design Boundaries

codex-helper intentionally avoids these patterns:

- one full Codex config per provider;
- inferring billing class from provider names;
- making speed or cost balancing primary before real measurements exist;
- keeping `level` as the main user-facing priority control;
- silently writing legacy station schema from GUI or TUI.

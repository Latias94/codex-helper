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

## Balance Adapters

Balance and quota live in a separate local file:

`~/.codex-helper/usage_providers.json`

This file describes how codex-helper should fetch provider balance state. Keep it separate from the relay config so provider onboarding stays thin.

Most relay users do not need to create this file just to see a balance. If no configured adapter matches an upstream, codex-helper automatically tries the common relay probes in this order:

1. `sub2api_usage`: `GET {{base_url}}/v1/usage` with the upstream model API key.
2. `new_api_user_self`: `GET {{base_url}}/api/user/self` with the upstream model API key.
3. `openai_balance_http_json`: `GET {{base_url}}/user/balance` with the upstream model API key.

The auto probe records only the first usable `ok` or `exhausted` snapshot. Failed guesses are logged but are not surfaced as three separate UI errors. If any configured provider in `usage_providers.json` matches an upstream host, that explicit configuration wins and the auto probe is skipped for that upstream.

Use explicit adapters when a relay needs a custom endpoint, custom headers, dashboard credentials, or `trust_exhaustion_for_routing = false`:

- `sub2api_usage`: Sub2API API-key telemetry, usually `GET {{base_url}}/v1/usage`.
- `sub2api_auth_me`: Sub2API dashboard JWT account balance, usually `GET {{base_url}}/api/v1/auth/me`.
- `new_api_user_self`: New API dashboard quota, usually `GET {{base_url}}/api/user/self`.
- `openai_balance_http_json`: generic OpenAI-compatible relay balance, usually `GET {{base_url}}/user/balance`.

`{{base_url}}` is the upstream URL normalized without a trailing `/v1`. Use `{{upstream_base_url}}` if a relay really exposes its balance endpoint under the same `/v1` prefix as chat/completions.

Sub2API API-key telemetry, modeled after all-api-hub's `/v1/usage` probe. If the upstream already has a configured `auth_token_env`, `token_env` can be omitted and codex-helper will reuse the upstream key:

```json
{
  "providers": [
    {
      "id": "input-monthly",
      "kind": "sub2api_usage",
      "domains": ["ai.input.im"],
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

Sub2API dashboard JWT balance:

```json
{
  "providers": [
    {
      "id": "input-dashboard",
      "kind": "sub2api_auth_me",
      "domains": ["ai.input.im"],
      "token_env": "INPUT_DASHBOARD_JWT",
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

For Sub2API, `sub2api_usage` uses the model API key and can expose remaining quota plus aggregate usage when the relay implements `/v1/usage`. `sub2api_auth_me` uses the dashboard JWT, not the model API key, and is mainly useful when you already maintain that credential separately. Because it needs dashboard auth, `sub2api_auth_me` is explicit-only and is not part of the zero-config auto probe. Keep `trust_exhaustion_for_routing = false` if a dashboard endpoint reports misleading zero balances for active subscriptions.

New API-style quota:

```json
{
  "providers": [
    {
      "id": "right-newapi",
      "kind": "new_api_user_self",
      "domains": ["www.right.codes"],
      "endpoint": "{{base_url}}/api/user/self",
      "token_env": "RIGHTCODE_NEWAPI_ACCESS_TOKEN",
      "headers": {
        "New-Api-User": "{{env:RIGHTCODE_NEWAPI_USER_ID}}"
      },
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

For New API, the dashboard access token and `New-Api-User` value are often not the same as the model API key. Keep them in environment variables.

The generated default file also includes fixed-domain official balance adapters modeled after CC Switch's built-ins: DeepSeek, StepFun, SiliconFlow, OpenRouter, and Novita AI. These are safe as defaults because their domains and account endpoints are unambiguous. Ordinary relays are auto-probed first; add an explicit `sub2api_usage`, `sub2api_auth_me`, or `new_api_user_self` entry only when the default probe order is not correct for that relay.

Supported adapter kinds include:

- `openai_balance_http_json`
- `relay_balance_http_json`
- `sub2api_usage`
- `sub2api_usage_http_json`
- `sub2api_auth_me`
- `new_api_user_self`
- `yescode_profile`
- `budget_http_json`

Useful fields:

| Field | Meaning |
| --- | --- |
| `domains` | Which relay hosts this adapter should apply to. |
| `endpoint` | Balance endpoint URL, with optional `{{base_url}}` templating. |
| `token_env` | Environment variable used for auth. |
| `poll_interval_secs` | Refresh throttle / cache window. `0` disables automatic refresh. |
| `refresh_on_request` | Whether routed requests may trigger a balance refresh. |
| `trust_exhaustion_for_routing` | Whether an exhausted snapshot may demote routing. |
| `headers` / `variables` | Adapter-specific request templating. |
| `extract` | JSON path extraction rules for balance fields. |

Useful `extract` fields:

| Field | Meaning |
| --- | --- |
| `remaining_balance_paths` | Candidate JSON paths for remaining balance. Array indexes are supported, for example `balance_infos.0.total_balance`. |
| `monthly_budget_paths` / `monthly_spent_paths` | Candidate JSON paths for plan limit and spent amount. |
| `remaining_divisor` / `monthly_budget_divisor` / `monthly_spent_divisor` | Convert minor units into display units. |
| `derive_budget_from_remaining_and_spent` | Compute budget as remaining + spent. |
| `derive_remaining_from_budget_and_spent` | Compute remaining as budget - spent. |
| `exhausted_paths` | Candidate JSON paths for an explicit exhausted boolean. |

Refresh policy:

- request-driven refresh is the default;
- unmatched relay upstreams are auto-probed after requests and by manual refresh;
- UI surfaces read cached snapshots only;
- manual refresh is exposed through `POST /__codex_helper/api/v1/providers/balances/refresh`;
- if a provider returns misleading zeroes, keep the raw exhausted state visible but set `trust_exhaustion_for_routing = false`.

## Pricing Catalog

Price data is also separate from the relay config.

- local overrides live in `~/.codex-helper/pricing_overrides.toml`
- the merged catalog is exposed by the proxy and rendered by GUI/TUI
- `codex-helper pricing sync` and `codex-helper pricing sync-basellm` refresh the local catalog from source-backed inputs

Use price overrides for local corrections or relay-specific multipliers; do not duplicate pricing tables inside the relay config itself.

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

Current UI behavior intentionally separates runtime steering from persisted routing edits.

- The GUI proxy settings screen accepts v3 routing-first TOML. Legacy station-first files are auto-migrated on load; use `config migrate --dry-run` if you want to inspect the rewrite first.
- GUI form view summarizes providers, profiles, and routing; detailed edits currently go through raw TOML or CLI commands.
- TUI station switching is runtime-only for legacy station configs. Under v3, provider choice belongs to persisted routing; `p` / `P` / `Enter` route users to the routing editor instead of pinning the internal `routing` station.
- TUI `Stations` page `r` opens the persisted v3 routing editor. It can pin a provider, switch back to ordered failover, reorder `routing.order`, enable or disable a provider, enable monthly-first tag preference, toggle `on_exhausted`, and set/clear the selected provider's `billing` tag.
- Persistent provider and routing edits should use the TUI routing editor, `provider`, `routing`, or the v3 raw config.

TUI routing editor shortcuts:

- `Enter`: `manual-sticky` pin selected provider.
- `a`: `ordered-failover` using the visible order.
- `[` / `]` or `u` / `d`: move selected provider in `routing.order`.
- `f`: `tag-preferred` with `prefer_tags = [{ billing = "monthly" }]`.
- `e`: enable or disable the selected provider. Disabling a pinned `manual-sticky` target also downgrades routing to `ordered-failover`.
- `s`: toggle `on_exhausted` between `continue` and `stop`.
- `1` / `2` / `0`: set `billing=monthly`, set `billing=paygo`, or clear `billing`.

This keeps the UI mental model simple: temporary station steering remains runtime-only; durable provider intent lives in `[service.providers]` and `[service.routing]`.

## Migration Notes

`v0.13.0` treats `version = 3` as the public persisted schema. Existing files are migrated automatically the first time they are loaded by the CLI, proxy, GUI, or TUI:

- `version = 3` TOML loads directly.
- `version = 2` TOML, unversioned legacy TOML, and legacy `config.json` are loaded, compiled, and written back as `config.toml` with `version = 3`.
- The previous source file is copied to `config.toml.bak` for TOML or `config.json.bak` for JSON before the new v3 file is written.
- If automatic migration cannot be written, startup continues with the loaded runtime config and logs a warning.

You can still preview the exact migration output before starting the proxy:

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

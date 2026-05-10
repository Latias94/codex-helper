# Configuration Guide

This guide documents the public `version = 3` config format.

The short version: define providers once, then choose one routing recipe. Most users only need `[codex.providers.*]`, `[codex.routing]`, and `[retry]`.

## Mental Model

- `providers` are your upstream catalog: base URL, auth, optional tags, optional endpoints.
- `routing` is the active provider-selection recipe: ordered fallback, manual pin, or tag preference.
- `profiles` are request defaults such as model and reasoning effort. They should not pick providers.
- `retry` controls how hard the proxy retries before returning an error.

Some runtime internals still use the legacy `station` wording, but hand-written config should think in `provider` plus `routing`.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Balance adapters: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request log: `~/.codex-helper/logs/requests.jsonl`

Codex-owned files remain owned by Codex:

- `~/.codex/auth.json`
- `~/.codex/config.toml`

`switch on/off` and one-command startup only patch the local Codex proxy section. They do not overwrite unrelated Codex config changes.

## Recommended Start

Use CLI commands when possible:

```bash
codex-helper config init

codex-helper provider add input \
  --base-url https://ai.input.im/v1 \
  --auth-token-env INPUT_API_KEY \
  --tag billing=monthly

codex-helper provider add openai \
  --base-url https://api.openai.com/v1 \
  --auth-token-env OPENAI_API_KEY \
  --tag billing=paygo

codex-helper routing order input openai
codex-helper config set-retry-profile balanced
```

This creates the same thin TOML shape you would write by hand:

```toml
version = 3

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "ordered-failover"
order = ["input", "openai"]
on_exhausted = "continue"

[retry]
profile = "balanced"
```

## Recipes

Pick one recipe first. You can refine fields later.

### One Provider

Use this when you only want codex-helper as a local proxy and dashboard.

```toml
version = 3

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
policy = "manual-sticky"
target = "main"
order = ["main"]

[retry]
profile = "balanced"
```

### Ordered Fallback

Use this as the default for multiple relays: first working provider wins, then fallback in order.

```toml
version = 3

[codex.providers.monthly]
base_url = "https://monthly.example/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"
tags = { billing = "paygo" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "official" }

[codex.routing]
policy = "ordered-failover"
order = ["monthly", "backup", "openai"]
on_exhausted = "continue"
```

This is the most direct replacement for old priority or level-based setups.

### Monthly First

Use this when the business intent matters: prefer every provider tagged `billing=monthly`, then continue to the rest.

```toml
version = 3

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly", region = "jp" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

Only known fully exhausted monthly candidates are demoted. A balance lookup failure is shown as `unknown` and does not mean exhausted.

### Monthly Only

Use this when you would rather fail than spill into a paid fallback.

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "stop"
```

`paygo` can stay in the file for later use, but the stop rule prevents automatic spillover after the preferred set is exhausted.

### Manual Pin

Use this for debugging, strict vendor selection, or temporary steering.

```toml
[codex.routing]
policy = "manual-sticky"
target = "input"
order = ["input", "openai"]
```

A pinned target is explicit. If it fails, codex-helper does not silently pretend a different provider was selected.

### Multiple Endpoints For One Provider

Use explicit endpoints only when one account really has several upstream targets.

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

Do not use endpoints just to model unrelated providers. Put unrelated accounts under separate provider names.

## Routing Policies

| Policy | Best For | UI Mental Model |
| --- | --- | --- |
| `ordered-failover` | Simple fallback chains | Reorder a provider list |
| `tag-preferred` | Monthly-first, region-first, vendor-class-first setups | Choose a preferred tag, then order fallback |
| `manual-sticky` | Debugging or strict manual selection | Pick one active provider |

`on_exhausted` controls what happens after the preferred set is known to be depleted:

| Value | Behavior |
| --- | --- |
| `continue` | Continue into the remaining fallback order. Best for availability. |
| `stop` | Stop after preferred providers are exhausted. Best for budget isolation. |

codex-helper does not infer billing class from names. If a provider is monthly, tag it explicitly:

```toml
tags = { billing = "monthly" }
```

## Provider Fields

Common provider fields:

| Field | Meaning | Recommendation |
| --- | --- | --- |
| `alias` | Human-friendly display name | Optional |
| `base_url` | OpenAI-compatible endpoint | Use for single-endpoint providers |
| `auth_token_env` | Environment variable for bearer auth | Preferred for secrets |
| `auth_token` | Inline bearer token | Supported, but avoid committing it |
| `api_key_env` | Environment variable for `X-API-Key` auth | Use only when required |
| `api_key` | Inline `X-API-Key` value | Supported, but avoid committing it |
| `tags` | Free-form metadata | Use stable tags like `billing`, `vendor`, `region` |
| `enabled` | Whether the provider is routeable | Prefer `provider disable/enable` for temporary changes |
| `supported_models` | Optional model allowlist | Advanced |
| `model_mapping` | Optional model alias map | Advanced |

Example with an inline secret:

```toml
[codex.providers.local_test]
base_url = "https://test.example/v1"
auth_token = "sk-..."
```

Inline secrets are useful for local scratch configs. For real use, prefer environment variables.

## Profiles

Profiles are optional request defaults. They should not decide provider routing.

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

Legacy profile station bindings are migration-only. New v3 configs should use `[codex.routing]`.

## Balance Adapters

Most relay users do not need to write `usage_providers.json` just to see balances. If no explicit adapter matches an upstream, codex-helper tries common relay probes:

1. `sub2api_usage`: `GET {{base_url}}/v1/usage` with the model API key.
2. `new_api_user_self`: `GET {{base_url}}/api/user/self` with the model API key.
3. `openai_balance_http_json`: `GET {{base_url}}/user/balance` with the model API key.

Explicit adapters are still useful when a relay needs dashboard credentials, custom headers, a custom endpoint, or safer exhaustion handling.

In balance adapter templates, `{{base_url}}` is normalized without a trailing `/v1`. Use `{{upstream_base_url}}` only when a balance endpoint really lives under the same `/v1` prefix as model requests.

Sub2API API-key telemetry:

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

New API dashboard-style quota:

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

Important balance behavior:

- Lookup failure is displayed as `unknown`, not `err`, and is not treated as exhausted.
- Known exhausted snapshots can demote automatic routing only when `trust_exhaustion_for_routing = true`.
- If a provider reports misleading zero balances for active subscriptions, set `trust_exhaustion_for_routing = false`.
- UI surfaces cached balance snapshots; manual refresh uses `POST /__codex_helper/api/v1/providers/balances/refresh`.

Common adapter kinds:

- `sub2api_usage`
- `sub2api_auth_me`
- `new_api_user_self`
- `openai_balance_http_json`
- `relay_balance_http_json`
- `yescode_profile`
- `budget_http_json`

Useful adapter fields:

| Field | Meaning |
| --- | --- |
| `domains` | Relay hosts this adapter applies to |
| `endpoint` | Balance endpoint URL, with optional `{{base_url}}` templating |
| `token_env` | Environment variable used for adapter auth |
| `headers` / `variables` | Request templating |
| `poll_interval_secs` | Refresh throttle / cache window |
| `refresh_on_request` | Whether routed requests may trigger balance refresh |
| `trust_exhaustion_for_routing` | Whether exhausted snapshots may demote routing |
| `extract` | JSON path extraction rules for custom balance fields |

## Pricing

Pricing is separate from relay config:

- Local overrides: `~/.codex-helper/pricing_overrides.toml`
- Built-in and synced catalog: rendered by TUI/GUI and used for estimated cost
- Sync commands:

```bash
codex-helper pricing sync <URL> --dry-run
codex-helper pricing sync-basellm --model gpt-5 --dry-run
```

Use pricing overrides for local corrections or relay-specific multipliers. Do not duplicate pricing tables inside provider config.

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

Use `--claude` on provider/routing commands when editing the Claude service instead of Codex.

`routing show` reads persisted config. `routing list` and `routing explain` read the compiled runtime candidate view.

## UI Editing

TUI and GUI should keep the same mental model as the config file:

- Provider list: names, aliases, enabled state, tags, balance, and fallback order.
- Routing editor: policy, target, order, preferred tags, and exhaustion behavior.
- Runtime steering: useful for temporary choices, but durable provider intent belongs in `[service.providers]` and `[service.routing]`.

TUI routing editor shortcuts:

- `Enter`: pin selected provider with `manual-sticky`.
- `a`: switch to `ordered-failover` using the visible order.
- `[` / `]` or `u` / `d`: move selected provider in `routing.order`.
- `f`: enable monthly-first tag preference with `prefer_tags = [{ billing = "monthly" }]`.
- `e`: enable or disable the selected provider.
- `s`: toggle `on_exhausted` between `continue` and `stop`.
- `1` / `2` / `0`: set `billing=monthly`, set `billing=paygo`, or clear `billing`.

Advanced multi-endpoint providers, model mappings, and custom balance extraction rules are still best edited with CLI or raw TOML/JSON.

## Migration

`v0.13.0` treats `version = 3` as the public persisted schema.

On load, legacy `version = 2`, unversioned TOML, and legacy `config.json` are migrated to `config.toml` with `version = 3`. The previous file is copied to `config.toml.bak` or `config.json.bak` before writing the new file.

Preview migration before starting the proxy:

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Migration rules:

- old `active_station` becomes part of the initial routing order;
- old `level` becomes ordering input only;
- old station/group members flatten into provider entries and `routing.order`;
- existing provider tags are preserved;
- business tags such as `billing=monthly` are never guessed;
- endpoint-scoped station groups may warn because v3 routing is provider-level by default.

After migration, treat provider and routing as the public write surface.

## Design Boundaries

codex-helper intentionally avoids:

- one full Codex config per provider;
- inferring billing class from provider names;
- pretending speed-first or cost-first routing is reliable before real measurements exist;
- keeping `level` as the main user-facing priority control;
- treating balance lookup failure as provider exhaustion;
- silently writing legacy station schema from GUI or TUI.

# Configuration Guide

中文参考: [CONFIGURATION.zh.md](CONFIGURATION.zh.md)

This guide documents the public `version = 5` route graph config format.

The short version: define providers once, then point `routing.entry` at a named route node under `routing.routes`. Most users only need `[codex.providers.*]`, `[codex.routing]`, `[codex.routing.routes.*]`, and `[retry]`.

## Mental Model

- `providers` are your upstream catalog: base URL, auth, optional tags, optional endpoints.
- `routing.entry` is the root route node for a service.
- `routing.routes.*` are named route nodes. A route node can reference providers or other route nodes.
- `profiles` are request defaults such as model and reasoning effort. They should not pick providers.
- `retry` controls how hard the proxy retries before returning an error.

Legacy `station` data is migration input. Hand-written config should think in `provider`, `endpoint`, and `route graph`.

## Local Proxy Vs Outbound Proxy

There are two different proxy layers:

- Local proxy: Codex connects to codex-helper, usually at `127.0.0.1:3211`. This still happens when you do not configure an outbound network proxy.
- Outbound proxy: codex-helper connects to provider endpoints, relay dashboards, or balance APIs through a network proxy.

Current outbound proxy support comes from the underlying HTTP client's system/environment proxy behavior. `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` may affect provider and balance requests. There is not yet a first-class `config.toml` outbound proxy section. See [Outbound Proxy](#outbound-proxy) for the current behavior and the intended future model.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Balance adapters: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request log: `~/.codex-helper/logs/requests.jsonl`
- Routing/control trace: `~/.codex-helper/logs/control_trace.jsonl`

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
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["input", "openai"]

[retry]
profile = "balanced"
```

## Route Graph Shape

Every service can have its own route graph:

```toml
[codex.routing]
entry = "monthly_first"
affinity_policy = "preferred-group"
# Optional compatibility bounds for fallback-sticky affinity.
# fallback_ttl_ms = 120000
# reprobe_preferred_after_ms = 30000

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]
```

Rules:

- A route node name must not be the same as a provider name.
- `children` can reference providers or route nodes.
- Cycles are rejected.
- Duplicate provider leaves are rejected because they make fallback behavior ambiguous.
- Runtime health, cooldown, balance exhaustion, and reprobe state are not stored in static config.
- Provider names do not imply business class. Use tags such as `billing = "monthly"` or `billing = "paygo"` when route policy should care about billing.

Common strategies:

- `ordered-failover`: try children from left to right. Children can be providers or nested route nodes.
- `tag-preferred`: split children into preferred groups by `prefer_tags`, then fallback to the rest. `on_exhausted = "continue"` allows paid fallback after trusted exhaustion; `on_exhausted = "stop"` prevents automatic spillover.
- `manual-sticky`: use one explicit `target`. The target can be a route node, provider, or provider endpoint.

Most users should prefer `ordered-failover` for fixed priority and `tag-preferred` for "monthly first" business intent.

## Session Affinity

Route graph session affinity is runtime state. The TOML config chooses the affinity policy and can optionally bound fallback stickiness:

- `preferred-group` is the default. Session affinity is only applied inside the currently best available preference group, so a session that temporarily falls back to paygo returns to monthly as soon as a monthly provider is viable again.
- `off` ignores automatic route affinity.
- `fallback-sticky` keeps the old fallback-sticky behavior as an explicit compatibility mode. Set `fallback_ttl_ms` to cap how long a lower-priority fallback affinity can be reused, or `reprobe_preferred_after_ms` to force a preferred-group reprobe after a fallback target change.
- `hard` treats an existing affinity target as strict for that route graph; if the target is unavailable, no alternate candidate is selected.

For each request with a session id, codex-helper keys affinity by `session_id + service + route_graph_key`. While the route graph is unchanged, the same session can keep using the previously selected provider/endpoint according to the policy. This improves upstream prompt-cache locality for relay providers that cache by account or upstream target without letting automatic stickiness override user preference by default.

Affinity is not a hard pin:

- request retry, provider health, capability mismatch, cooldown, and trusted balance exhaustion still apply;
- if the sticky provider fails, the request continues through the current route graph and then sticks to the next successful provider;
- if provider tags, route node strategy, children, entry, or provider endpoint identity change, the route graph key changes and old affinity no longer matches;
- legacy station overrides are disabled for route graph configs; use route/provider/endpoint controls instead.

This means monthly pools such as `monthly_pool -> paygo` normally keep a conversation on one monthly provider until that provider stops being viable, instead of round-robining every request and reducing upstream cache hit rate.

## Recipes

Pick one recipe first. You can refine fields later. For Claude, replace `codex` with `claude`.

| User Goal | Start With | Why |
| --- | --- | --- |
| I only have one upstream and want the dashboard/logs | [One Provider](#one-provider) | Smallest config; no accidental fallback |
| I have several relays and want the first working one | [Ordered Fallback](#ordered-fallback) | Simple left-to-right fallback |
| I have several monthly relays and one pay-as-you-go backup | [Monthly Pool With Paygo Fallback](#monthly-pool-with-paygo-fallback) | Preserves the monthly pool as one preferred group |
| I have several monthly relays and several paid relay backups | [Monthly Pool With Relay Fallback Pool](#monthly-pool-with-relay-fallback-pool) | Keeps monthly and paid fallback pools explicit |
| I want all monthly-tagged providers before anything paid | [Monthly First By Tag](#monthly-first-by-tag) | Uses metadata instead of hard-coding a named pool |
| I would rather fail than spend pay-as-you-go money | [Monthly Only](#monthly-only) | Stops after trusted monthly exhaustion |
| I need to force one provider temporarily | [Manual Pin](#manual-pin) | Explicit and easy to undo |
| One provider account has multiple upstream endpoints | [Multiple Endpoints For One Provider](#multiple-endpoints-for-one-provider) | Keeps one provider identity with endpoint-level routing |

Routing decisions use runtime provider endpoints. `compatibility` station/upstream fields in diagnostics are migration context, not the new identity.

### One Provider

Use this when you only want codex-helper as a local proxy and dashboard.

```toml
version = 5

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "manual-sticky"
target = "main"

[retry]
profile = "balanced"
```

### Ordered Fallback

Use this as the default for multiple relays: first working provider wins, then fallback in order.

```toml
version = 5

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
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["monthly", "backup", "openai"]

[retry]
profile = "balanced"
```

This is the most direct replacement for old priority or level-based setups.

### Monthly Pool With Paygo Fallback

Use this when several monthly providers form one preferred group and a paygo provider is only the fallback of last resort.

```toml
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.input1]
base_url = "https://ai.input1.im/v1"
auth_token_env = "INPUT1_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.input2]
base_url = "https://ai.input2.im/v1"
auth_token_env = "INPUT2_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]

[retry]
profile = "balanced"
```

This keeps the monthly pool as a first-class route node. Temporary 502/429-style failures recover through cooldown and later reprobe. `unknown` balance is not treated as exhausted. Confirmed exhaustion is the only balance signal that can demote a monthly candidate.

### Monthly Pool With Relay Fallback Pool

Use this when you want to spend monthly providers first, then try several relay fallbacks in a fixed order.

```toml
version = 5

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_c]
base_url = "https://monthly-c.example/v1"
auth_token_env = "MONTHLY_C_API_KEY"
tags = { billing = "monthly" }

[codex.providers.right]
base_url = "https://right.example/v1"
auth_token_env = "RIGHT_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.providers.cch]
base_url = "https://cch.example/v1"
auth_token_env = "CCH_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly_a", "monthly_b", "monthly_c"]

[codex.routing.routes.fallback_pool]
strategy = "ordered-failover"
children = ["right", "cch", "codex_for"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "fallback_pool"]

[retry]
profile = "balanced"
```

This is the clearest shape for "monthly first, several relays as backup". Session affinity still applies: a conversation keeps using the last successful provider while the route graph stays the same, then moves forward only after that provider fails, cools down, no longer supports the request, or is confirmed exhausted.

### Monthly First By Tag

Use this when the business intent is metadata: prefer every provider tagged `billing=monthly`, then continue to the rest.

```toml
version = 5

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
entry = "monthly_first"

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"

[retry]
profile = "balanced"
```

Only known fully exhausted monthly candidates are demoted. A balance lookup failure is shown as `unknown` and does not mean exhausted.

### Monthly Only

Use this when you would rather fail than spill into a paid fallback.

```toml
version = 5

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly_a", "monthly_b"]

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_pool", "paygo"]
on_exhausted = "stop"

[retry]
profile = "balanced"
```

`paygo` can stay in the file for later use, but the stop rule prevents automatic spillover after the preferred set is exhausted.

### Manual Pin

Use this for debugging, strict vendor selection, or temporary steering.

```toml
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"

[codex.routing]
entry = "debug_pin"

[codex.routing.routes.debug_pin]
strategy = "manual-sticky"
target = "input"
children = ["input", "openai"]

[retry]
profile = "balanced"
```

A pinned target is explicit. It can name a route node, a provider, or a
provider endpoint such as `relay.hk`. If it is disabled, codex-helper rejects
the route instead of silently selecting a different provider.

### Multiple Endpoints For One Provider

Use explicit endpoints only when one account really has several upstream targets.

```toml
version = 5

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

[codex.routing]
entry = "relay_route"

[codex.routing.routes.relay_route]
strategy = "ordered-failover"
children = ["relay.hk", "relay.us"]

[retry]
profile = "balanced"
```

Do not use endpoints just to model unrelated providers. Put unrelated accounts under separate provider names.

## Route Strategies

| Strategy | Best For | UI Mental Model |
| --- | --- | --- |
| `ordered-failover` | Simple fallback chains and named pools | Reorder child routes/providers |
| `tag-preferred` | Monthly-first, region-first, vendor-class-first setups | Choose preferred tags, then fallback |
| `manual-sticky` | Debugging or strict manual selection | Pick one target |

`on_exhausted` is currently used by `tag-preferred`:

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

Legacy profile station bindings are migration-only. New v5 configs should use `[codex.routing]`.

## Balance Adapters

Most relay users do not need to write `usage_providers.json` just to see balances. If no explicit adapter matches an upstream, codex-helper tries common relay probes:

1. `sub2api_usage`: `GET {{base_url}}/v1/usage` with the model API key.
2. `new_api_token_usage`: `GET {{base_url}}/api/usage/token/` with the model API key.
3. `new_api_user_self`: `GET {{base_url}}/api/user/self` with dashboard-style auth.
4. `openai_balance_http_json`: `GET {{base_url}}/user/balance` with the model API key.

RightCode hosts (`www.right.codes` / `right.codes`) are special-cased before the generic relay probes. The built-in `rightcode_account_summary` adapter calls `GET https://www.right.codes/account/summary`, uses bearer auth, reads wallet `balance`, and matches subscription daily quota by the upstream path prefix such as `/codex`.

Explicit adapters are still useful when a relay needs dashboard credentials, custom headers, a custom endpoint, or safer exhaustion handling.

For `api.openai.com`, codex-helper skips relay-style `/user/balance` probing. If `OPENAI_ADMIN_KEY` is set, it can auto-read `openai_organization_costs`; otherwise the official OpenAI provider remains unknown instead of being treated as exhausted.

OpenAI's public platform surface is not a wallet-balance API. It exposes organization-level costs/usage views, which are suitable for showing current spend but not for routing off a wallet balance or subscription remainder. To connect the official OpenAI billing view, use:

```json
{
  "providers": [
    {
      "id": "openai-official-costs",
      "kind": "openai_organization_costs",
      "domains": ["api.openai.com"],
      "token_env": "OPENAI_ADMIN_KEY",
      "require_token_env": true,
      "endpoint": "https://api.openai.com/v1/organization/costs?start_time={{unix_days_ago:30}}&limit=30",
      "poll_interval_secs": 60,
      "refresh_on_request": false,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

`OPENAI_ADMIN_KEY` must be an organization-level admin key; a normal model API key is not a stable substitute.

In balance adapter templates, `{{base_url}}` is normalized without a trailing `/v1`. Use `{{upstream_base_url}}` only when a balance endpoint really lives under the same `/v1` prefix as model requests. Time helpers such as `{{unix_now}}`, `{{unix_now_ms}}`, and `{{unix_days_ago:30}}` are available for official usage/cost APIs that require query windows.

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

RightCode account summary:

```json
{
  "providers": [
    {
      "id": "rightcode",
      "kind": "rightcode_account_summary",
      "domains": ["www.right.codes", "right.codes"],
      "endpoint": "https://www.right.codes/account/summary",
      "token_env": "RIGHTCODE_API_KEY",
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

You can omit this block for the normal case: the default adapter is built in, matches RightCode by upstream URL, and uses that upstream's configured model API key. Add it only when you want a separate balance key such as `RIGHTCODE_API_KEY`, a custom endpoint, or a different routing trust policy. By default, RightCode daily package quota is display-only for routing because the account `balance` may still be available and daily subscription windows can reset lazily.

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

- Lookup failure is displayed as `unknown`, not exhausted, and does not change route graph config.
- Known exhausted snapshots can demote automatic routing only when `trust_exhaustion_for_routing = true`.
- Sub2API lazy subscription-window zeros are displayed as lazy reset state before a real request refreshes the period; they should not be confused with a durable package design choice.
- Sub2API subscription-mode `remaining` is a period-limit capacity signal, not a wallet balance. A zero `remaining` means at least one configured subscription window is currently exhausted and may demote routing once trusted.
- New API quota values are quota units converted with `QuotaPerUnit = 500000`; token usage snapshots with `unlimited_quota = true` are never treated as exhausted.
- RightCode `balance` is shown as wallet balance. Matched `subscriptions[*].total_quota` and `remaining_quota` are shown as daily quota; `reset_today = false` means codex-helper includes today's fresh daily quota before displaying remaining quota.
- If a provider reports misleading zero balances for active subscriptions, set `trust_exhaustion_for_routing = false`.
- UI surfaces cached balance snapshots; manual refresh uses `POST /__codex_helper/api/v1/providers/balances/refresh`.
- Balance HTTP calls are bounded and reuse the same outbound client as proxy runtime calls. A failed lookup should surface the probed origin and adapter kind in logs, for example whether `sub2api_usage` or `openai_balance_http_json` returned non-JSON.

## Usage / Balance Page

TUI page 5 is now labeled `Usage`, and the GUI stats page is titled `Usage / Balance`. Both consume the same core `UsageBalanceView`, so provider, endpoint, balance state, and route-impact semantics should match.

How to read it:

- The summary band shows request count, tokens, estimated cost, balance state counts, and the latest refresh state for the selected window.
- Provider rows show request volume, success rate, tokens, cost, primary balance/quota summary, balance state, and routing impact.
- Endpoint rows show recent provider endpoint samples, request count, error count, tokens, attached balance snapshot, and route skip reason.
- `unknown` means there is no trusted balance data or the lookup failed. Do not treat it as healthy balance.
- `stale` means the snapshot expired; it is distinct from `exhausted`, `error`, and `unlimited`.
- `unlimited` is a known unlimited quota state, not unknown.
- Press `g` on the TUI `Usage` page to refresh balances; use the `Refresh balances` button on the GUI stats page.
- A single provider balance refresh failure only updates that provider's error/unknown state. It does not interrupt other provider refreshes, TUI redraw, or snapshot refresh.
- The `Routing` page keeps compact balance context only. Use `Usage / Balance` to answer which provider is used most, which one is running out, or which endpoint is failing.

## Outbound Proxy

codex-helper is itself a local proxy, but it may still need an outbound proxy to reach some relays or dashboard balance APIs.

Current behavior:

- The underlying HTTP client uses reqwest's default system/environment proxy support. Standard `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` environment variables may affect outbound requests.
- There is not yet a first-class `config.toml` outbound proxy section.

Recommended model for a future config version:

- Add a global outbound proxy profile for all provider and balance traffic.
- Allow provider endpoint overrides when a specific relay needs a different egress path.
- Prefer provider/endpoint-scoped proxy selection over route-scoped proxy selection. Route policy should decide which provider endpoint to use; the endpoint should own how it is reached.
- Allow balance adapters to override proxy behavior only when their dashboard/balance API lives on a different network path than the model endpoint.

Common adapter kinds:

- `sub2api_usage`
- `sub2api_auth_me`
- `new_api_token_usage`
- `new_api_user_self`
- `rightcode_account_summary`
- `openai_organization_costs`
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
| `require_token_env` | Require `token_env` instead of falling back to the model API key |
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

Initialize or inspect migration:

Normal startup, including the default TUI path, performs config migration automatically. Use the migration commands only when you want to preview or diagnose the migration explicitly.

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

Manage the entry route from CLI:

```bash
codex-helper routing order input openai
codex-helper routing pin input
codex-helper routing prefer-tag --tag billing=monthly --order input,openai --on-exhausted continue
codex-helper routing set --policy ordered-failover --order input,openai
codex-helper routing clear-target
codex-helper routing show
codex-helper routing explain
```

The CLI preserves existing route graph structure when it only edits the entry node. Advanced nested graph authoring is still best done in TOML until dedicated route-node commands are added.

Use `--claude` on provider/routing commands when editing the Claude service instead of Codex.

`routing show` reads persisted config. `routing list` and `routing explain` read the compiled runtime candidate view.
Use `routing explain --model <MODEL> --json` to inspect the same selected route, candidate order, route paths, and structured skip reasons exposed by the runtime admin explain API.
In that response, `provider_endpoint_key`, `provider_id`, `endpoint_id`, `route_path`, and `preference_group` are the primary v5 routing identity. Legacy station/upstream identity is reported under each candidate's `compatibility` object for migration diagnostics.

## Inspect Routing And Logs

Use these commands before editing TOML by hand:

```bash
codex-helper routing show
codex-helper routing explain --json
codex-helper routing explain --model <MODEL> --json
```

`routing show` answers "what is saved in config". `routing explain` answers "what the runtime would try now", including candidate order, route paths, and skip reasons such as disabled provider, unsupported model, cooldown, or trusted balance exhaustion.

Every completed request is written to:

```text
~/.codex-helper/logs/requests.jsonl
```

When a request retries or switches provider, the request log stores `retry.route_attempts[]`. The most useful fields are `provider_id`, `endpoint_id`, `route_path`, `decision`, `status_code`, and `error_class`.

The control trace is enabled by default and is written to:

```text
~/.codex-helper/logs/control_trace.jsonl
```

It records routing selection events such as the compiled route plan, provider endpoint, preference group, skipped higher-priority groups, pinned-route decisions, retry options, and failover reasons. When a lower-priority preference group is selected, the `route_graph_selection_explain` event lists each higher-priority provider endpoint that was skipped and the structured reasons such as `unsupported_model`, `cooldown`, `usage_exhausted`, `runtime_disabled`, or `attempt_avoided`. Set `CODEX_HELPER_CONTROL_TRACE=0` to turn it off, or `CODEX_HELPER_CONTROL_TRACE_PATH` to write it somewhere else. The older `retry_trace.jsonl` file is only written when `CODEX_HELPER_RETRY_TRACE=1`.

## Troubleshoot Monthly-First Routing

If a route that should prefer monthly providers falls back to paygo, inspect the runtime state before changing the config:

```bash
codex-helper routing explain --model <MODEL> --json
```

Check these fields first:

- `selected_route.provider_endpoint_key` and `selected_route.preference_group` show what the runtime would try now. Group `0` is the most preferred group.
- `candidates[].skip_reasons` explains why a preferred candidate was skipped, for example `unsupported_model`, `cooldown`, `usage_exhausted`, `runtime_disabled`, or `attempt_avoided`.
- `affinity.policy` / `affinity_policy` tells whether automatic affinity is `preferred-group`, `off`, `fallback-sticky`, or `hard`.
- `compatibility` is legacy station/upstream context only. For route graph decisions, prefer `provider_endpoint_key`, `provider_id`, `endpoint_id`, and `route_path`.

For a monthly-first setup, the normal default is `affinity_policy = "preferred-group"`. With that policy, a session may use a fallback provider during a temporary outage, but the next request returns to the best viable monthly group once a monthly provider is available again. If the route keeps using paygo, look for one of these causes:

- an explicit session/global route target override is set;
- the monthly provider is disabled or missing auth;
- the requested model is unsupported by the monthly provider;
- the monthly endpoint is cooling down after retryable failures;
- trusted balance data marks the endpoint `usage_exhausted`;
- the config explicitly uses `affinity_policy = "fallback-sticky"` or `hard`.

Trusted balance exhaustion is a provider-endpoint runtime signal. It can demote a monthly endpoint for the current request/refresh window, but it is not a permanent session preference. If a provider reports misleading zero balances for an active subscription, set `trust_exhaustion_for_routing = false` for that usage provider or fix the balance extractor.

Use the control trace when a lower-priority group is selected:

```text
~/.codex-helper/logs/control_trace.jsonl
```

Look for `route_graph_selection_explain`. It records the selected provider endpoint, selected preference group, skipped higher-priority groups, and per-candidate skip reasons. Use route/provider/endpoint controls for temporary steering; legacy station overrides are rejected for route graph configs.

## UI Editing

TUI and GUI should keep the same mental model as the config file:

- Provider list: names, aliases, enabled state, tags, balance, and expanded fallback order.
- Routing editor: entry strategy, target, children/order, preferred tags, exhaustion behavior, and route graph tree preview.
- GUI route node editor: create, rename, delete, and save nested route nodes for common graph edits.
- Requests and sessions: provider choice, route affinity, retry chain, token/cache token usage, cache hit rate, and estimated cost.
- Runtime steering: useful for temporary choices, but durable provider intent belongs in `[service.providers]` and `[service.routing]`.

TUI routing editor shortcuts:

- `Enter`: pin selected provider with `manual-sticky`.
- `a`: switch the entry route to `ordered-failover` using the visible order.
- `[` / `]` or `u` / `d`: move selected provider in the entry route's expanded order.
- `f`: enable monthly-first tag preference with `prefer_tags = [{ billing = "monthly" }]`.
- `e`: enable or disable the selected provider.
- `s`: toggle `on_exhausted` between `continue` and `stop`.
- `1` / `2` / `0`: set `billing=monthly`, set `billing=paygo`, or clear `billing`.

Advanced multi-endpoint providers, model mappings, custom balance extraction rules, and deeply nested graphs are still best edited with CLI or raw TOML/JSON.

## Migration

The current route graph schema writes `version = 5`. Existing `version = 4` route graph configs still load as migration input.

Normal users usually do not need to run migration commands by hand. Starting codex-helper, including the default TUI startup path, loads legacy `version = 4`, `version = 3`, `version = 2`, unversioned TOML, and legacy `config.json`, then migrates them to `config.toml` with `version = 5`. The previous file is copied to `config.toml.bak` or `config.json.bak` before writing the new file.

During migration, codex-helper warns when the resulting route graph would use the new `preferred-group` default instead of old fallback stickiness. If you want the old behavior back, set `affinity_policy = "fallback-sticky"` explicitly before or after migration.

Manual migration commands are mainly for previewing or diagnosing a migration without going through the normal TUI/proxy startup path:

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Migration rules:

- old `active_station` becomes part of the initial route entry;
- old `level` becomes ordering input only;
- old station/group members flatten into provider entries and an entry route's `children`;
- legacy v3 `policy/order/target/prefer_tags` becomes a v5 entry route node;
- legacy v3 `pool-fallback` becomes nested route nodes;
- existing provider tags are preserved;
- business tags such as `billing=monthly` are never guessed;
- endpoint-scoped station groups may warn because provider routing is provider-level by default.

After migration, treat provider and routing graph as the public write surface. Station-shaped inputs are compatibility readers and migration diagnostics, not the runtime routing identity.

## Design Boundaries

codex-helper intentionally avoids:

- one full Codex config per provider;
- inferring billing class from provider names;
- pretending speed-first or cost-first routing is reliable before real measurements exist;
- keeping `level` as the main user-facing priority control;
- treating balance lookup failure as provider exhaustion;
- silently writing legacy station schema from GUI or TUI;
- using a special `pool-fallback` syntax when nested route nodes express the same intent more cleanly.

# Config V2 Migration Guide

This guide explains how to move an existing `~/.codex-helper/config.toml` from the legacy `configs` layout to the station/provider-oriented `v2` layout.

## Why V2

The legacy shape is easy to bootstrap, but it mixes three different concerns into one object:

- station selection
- provider credentials
- endpoint inventory

For a personal relay manager, that quickly becomes hard to maintain when you want to:

- reuse one provider across multiple stations
- define fast/deep profiles cleanly
- switch stations without duplicating auth blocks
- prepare for GUI/WebUI management later

`v2` separates the model into:

- `providers`: credentials, shared tags, shared model capabilities, endpoint list
- `stations`: routing entries, level/enabled state, provider membership
- `profiles`: reusable session intent (`station`, `model`, `reasoning_effort`, `service_tier`)

## Safe Migration Commands

Preview the migrated file first:

```powershell
codex-helper config migrate --to v2 --dry-run
```

Preview a cleaner provider/endpoint layout:

```powershell
codex-helper config migrate --to v2 --compact --dry-run
```

Write the migrated result back to `~/.codex-helper/config.toml`:

```powershell
codex-helper config migrate --to v2 --compact --write --yes
```

Generate a fresh `v2` template:

```powershell
codex-helper config init --force
```

Notes:

- `config init` now writes a `version = 2` TOML template.
- If the current file is already `version = 2`, runtime saves now preserve `v2` instead of silently writing back the legacy `version = 1` shape.
- Existing `v2` files that still use `active_group` / `groups` are still accepted on load.
- Legacy boolean-like values such as `active = "true"` / `active = "false"` are normalized during load when they do not point to a real station name.

## Vocabulary Mapping

| Legacy | V2 | Meaning |
| --- | --- | --- |
| `active` | `active_station` | default station entry for the service |
| `codex.configs.<name>` | `codex.stations.<name>` | routing station |
| `[[...upstreams]]` | `providers.*.endpoints.*` + `stations.*.members` | upstream inventory + station membership |
| `profiles.*.station` | unchanged | profile pins a station |

## Before

Legacy `version = 1` example:

```toml
version = 1

[codex]
active = "right"
default_profile = "fast"

[codex.configs.right]
name = "right"
enabled = true
level = 1

[[codex.configs.right.upstreams]]
base_url = "https://www.right.codes/codex/v1"

[codex.configs.right.upstreams.auth]
auth_token_env = "RIGHTCODE_API_KEY"

[codex.configs.right.upstreams.tags]
provider_id = "right"

[codex.configs.vibe]
name = "vibe"
enabled = true
level = 2

[[codex.configs.vibe.upstreams]]
base_url = "https://api-vip.codex-for.me/v1"

[codex.configs.vibe.upstreams.auth]
auth_token_env = "VIBE_API_KEY"

[codex.configs.vibe.upstreams.tags]
provider_id = "vibe"

[codex.profiles.fast]
station = "right"
service_tier = "priority"
reasoning_effort = "low"

[codex.profiles.deep]
station = "vibe"
model = "gpt-5.4"
reasoning_effort = "high"
```

## After

Recommended `version = 2` shape:

```toml
version = 2

[codex]
active_station = "right"
default_profile = "fast"

[codex.providers.right]
[codex.providers.right.auth]
auth_token_env = "RIGHTCODE_API_KEY"
[codex.providers.right.tags]
provider_id = "right"
[codex.providers.right.endpoints.default]
base_url = "https://www.right.codes/codex/v1"

[codex.providers.vibe]
[codex.providers.vibe.auth]
auth_token_env = "VIBE_API_KEY"
[codex.providers.vibe.tags]
provider_id = "vibe"
[codex.providers.vibe.endpoints.default]
base_url = "https://api-vip.codex-for.me/v1"

[codex.stations.right]
enabled = true
level = 1

[[codex.stations.right.members]]
provider = "right"
endpoint_names = ["default"]
preferred = true

[codex.stations.vibe]
enabled = true
level = 2

[[codex.stations.vibe.members]]
provider = "vibe"
endpoint_names = ["default"]

[codex.profiles.fast]
station = "right"
service_tier = "priority"
reasoning_effort = "low"

[codex.profiles.deep]
station = "vibe"
model = "gpt-5.4"
reasoning_effort = "high"
```

## How To Think About Providers vs Stations

Use a `provider` when the auth identity is the same and only the endpoint list differs.

Examples:

- one OpenAI-compatible account with `hk` / `us` endpoints
- one relay provider with multiple POPs or ingress URLs

Use a `station` when you want a routing decision point.

Examples:

- `right-primary`
- `budget-fallback`
- `deep-reasoning`

One provider can feed multiple stations. One station can combine multiple providers.

## Fast / Deep Profiles

Profiles stay intentionally small. Keep them about session intent rather than inventory.

Example:

```toml
[codex.profiles.fast]
station = "right"
service_tier = "priority"
reasoning_effort = "low"

[codex.profiles.deep]
station = "vibe"
model = "gpt-5.4"
reasoning_effort = "high"
```

Current compatibility rules:

- `profile.station` is the public name to use.
- legacy `profile.config` is still accepted as an alias when reading older files.
- profile/station capability compatibility is validated when the config is loaded.

## Compatibility Rules

- Legacy `version = 1` TOML still loads.
- Legacy `v2` names `active_group` and `groups` still load.
- Public control-plane APIs now prefer `station` naming, but legacy `config` API paths remain as compatibility aliases.

## Recommended Upgrade Path

1. Run `codex-helper config migrate --to v2 --compact --dry-run`.
2. Check whether provider names and station names match your mental model.
3. If needed, edit provider/station names manually before writing.
4. Run `codex-helper config migrate --to v2 --compact --write --yes`.
5. Keep using `profiles` for fast/deep mode switching instead of cloning stations only to change `service_tier` or `reasoning_effort`.

# codex-helper (Codex CLI Local Helper / Proxy)

> Put Codex behind a small local “bumper”:  
> centralize all your relays / keys / quotas, auto-switch when an upstream is exhausted or failing, and get handy CLI helpers for sessions, filtering, and diagnostics.

Current version: `v0.13.0`

> 中文说明: `README.md`

---

## Screenshot

![Built-in TUI dashboard](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## Why codex-helper?

codex-helper is a good fit if any of these sound familiar:

- **You’re tired of hand-editing `~/.codex/config.toml`**  
  Changing `model_provider` / `base_url` by hand is easy to break and annoying to restore.

- **You juggle multiple relays / keys and switch often**  
  You’d like OpenAI / Packy / your own relays managed in one place, and a single command to select the “current” one.

- **You discover exhausted quotas only after 401/429s**  
  You’d prefer “auto-switch to a backup upstream when quota is exhausted” instead of debugging failures.

- **You want a CLI way to quickly resume Codex sessions**  
  For example: “show me the last session for this project and give me `codex resume <ID>`.”

- **You want a local layer for redaction + logging**  
  Requests go through a filter first, and all traffic is logged to a JSONL file for analysis and troubleshooting.

---

## Quick Start (TL;DR)

### 1. Install (recommended: `cargo-binstall`)

```bash
cargo install cargo-binstall
cargo binstall codex-helper   # installs codex-helper and the short alias `ch`
```

This installs `codex-helper` and `ch` into your Cargo bin directory (usually `~/.cargo/bin`).  
Make sure that directory is on your `PATH` so you can run them from anywhere.

> Prefer building from source?  
> Run `cargo build --release` and use `target/release/codex-helper` / `ch`.

### 2. One-command helper for Codex (recommended)

```bash
codex-helper
# or shorter:
ch
```

This will:

- Start a Codex proxy on `127.0.0.1:3211`;
- Guard and, if needed, rewrite `~/.codex/config.toml` to point Codex at the local proxy (snapshotting the original config before `switch on` and cleaning the backup after restore so the next cycle captures the latest original state);
- When writing `model_providers.codex_proxy`, set `request_max_retries = 0` by default to avoid double-retry (Codex retries + codex-helper retries); you can override it in `~/.codex/config.toml`;
- Automatically retry/fail over a small number of times for transient failures (429/5xx/network hiccups) and common provider auth/routing failures (e.g. 401/403/404/408) **before any response bytes are streamed to the client** (configurable);
- If `~/.codex-helper/config.toml` / `config.json` is still empty, bootstrap a default upstream from `~/.codex/config.toml` + `auth.json`;
- If running in an interactive terminal, show a built-in TUI dashboard (disable with `--no-tui`; press `q` to quit; use `1-7` to switch pages; use `7` to browse history; on Sessions/History press `t` to view transcript);
- On Ctrl+C, attempt to restore the original Codex config from the backup.

After that, you keep using your usual `codex ...` commands; codex-helper just sits in the middle.

---

## What The Product Is Now

As of the current release, `codex-helper` is no longer just “a local proxy with multi-upstream failover”.

It is now a **Codex-first local control plane**:

- `station` / `provider` management for relays and upstream inventory;
- `profile`-driven intent such as `daily`, `fast`, or `deep`;
- session identity cards that answer which station / upstream / model / fast mode / reasoning setting a Codex session is actually using;
- session-scoped overrides for `model`, `reasoning_effort`, `service_tier`, and station selection;
- runtime health, breaker, and same-station failover semantics;
- an honest LAN / Tailscale “central relay” shape, without pretending remote devices automatically gain access to host-local transcript/session files.

The practical mental model is now:

> `Codex CLI -> codex-helper data plane -> station/profile/session control plane`

---

## Three Core Concepts

### 1. Station

A `station` is the operator-facing routing target: a relay or a grouped provider target that you enable, disable, probe, drain, or quick-switch.

Compatibility note:

- older config/runtime naming still uses `config` in places;
- on current public API / GUI / docs surfaces, you should read that as `station` first.

### 2. Profile

A `profile` is a reusable control template:

- target station
- target model
- `service_tier` / fast mode
- `reasoning_effort`

Think of profiles as intent presets, not just provider presets.

### 3. Session Binding / Override

The session is now the main control object. You can:

- inspect a single-session identity card;
- apply a profile to one session;
- override `model / reasoning_effort / service_tier / station` per session;
- inspect where each effective value came from:
  - session override
  - profile default
  - request payload
  - station mapping
  - runtime fallback

This is the reason session control is now meaningful instead of “guess which Codex process this maps to”.

---

## Control Plane Quick Entry Points

If you want the shortest path to the current feature set, these are the main entry points:

- TUI / GUI
  - `Stations`: station capability, health, breaker, quick switch
  - `Sessions`: session identity, effective route, session overrides
  - `Profiles` / Config v2: profile and station/provider structure management
- Read APIs
  - `GET /__codex_helper/api/v1/capabilities`
  - `GET /__codex_helper/api/v1/snapshot`
  - `GET /__codex_helper/api/v1/sessions`
  - `GET /__codex_helper/api/v1/sessions/{session_id}`
- Control APIs
  - `GET/POST /__codex_helper/api/v1/overrides/session`
  - `POST /__codex_helper/api/v1/overrides/session/profile`
  - `GET /__codex_helper/api/v1/profiles`
  - `GET /__codex_helper/api/v1/stations`
  - `GET/POST /__codex_helper/api/v1/retry/config`

For design/runtime boundaries, read:

- `docs/workstreams/codex-control-plane-refactor/README.md`
- `docs/workstreams/codex-control-plane-refactor/CENTRAL_RELAY.md`
- `docs/workstreams/codex-control-plane-refactor/CONFIG_V2_MIGRATION.md`

---

## LAN / Tailscale Central Relay Mode

The recommended shared deployment shape is not “remote desktop attachment”.

It is:

1. one always-on host runs `codex-helper`;
2. other devices send Codex traffic to that host’s proxy port;
3. GUI or future WebUI attaches to the admin/control-plane port.

Current capability boundary:

- Shareable:
  - station/profile management
  - session identity
  - observed request history
  - session overrides
  - health / breaker / probe visibility
- Host-local only:
  - `~/.codex/sessions`
  - transcript browsing
  - local path opening

Remote admin security boundary:

- loopback access does not require a token;
- non-loopback admin access requires `CODEX_HELPER_ADMIN_TOKEN` on the host;
- clients must send the same token via header `x-codex-helper-admin-token`.

If you plan to use codex-helper across LAN / Tailscale devices, this section matters more than the older “multi-upstream failover” framing below.

---

## Optional: Codex `notify` integration (rate-limited, duration-based)

Codex can invoke an external program for `"agent-turn-complete"` events via the `notify` setting in `~/.codex/config.toml`. codex-helper can act as that program and apply a low-noise policy:

- **D (duration-based)**: only notify when the corresponding proxied request has `duration_ms >= min_duration_ms`;
- **A (aggregation/rate-limit)**: merge bursts and enforce **at most 1 notification per minute** by default.

### 1) Configure Codex to call codex-helper

Add to `~/.codex/config.toml`:

```toml
notify = ["codex-helper", "notify", "codex"]
```

> This is independent from `tui.notifications`. You can use both.

### 2) Enable notifications in `~/.codex-helper/config.toml` (or `config.json`) (default: off)

Add (or edit) the `notify` section:

```toml
[notify]
enabled = true

[notify.system]
enabled = true

[notify.policy]
min_duration_ms = 60000
global_cooldown_ms = 60000
merge_window_ms = 10000
per_thread_cooldown_ms = 180000
```

Notes:

- codex-helper matches the Codex `"thread-id"` to proxy `FinishedRequest.session_id` and uses `/__codex_helper/status/recent` to compute `duration_ms`. If Codex is not routed through codex-helper, duration matching is unavailable and notifications are skipped.
- System notifications are implemented on Windows (toast via `powershell.exe`) and macOS (via `osascript`). Other platforms currently fall back to printing a short line.
- Optional callback sink: set `notify.exec.enabled = true` and `notify.exec.command = ["your-program", "arg1"]` to receive aggregated JSON on stdin.

---

## Common configuration: multi-upstream failover

The most common and powerful way to use codex-helper is to let it **fail over between multiple upstreams automatically** when one is failing or out of quota.

The key idea: put your primary and backup upstreams **in the same config’s `upstreams` array**.

> Note: if you split each provider into its own config and keep them all at the same `level` (e.g. everything is `level = 1`), codex-helper will still prefer the `active` config, but other same-level configs can participate in failover (to avoid a single point of failure).
>
> Important: a **pinned override** (e.g. TUI `p`: session provider override/pinned; older builds may also have a global pinned override) forces `pinned` routing mode and will only use that single config, so it will **not fail over across configs**.  
> If you want “preferred + failover”, use `active` (TUI: `P` global active, or `Enter` on the Configs page) and clear any pinned override.

### Scenario quick matrix

Think of codex-helper config in 2 layers:

1) **Grouping (routing)**: each config has a `level` (1..=10). `active` is preferred. `enabled=false` excludes a config from automatic routing (unless it is the active config).
2) **Strategy (retry)**: controls how codex-helper retries/cools down/probes back.

If you already imported accounts via `codex-helper config overwrite-from-codex --yes` (most common), you usually don’t need to hand-write `[[...upstreams]]`. You only need:

- Grouping: `codex-helper config set-level <name> <level>` + `codex-helper config set-active <name>`
- Strategy: `codex-helper config set-retry-profile <balanced|same-upstream|aggressive-failover|cost-primary>`

> Note: `set-retry-profile` overwrites the whole `[retry]` block. If you want advanced tweaks (e.g. `retry.upstream.max_attempts`, `retry.provider.on_status`, `transport_cooldown_secs`, and guardrails like `never_on_status` / `never_on_class`), apply a profile first, then edit the config file. Retry tweaks must live under the layered `retry.upstream` / `retry.provider` blocks.

| Goal | What to change after import | Suggested retry profile | Notes |
| --- | --- | --- | --- |
| One account, multiple endpoints (auto failover) | Merge multiple endpoints into one config’s `upstreams` (see Template A) | `balanced` | Simplest and most reliable |
| Multiple providers as same-level backups | Keep them at the same `level` (default is 1) and set one `active` (see Template B) | `balanced` | `active` is preferred; other same-level configs still participate in failover |
| Relay-first, direct/official backup | Put relays at `level=1`, direct/official at `level=2` (see Template C) | `balanced` | Degrades across levels; fully cooled configs are skipped when alternatives exist |
| Monthly primary + pay-as-you-go backup (cost) | Same grouping as above, set the monthly relay as `active` (see Template D) | `cost-primary` | Degrade to backup when unstable, and “probe back” via cooldown/backoff |

#### Template A: one config with multiple upstream endpoints

```toml
version = 1

[codex]
active = "codex-main"

[codex.configs.codex-main]
name = "codex-main"
enabled = true
level = 1

[[codex.configs.codex-main.upstreams]]
base_url = "https://codex-api.packycode.com/v1"
auth = { auth_token_env = "PACKYCODE_API_KEY" }
tags = { provider_id = "packycode", source = "codex-config" }

[[codex.configs.codex-main.upstreams]]
base_url = "https://co.yes.vg/v1"
auth = { auth_token_env = "YESCODE_API_KEY" }
tags = { provider_id = "yes", source = "codex-config" }
```

Notes:

- `active` points to this config, so the LB can fail over between multiple upstream endpoints.
- When an upstream fails or is marked `usage_exhausted`, codex-helper prefers other upstreams when possible.

#### Template B: multiple providers as same-level backups (import-first)

```bash
codex-helper config overwrite-from-codex --yes

# Pick a preferred config (still allows same-level failover)
codex-helper config set-active right

codex-helper config set-retry-profile balanced
```

If you prefer editing `config.toml` directly, the equivalent is:

```toml
[codex]
active = "right"

[retry]
profile = "balanced"
```

> Want fewer candidates? Disable configs you don’t want in automatic routing (active is still eligible): `codex-helper config disable some-provider`.

#### Template C: relay-first, direct/official backup (level grouping)

> `right/packyapi/yescode/openai` are just example names; replace them with what you see in `codex-helper config list`.

```bash
codex-helper config overwrite-from-codex --yes

# L1: relays
codex-helper config set-level right 1
codex-helper config set-level packyapi 1
codex-helper config set-level yescode 1

# L2: direct/official backup
codex-helper config set-level openai 2

codex-helper config set-active right
codex-helper config set-retry-profile balanced
```

Equivalent `config.toml` (example):

```toml
[codex]
active = "right"

[codex.configs.right]
level = 1

[codex.configs.openai]
level = 2

[retry]
profile = "balanced"
```

#### Template D: monthly primary + pay-as-you-go backup (cost + probe-back)

> `right/openai` are just example names; replace them with what you see in `codex-helper config list`.

```bash
codex-helper config overwrite-from-codex --yes

# L1: monthly relay (cheap, may be flaky)
codex-helper config set-level right 1
codex-helper config set-active right

# L2: pay-as-you-go direct (more expensive, more reliable)
codex-helper config set-level openai 2

# Cost-primary enables cooldown exponential backoff for probe-back.
codex-helper config set-retry-profile cost-primary
```

Equivalent `config.toml` (example):

```toml
[codex]
active = "right"

[codex.configs.right]
level = 1

[codex.configs.openai]
level = 2

[retry]
profile = "cost-primary"
```

> Note: if a config name contains `-` etc, quote it in TOML, e.g. `[codex.configs."openai-main"]`.

### Level-based multi-config failover (optional)

If you prefer to keep upstreams in separate configs, codex-helper also supports **level-based config grouping**:

- Each config has a `level` (1..=10, lower is higher priority).
- If there are **multiple distinct levels**, codex-helper routes from low to high (lower level is preferred).
- If all configs share the same level, they are treated as same-level candidates: `active` is preferred, but other configs can still be used for failover.
- Within the same level, the `active` config is preferred.
- Set `enabled = false` to exclude a config from automatic routing (unless it is the active config).

A common cost-optimization pattern is “monthly relay as primary, pay-as-you-go as backup”: set the cheaper relay as `active` with `level = 1`, keep your direct/official provider at `level = 2`, and use cooldown penalties (optionally with cooldown backoff) to periodically probe back to the primary without hammering it on every request.

---

## Command cheatsheet

### Daily use

- Start Codex helper (recommended):
  - `codex-helper` / `ch`
- Explicit Codex proxy:
  - `codex-helper serve` (default port 3211)
  - `codex-helper serve --no-tui` (disable the built-in TUI dashboard)
  - `codex-helper serve --host 0.0.0.0` (bind all interfaces; security risk)
  - codex-helper also starts a loopback-only admin API on `proxy_port + 1000` (for example `3211 -> 4211`); local GUI/TUI/notify flows use it automatically.

### Turn Codex on/off via local proxy

- Switch Codex to the local proxy:

  ```bash
  codex-helper switch on
  ```

- Restore original configs from backup:

  ```bash
  codex-helper switch off
  ```

- Inspect current switch status:

  ```bash
  codex-helper switch status
  ```

### Manage upstream configs (providers / relays)

- List configs:

  ```bash
  codex-helper config list
  ```

- Add a new config:

  ```bash
  codex-helper config add openai-main \
    --base-url https://api.openai.com/v1 \
    --auth-token-env OPENAI_API_KEY \
    --alias "Main OpenAI quota"
  ```

- Set the active config:

  ```bash
  codex-helper config set-active openai-main
  ```

- Set a curated retry profile (writes the `[retry]` block; good when you only want “pick a strategy”):

  ```bash
  codex-helper config set-retry-profile balanced
  codex-helper config set-retry-profile cost-primary
  ```

- Level-based routing controls (multi-config failover):
  
  ```bash
  codex-helper config set-level openai-main 1
  codex-helper config disable packy-main
  codex-helper config enable packy-main
  ```

- Overwrite Codex configs from Codex CLI (reset to defaults):
  
  ```bash
  # overwrite codex-helper Codex configs (resets active/enabled/level to defaults)
  codex-helper config overwrite-from-codex --dry-run
  codex-helper config overwrite-from-codex --yes
  ```

### TUI Settings (runtime)

- `R`: reload runtime config now (helps confirm manual edits; next request will use the new config)

### Sessions, usage, diagnostics

- Session helpers (Codex):

  ```bash
  codex-helper session list
  codex-helper session recent
  codex-helper session last
  codex-helper session transcript <ID> --tail 40
  ```

- Usage & logs:

  ```bash
  codex-helper usage summary
  codex-helper usage tail --limit 20
  codex-helper usage tail --limit 20 --raw
  codex-helper usage find --errors --model gpt-5 --retried --limit 10
  codex-helper usage find --session <SESSION_ID> --raw
  ```

  Text output shows station/provider/model, service_tier/fast, input/output/cache/reasoning tokens, duration, TTFB, output speed, and estimated cost when possible. `usage find` filters by session/model/station/provider/status/fast/retry; `--raw` still prints the original JSONL.

- Status & doctor:

  ```bash
  codex-helper status
  codex-helper doctor

  # JSON outputs for scripts / UI integration
  codex-helper status --json | jq .
  codex-helper doctor --json | jq '.checks[] | select(.status != "ok")'
  ```

---

## Example workflows

### Scenario 1: Manage multiple relays / keys and switch quickly

```bash
# 1. Add configs for different providers
codex-helper config add openai-main \
  --base-url https://api.openai.com/v1 \
  --auth-token-env OPENAI_API_KEY \
  --alias "Main OpenAI quota"

codex-helper config add packy-main \
  --base-url https://codex-api.packycode.com/v1 \
  --auth-token-env PACKYCODE_API_KEY \
  --alias "Packy relay"

codex-helper config list

# 2. Select which config is active
codex-helper config set-active openai-main   # use OpenAI
codex-helper config set-active packy-main    # use Packy

# 3. Point Codex at the local proxy (once)
codex-helper switch on

# 4. Start the proxy with the current active config
codex-helper
```

### Scenario 2: Resume Codex sessions by project

```bash
cd ~/code/my-app

codex-helper session list   # list recent sessions for this project
codex-helper session recent # list recent sessions across projects (project_root + session_id per line)
codex-helper session last   # show last session + a codex resume command
codex-helper session transcript <ID> --tail 40   # view recent conversation to identify a session
```

`session list` now includes the conversation rounds (`rounds`) and the last update timestamp (`last_update`, which prefers the last assistant response time when available).

Tip: by default `session list` prints the full first prompt; you can truncate it for a tighter view:

```bash
codex-helper session list --truncate 120
```

`session recent` is designed for fast `codex resume` workflows when you're hopping between repos. By default it shows sessions updated within the last 12 hours (file mtime), newest first:

```bash
codex-helper session recent --since 12h --limit 50
# <project_root> <session_id>
```

For scripts, prefer TSV/JSON output to avoid parsing ambiguity:

```bash
codex-helper session recent --format tsv
codex-helper session recent --format json
```

On Windows, you can also open each session directly (best-effort):

```bash
codex-helper session recent --open --terminal wt --shell pwsh --resume-cmd "codex resume {id}"
```

You can also query sessions for any directory without cd:

```bash
codex-helper session list --path ~/code/my-app
codex-helper session last --path ~/code/my-app
```

This is especially handy when juggling multiple side projects: you don’t need to remember session IDs, just tell codex-helper which directory you care about and it will find the most relevant sessions and suggest `codex resume <ID>`.

---

## Advanced configuration (optional)

Most users do not need to touch these. If you want deeper customization, these files are relevant:

- Main config: `~/.codex-helper/config.toml` (preferred) or `~/.codex-helper/config.json` (legacy). If both exist, `config.toml` wins.
- Filter rules: `~/.codex-helper/filter.json`
- Usage providers: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request logs: `~/.codex-helper/logs/requests.jsonl`
- Detailed debug logs (optional): `~/.codex-helper/logs/requests_debug.jsonl` (only created when `http_debug` split is enabled)
- Session stats cache (auto-generated): `~/.codex-helper/cache/session_stats.json` (speeds up `session list/search` rounds/timestamps; invalidated by session file `mtime+size`—delete this file to force a full rescan if needed)

To quickly generate a commented `config.toml` template:

```bash
codex-helper config init
```

> Notes:
> - The generated template comments are Chinese by default.
> - If `~/.codex/config.toml` is present, codex-helper will best-effort auto-import Codex providers into the generated `config.toml`.
> - Use `codex-helper config init --no-import` for a template-only file.

Codex official files:

- `~/.codex/auth.json`: managed by `codex login`; codex-helper only reads it.
- `~/.codex/config.toml`: managed by Codex CLI; codex-helper touches it only via `switch on/off`.

### Config structure (brief)

codex-helper supports both `config.toml` (preferred) and `config.json` (legacy). If both exist, `config.toml` wins.

```toml
version = 1

[codex]
active = "openai-main"

[codex.configs.openai-main]
name = "openai-main"
alias = "Main OpenAI quota"
enabled = true
level = 1

[[codex.configs.openai-main.upstreams]]
base_url = "https://api.openai.com/v1"
auth = { auth_token_env = "OPENAI_API_KEY" }
tags = { source = "codex-config", provider_id = "openai" }
```

Key ideas:

- `active`: the name of the currently active config;
- `configs`: a map of named configs;
- `level`: priority group for level-based config routing (1..=10, lower is higher priority; defaults to 1);
- `enabled`: whether the config participates in automatic routing (defaults to true);
- each `upstream` is one endpoint, ordered by priority (primary → backups).

### `pricing_overrides.toml`

The bundled price catalog covers common Codex/OpenAI models. If your relay provider uses custom model aliases, prices, or multipliers, add `~/.codex-helper/pricing_overrides.toml`. Overrides replace bundled rows with the same model id and can also add new models; request cost calculation and GUI/TUI pricing views use the merged catalog.

```toml
[models.gpt-5]
display_name = "GPT-5 via relay"
aliases = ["relay-gpt5"]
input_per_1m_usd = "1.10"
output_per_1m_usd = "8.80"
cache_read_input_per_1m_usd = "0.11"
cache_creation_input_per_1m_usd = "0"
confidence = "estimated"

[models.custom-codex]
input_per_1m_usd = "0.50"
output_per_1m_usd = "1.50"
```

When the GUI is running the local proxy, `Stats -> Pricing catalog` can save catalog rows as local overrides, and `Stats -> Local pricing overrides` can edit the same file directly. In attached mode this area stays read-only so the GUI does not write the current machine while showing a remote proxy.

You can also manage this file through the CLI instead of hand-writing TOML:

```bash
codex-helper pricing path
codex-helper pricing list
codex-helper pricing list --local --model gpt-5
codex-helper pricing set custom-codex --input-per-1m-usd 0.50 --output-per-1m-usd 1.50 --confidence estimated
codex-helper pricing sync http://127.0.0.1:4322/__codex_helper/api/v1/pricing/catalog --model relay-gpt5 --dry-run
codex-helper pricing sync http://127.0.0.1:4322/__codex_helper/api/v1/pricing/catalog --model relay-gpt5
codex-helper pricing sync-basellm --model gpt-5 --dry-run
codex-helper pricing sync-basellm --model gpt-5
codex-helper pricing remove custom-codex
```

`pricing sync` pulls `ModelPriceCatalogSnapshot` JSON, the same shape exposed by this project's admin API. It merges matching rows into local overrides by default; add `--replace` to rewrite the local override file from the matched remote rows.

`pricing sync-basellm` pulls `https://basellm.github.io/llm-metadata/api/all.json` and converts its per-million model prices into this project's local override format. Use it to refresh bundled seed prices with an external catalog source while keeping local overrides on top.

### `usage_providers.json`

Path: `~/.codex-helper/usage_providers.json`. If it does not exist, codex-helper will write a default file similar to:

```jsonc
{
  "providers": [
    {
      "id": "packycode",
      "kind": "budget_http_json",
      "domains": ["packycode.com"],
      "endpoint": "https://www.packycode.com/api/backend/users/info",
      "token_env": null,
      "poll_interval_secs": 60
    },
    {
      "id": "my-sub2api",
      "kind": "openai_balance_http_json",
      "domains": ["relay.example.com"],
      "endpoint": "{{base_url}}/user/balance",
      "poll_interval_secs": 60
    },
    {
      "id": "my-new-api",
      "kind": "new_api_user_self",
      "domains": ["newapi.example.com"],
      "endpoint": "{{base_url}}/api/user/self",
      "token_env": "NEW_API_ACCESS_TOKEN",
      "headers": {
        "New-Api-User": "{{userId}}"
      },
      "variables": {
        "userId": "{{env:NEW_API_USER_ID}}"
      },
      "poll_interval_secs": 60
    }
  ]
}
```

For `budget_http_json`:

- up to date usage is obtained by calling `endpoint` with a Bearer token (from `token_env` or the associated upstream’s `auth_token` / `auth_token_env`);
- if the upstream uses `auth_token_env`, the token is read from that environment variable at runtime;
- the response is inspected for fields like `monthly_budget_usd` / `monthly_spent_usd` to decide if the quota is exhausted;
- associated upstreams are then marked `usage_exhausted = true` in LB state; when possible, LB avoids these upstreams.

For the new generic adapters:

- `endpoint` supports `{{base_url}}`, `{{upstream_base_url}}`, `{{token}}` / `{{apiKey}}` / `{{accessToken}}`, `{{env:NAME}}`, and `variables` templates; `{{base_url}}` is normalized to drop a trailing `/v1` when present;
- `openai_balance_http_json` covers the cc-switch style generic template / common sub2api relays: it defaults to `{{base_url}}/user/balance` and reads fields such as `balance`, `remaining`, `credit`, `subscription_balance`, and `pay_as_you_go_balance`;
- `new_api_user_self` covers New API style relays: it defaults to `{{base_url}}/api/user/self` and parses `data.quota` / `data.used_quota`, converting the quota units to USD with the cc-switch-style `500000` divisor by default;
- custom/self-hosted providers can extend the parser with `extract.remaining_balance_paths`, `extract.monthly_spent_paths`, `extract.monthly_budget_paths`, `extract.exhausted_paths`, and divisor fields without touching Rust code;
- `refresh_on_request` controls whether a request finish automatically triggers a balance poll for that provider; it defaults to `true`, and `false` disables the request-driven refresh path;
- `poll_interval_secs` controls the minimum interval between balance polls for that provider; when omitted it defaults to `60`, the current trigger is on-demand polling after a routed request finishes rather than the TUI/GUI repaint loop, values below 20 seconds are clamped up to 20, and `0` disables request-driven refresh entirely;
- after a request finishes, codex-helper polls `endpoint` on demand and stores `ok` / `exhausted` / `stale` / `error` / `unknown` balance snapshots;
- matching upstreams are then marked `usage_exhausted = true` in LB state; when possible, LB avoids these upstreams.

### Filtering & logging

- Filter rules: `~/.codex-helper/filter.json`, e.g.:

  ```jsonc
  [
    { "op": "replace", "source": "your-company.com", "target": "[REDACTED_DOMAIN]" },
    { "op": "remove",  "source": "super-secret-token" }
  ]
  ```

  Filters are applied to the request body before sending it upstream; rules are reloaded based on file mtime.

- Logs: `~/.codex-helper/logs/requests.jsonl`, each line is a JSON object like:

  ```jsonc
  {
    "timestamp_ms": 1730000000000,
    "service": "codex",
    "method": "POST",
    "path": "/v1/responses",
    "status_code": 200,
    "duration_ms": 1234,
    "config_name": "openai-main",
    "upstream_base_url": "https://api.openai.com/v1",
    "usage": {
      "input_tokens": 123,
      "output_tokens": 456,
      "reasoning_tokens": 0,
      "total_tokens": 579
    }
  }
  ```

These fields form a **stable contract**: future versions will only add fields, not remove or rename existing ones, so you can safely build scripts and dashboards on top of them.

When retries happen, logs may also include a `retry` object (e.g. `retry.attempts` and `retry.upstream_chain`) to help you understand which upstreams were tried before the final result.

### Optional HTTP debug logs (for 4xx/5xx)

To help diagnose upstream `400` and other non-2xx responses, codex-helper can optionally attach an `http_debug` object to each log line (request headers, request body preview, upstream response headers/body preview, etc.).

Enable it via env vars (off by default):

- `CODEX_HELPER_HTTP_DEBUG=1`: only write `http_debug` for non-2xx upstream responses
- `CODEX_HELPER_HTTP_DEBUG_ALL=1`: write `http_debug` for all requests (can grow logs quickly)
- `CODEX_HELPER_HTTP_DEBUG_BODY_MAX=65536`: max bytes for request/response body preview (will truncate)
- `CODEX_HELPER_HTTP_DEBUG_SPLIT=1`: write large `http_debug` blobs to `requests_debug.jsonl` and keep only `http_debug_ref` in `requests.jsonl` (recommended when `*_ALL=1`)

You can also print a truncated `http_debug` JSON directly to the terminal on non-2xx responses (off by default):

- `CODEX_HELPER_HTTP_WARN=1`: emit a `warn` log with `http_debug` JSON for non-2xx upstream responses
- `CODEX_HELPER_HTTP_WARN_ALL=1`: emit for all requests (not recommended)
- `CODEX_HELPER_HTTP_WARN_BODY_MAX=65536`: max bytes for body preview used by terminal output (will truncate)

Sensitive headers are redacted automatically (e.g. `Authorization`/`Cookie`). If you need to scrub secrets inside request bodies, consider using `~/.codex-helper/filter.json`.

### Two-layer retry + failover (defaults: 2 attempts per upstream; try up to 2 configs/providers; switch across upstreams within a config)

Some upstream failures are transient (network hiccups, 429 rate limits, 5xx/524, or Cloudflare/WAF-like HTML challenge pages) or provider-specific (common auth/routing failures like 401/403/404/408). codex-helper uses a two-layer model **before any response bytes are streamed to the client**: it retries within the current provider/config first (upstream layer), and if still failing, fails over to other upstreams and then other same-level configs/providers (provider/config layer).

- Strongly recommended: set Codex-side `model_providers.codex_proxy.request_max_retries = 0` so retry/failover happens in codex-helper (and you don’t burn Codex’s default request retries on the same 502). `switch on` writes `0` only when the key is absent.
- Global defaults live under the `[retry]` block in `~/.codex-helper/config.toml` (or `config.json`). Starting from `v0.8.0`, retry parameters are no longer overridable via environment variables.

Example config (`~/.codex-helper/config.toml`, layered overrides; default profile is `balanced`):

```toml
[retry]
profile = "balanced"

[retry.upstream]
max_attempts = 2
strategy = "same_upstream"
backoff_ms = 200
backoff_max_ms = 2000
jitter_ms = 100
on_status = "429,500-599,524"
on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge"]

[retry.provider]
max_attempts = 2
strategy = "failover"
on_status = "401,403,404,408,429,500-599,524"
on_class = ["upstream_transport_error"]

never_on_status = "413,415,422"
never_on_class = ["client_error_non_retryable"]
cloudflare_challenge_cooldown_secs = 300
cloudflare_timeout_cooldown_secs = 60
transport_cooldown_secs = 30
cooldown_backoff_factor = 1
cooldown_backoff_max_secs = 600
```

Note: retries may replay **non-idempotent POST requests** (potential double-billing or duplicate writes). Only enable retries if you accept this risk, and keep the attempt count low.

### Log file size control (recommended)

`requests.jsonl` is append-only by default. To avoid it growing without bound, codex-helper supports automatic log rotation (enabled by default):

- `CODEX_HELPER_REQUEST_LOG_MAX_BYTES=52428800`: maximum bytes per log file before rotating (`requests.jsonl` → `requests.<timestamp_ms>.jsonl`; `requests_debug.jsonl` → `requests_debug.<timestamp_ms>.jsonl`) (default 50MB)
- `CODEX_HELPER_REQUEST_LOG_MAX_FILES=10`: how many rotated files to keep (default 10)
- `CODEX_HELPER_REQUEST_LOG_ONLY_ERRORS=1`: only log non-2xx requests (reduces disk usage; off by default)

---

## Relationship to cli_proxy and cc-switch

- [cli_proxy](https://github.com/guojinpeng/cli_proxy): a multi-service daemon + Web UI with centralized monitoring.
- [cc-switch](https://github.com/farion1231/cc-switch): a desktop GUI supplier/MCP manager focused on “manage configs in one place, apply to many clients”.

codex-helper takes inspiration from both, but stays deliberately lightweight:

- focused on Codex CLI;
- single binary, no daemon, no Web UI;
- designed to be a small CLI companion you can run ad hoc, or embed into your own scripts and tooling.

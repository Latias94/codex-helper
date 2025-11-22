# codex-helper (Codex CLI Local Helper / Proxy)

> Put Codex / Claude Code behind a small local “bumper”:  
> centralize all your relays / keys / quotas, auto-switch when an upstream is exhausted or failing, and get handy CLI helpers for sessions, filtering, and diagnostics.

> 中文说明: `README.md`

---

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
- Guard and, if needed, rewrite `~/.codex/config.toml` to point Codex at the local proxy (backing up the original config on first run);
- If `~/.codex-helper/config.json` is still empty, bootstrap a default upstream from `~/.codex/config.toml` + `auth.json`;
- On Ctrl+C, attempt to restore the original Codex config from the backup.

After that, you keep using your usual `codex ...` commands; codex-helper just sits in the middle.

### 3. Optional: switch the default target to Claude (experimental)

By default, commands assume **Codex**. If you primarily use Claude Code, you can flip the default:

```bash
codex-helper default --claude   # set default target service to Claude (experimental)
```

After this:

- `codex-helper serve` (without flags) will start a **Claude** proxy on `127.0.0.1:3210`;
- `codex-helper config list/add/set-active` (without `--codex/--claude`) will operate on Claude configs.

You can always check the current default with:

```bash
codex-helper default
```

---

## Common configuration: multi-upstream failover

The most common and powerful way to use codex-helper is to let it **fail over between multiple upstreams automatically** when one is failing or out of quota.

The key idea: put your primary and backup upstreams **in the same config’s `upstreams` array**, instead of as separate configs.

Example `~/.codex-helper/config.json`:

```jsonc
{
  "version": 1,
  "codex": {
    "active": "codex-main",
    "configs": {
      "codex-main": {
        "name": "codex-main",
        "alias": null,
        "upstreams": [
          {
            "base_url": "https://codex-api.packycode.com/v1",
            "auth": { "auth_token": "sk-packy-..." },
            "tags": { "provider_id": "packycode", "source": "codex-config" }
          },
          {
            "base_url": "https://co.yes.vg/v1",
            "auth": { "auth_token": "cr-..." },
            "tags": { "provider_id": "yes", "source": "codex-config" }
          }
        ]
      }
    }
  },
  "claude": { "active": null, "configs": {} },
  "default_service": null
}
```

With this layout:

- `active = "codex-main"` → the load balancer chooses between `upstreams[0]` (Packy) and `upstreams[1]` (Yes);
- when an upstream either:
  - exceeds the failure threshold (`FAILURE_THRESHOLD` in `src/lb.rs:6`), or
  - is marked `usage_exhausted = true` by `usage_providers`,
  the LB will prefer the other upstream whenever possible.

---

## Command cheatsheet

### Daily use

- Start Codex helper (recommended):
  - `codex-helper` / `ch`
- Explicit Codex / Claude proxy:
  - `codex-helper serve` (Codex, default port 3211)
  - `codex-helper serve --codex`
  - `codex-helper serve --claude` (Claude, default port 3210)

### Turn Codex / Claude on/off via local proxy

- Switch Codex / Claude to the local proxy:

  ```bash
  codex-helper switch on          # Codex
  codex-helper switch on --claude # Claude (experimental)
  ```

- Restore original configs from backup:

  ```bash
  codex-helper switch off
  codex-helper switch off --claude
  ```

- Inspect current switch status:

  ```bash
  codex-helper switch status
  codex-helper switch status --codex
  codex-helper switch status --claude
  ```

### Manage upstream configs (providers / relays)

- List configs (defaults to Codex, can target Claude explicitly):

  ```bash
  codex-helper config list
  codex-helper config list --claude
  ```

- Add a new config:

  ```bash
  # Codex
  codex-helper config add openai-main \
    --base-url https://api.openai.com/v1 \
    --auth-token sk-openai-xxx \
    --alias "Main OpenAI quota"

  # Claude (experimental)
  codex-helper config add claude-main \
    --base-url https://api.anthropic.com/v1 \
    --auth-token sk-claude-yyy \
    --alias "Claude main quota" \
    --claude
  ```

- Set the active config:

  ```bash
  codex-helper config set-active openai-main
  codex-helper config set-active claude-main --claude
  ```

### Sessions, usage, diagnostics

- Session helpers (Codex):

  ```bash
  codex-helper session list
  codex-helper session last
  ```

- Usage & logs:

  ```bash
  codex-helper usage summary
  codex-helper usage tail --limit 20 --raw
  ```

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
  --auth-token sk-openai-xxx \
  --alias "Main OpenAI quota"

codex-helper config add packy-main \
  --base-url https://codex-api.packycode.com/v1 \
  --auth-token sk-packy-yyy \
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
codex-helper session last   # show last session + a codex resume command
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

- Main config: `~/.codex-helper/config.json`
- Filter rules: `~/.codex-helper/filter.json`
- Usage providers: `~/.codex-helper/usage_providers.json`
- Request logs: `~/.codex-helper/logs/requests.jsonl`

Codex official files:

- `~/.codex/auth.json`: managed by `codex login`; codex-helper only reads it.
- `~/.codex/config.toml`: managed by Codex CLI; codex-helper touches it only via `switch on/off`.

### `config.json` structure (brief)

```jsonc
{
  "codex": {
    "active": "openai-main",
    "configs": {
      "openai-main": {
        "name": "openai-main",
        "alias": "Main OpenAI quota",
        "upstreams": [
          {
            "base_url": "https://api.openai.com/v1",
            "auth": {
              "auth_token": "sk-...",
              "api_key": null
            },
            "tags": {
              "source": "codex-config",
              "provider_id": "openai"
            }
          }
        ]
      }
    }
  }
}
```

Key ideas:

- `active`: the name of the currently active config;
- `configs`: a map of named configs;
- each `upstream` is one endpoint, ordered by priority (primary → backups).

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
    }
  ]
}
```

For `budget_http_json`:

- up to date usage is obtained by calling `endpoint` with a Bearer token (from `token_env` or the associated upstream’s `auth_token`);
- the response is inspected for fields like `monthly_budget_usd` / `monthly_spent_usd` to decide if the quota is exhausted;
- associated upstreams are then marked `usage_exhausted = true` in LB state; when possible, LB avoids these upstreams.

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

---

## Relationship to cli_proxy and cc-switch

- [cli_proxy](https://github.com/guojinpeng/cli_proxy): a multi-service daemon + Web UI with a broader scope (Codex, Claude, etc.) and centralized monitoring.
- [cc-switch](https://github.com/farion1231/cc-switch): a desktop GUI supplier/MCP manager focused on “manage configs in one place, apply to many clients”.

codex-helper takes inspiration from both, but stays deliberately lightweight:

- focused on Codex CLI (with experimental Claude support);
- single binary, no daemon, no Web UI;
- designed to be a small CLI companion you can run ad hoc, or embed into your own scripts and tooling.

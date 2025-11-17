# codex-helper (Codex CLI Local Helper / Proxy)

> A Rust-based local helper / local proxy for Codex CLI traffic. It centralizes multiple upstream providers/keys/endpoints, switches safely when quotas are exhausted or endpoints fail, and ships with handy CLI tools for sessions and filtering.

> 中文说明: [README.md](README.md)

## What can codex-helper do?

- **Seamlessly put Codex behind a local proxy**
  - `codex-helper switch-on` rewrites `~/.codex/config.toml` once so that all Codex traffic goes through the local proxy;
  - It safely backs up the original config and can restore it with `switch-off`.

- **Manage multiple keys / providers / relays in one place**
  - All upstream configurations live in `~/.codex-proxy/config.json`;
  - You can define multiple Codex configs (OpenAI, PackyCode, self-hosted relays, etc.), each with its own upstream pool;
  - Switch which config is active via `config set-active` (then restart `codex-helper serve`—Codex sessions are persisted by Codex itself).

- **Usage-aware routing (“auto switch when quota is exhausted”)**
  - A pluggable “usage provider” layer can query provider quotas and mark upstreams as “exhausted”;
  - Ships with a default provider for **PackyCode** (configured via domains, not hardcoded in LB):
    - `~/.codex-proxy/usage_providers.json` is auto-generated with a `packycode` entry;
    - Upstreams whose `base_url` host matches `packycode.com` are associated with this provider;
    - The provider calls Packy’s budget API and decides whether the monthly quota is exhausted;
    - It reuses the same token Codex uses for that upstream (or an env override).
  - LB behavior:
    - Normal path: prefer upstreams that are **not exhausted** and **not in cooldown**, in priority order;
    - If all upstreams are marked exhausted: fallback mode ignores the exhausted flag and only respects failure/cooldown so there is always a last-resort upstream.

- **Session helpers for Codex**
  - `codex-helper session list` scans `~/.codex/sessions` and lists recent sessions that match the current project (cwd / ancestors / descendants) first;
  - `codex-helper session last` jumps directly to the last session for the current project and prints a ready-to-copy `codex resume <ID>` command.

- **Request filtering and structured request logging**
  - Reads redaction/removal rules from `~/.codex-proxy/filter.json` before sending request bodies upstream;
  - Logs every request to `~/.codex-proxy/logs/requests.jsonl` with method, path, status, duration, and usage metrics so you can inspect or aggregate with tools like `jq`.

- **(Experimental) Claude Code support**
  - Can bootstrap Claude upstreams from `~/.claude/settings.json` (or `claude.json`) by reading `env.ANTHROPIC_AUTH_TOKEN/ANTHROPIC_API_KEY` and `env.ANTHROPIC_BASE_URL`;
  - Supports `codex-helper switch-on --claude` / `serve --claude` to point Claude Code at the local proxy, with backups and guards around `settings.json`;
  - This behavior follows cc-switch’s directory/field conventions but is still experimental and may need adjustments as Claude evolves.

## Install & Run

### 1. Build the binary

From the project root:

```bash
cargo build --release
```

The resulting binary is at:

```bash
target/release/codex-helper
```

You may want to add it to your `PATH` so you can run `codex-helper` directly.

### 2. Point Codex at the local proxy (once)

Run:

```bash
codex-helper switch-on
```

This will:

- Read `~/.codex/config.toml`.
- Backup to `~/.codex/config.toml.codex-proxy-backup` if not already present.
- Insert the following into `[model_providers]` and set `model_provider`:

```toml
[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"

model_provider = "codex_proxy"
```

You can override the port via:

```bash
codex-helper switch-on --port <PORT>
```

To restore your original Codex configuration:

```bash
codex-helper switch-off
```

### 3. Start the proxy server

Codex (default):

```bash
codex-helper serve
codex-helper serve --port 3211
```

On startup:

- The proxy listens on `127.0.0.1:<port>`.
- In Codex mode, on first run it bootstraps a default upstream from `~/.codex`:
  - Uses the current `model_provider`.
  - Resolves its `env_key` / `auth.json` into an upstream `auth_token`.
- If it cannot resolve a valid token for Codex upstreams, the process fails fast with an error.

### 4. Session helpers (Codex)

List recent sessions for the current project:

```bash
codex-helper session list
codex-helper session list --limit 20
```

- Reads `~/.codex/sessions/**/rollout-*.jsonl`.
- Prefers sessions whose recorded `cwd` matches the current directory, one of its ancestors, or one of its descendants.
- Displays:
  - `id` (full, on its own line for easy copy).
  - `updated` (last activity timestamp).
  - `cwd` (session working directory).
  - `prompt` (short preview of the first user message).

Jump directly to the last session for the current project:

```bash
codex-helper session last
```

Example output:

```text
Last Codex session for current project:
  id: 1234-...-abcd
  updated_at: 2025-04-01T10:30:00Z
  cwd: /Users/you/project
  first_prompt: your first message ...

Resume with:
  codex resume 1234-...-abcd
```

## Configuration & Pools

### Files

- Main config: `~/.codex-proxy/config.json`
  - Contains `codex` configurations.
- Filter rules: `~/.codex-proxy/filter.json`
- Usage providers: `~/.codex-proxy/usage_providers.json`
- Request logs: `~/.codex-proxy/logs/requests.jsonl`

### `config.json` layout (brief)

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

- `name`: config ID (map key).
- `alias`: optional display name.
- `upstreams`: upstream pool (order = priority):
  - `base_url`: upstream API base URL.
  - `auth.auth_token`: used as `Authorization: Bearer <token>`.
  - `auth.api_key`: optional extra API key header.
  - `tags`: optional metadata (e.g., provenance).

### CLI configuration commands

List configs:

```bash
codex-helper config list
```

Add a new config:

```bash
codex-helper config add openai-main \
  --base-url https://api.openai.com/v1 \
  --auth-token sk-xxx \
  --alias "Main OpenAI quota"
```

Set active config:

```bash
codex-helper config set-active openai-main
```

## Usage Providers

### `usage_providers.json`

Path: `~/.codex-proxy/usage_providers.json`. If it does not exist, the proxy will create a default file similar to:

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

Fields:

- `id`: provider ID (for logging / distinction).
- `kind`: currently `budget_http_json`:
  - Expects a JSON response with budget/spent fields and determines whether the quota is exhausted.
- `domains`: any upstream whose `base_url` host matches one of these domains is associated with this provider.
- `endpoint`: usage API endpoint.
- `token_env`: optional env var name to override the token.
- `poll_interval_secs`: polling interval in seconds (default 60).

### Token resolution

For each provider:

1. If `token_env` is set and the env var is non-empty, that value is used.
2. Otherwise, it scans associated upstreams and takes the first non-empty `auth.auth_token` as its Bearer token.

This means that for the common case of a single token:

- Whatever token Codex uses to talk to that upstream (e.g., Packy) is reused for the usage API, without needing a separate token.

### Impact on LB

- For `budget_http_json`:
  - The provider reads `monthly_budget_usd` and `monthly_spent_usd` from the JSON response.
  - If `monthly_budget_usd > 0` and `monthly_spent_usd >= monthly_budget_usd`, the provider treats the quota as exhausted.
- All associated upstreams then get `usage_exhausted = true` in the LB state.
- LB behavior:
  - Normal path:
    - Excludes upstreams that are:
      - In failure cooldown, or
      - Marked as `usage_exhausted`.
  - Fallback path:
    - If all upstreams are marked exhausted, the LB ignores `usage_exhausted` and only respects failure/cooldown.
    - This ensures there is always an upstream to fall back to.

## Request Filtering & Logging

### Filtering: `~/.codex-proxy/filter.json`

Example:

```jsonc
[
  { "op": "replace", "source": "your-company.com", "target": "[REDACTED_DOMAIN]" },
  { "op": "remove",  "source": "super-secret-token" }
]
```

- Filters are applied to the request body before sending it upstream.
- The file is monitored via mtime; updates are picked up within about one second.

### Logging: `~/.codex-proxy/logs/requests.jsonl`

Each line is a JSON object, e.g.:

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

You can use tools like `jq` to aggregate usage by config, upstream, or time window.

## Relationship to cli_proxy and cc-switch

codex-helper is inspired by, and designed to work well alongside, both [cli_proxy](https://github.com/guojinpeng/cli_proxy) and [cc-switch](https://github.com/farion1231/cc-switch), but focuses on being a lightweight, Rust-based local proxy and config manager specifically for Codex CLI traffic.

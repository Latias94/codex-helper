# codex-helper

A local relay proxy and operator console for Codex CLI.

codex-helper puts a local proxy between Codex and your upstream providers. It lets you manage multiple relays, keys, balances, request logs, cost estimates, and fallback policies without interrupting the normal Codex workflow.

Current release: `v0.15.0`

中文说明: [README.md](README.md)

![Built-in TUI dashboard](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## Who Is It For?

Use codex-helper if:

- you use multiple Codex/OpenAI-compatible relays and do not want to keep editing `~/.codex/config.toml`;
- you want monthly relays first, then pay-as-you-go or official providers as fallback;
- you want TUI/GUI visibility into provider choice, balance/plan, tokens, cache tokens, latency, retries, and estimated cost;
- you run a local proxy for long periods and need bounded runtime state plus rotated logs;
- you want quick helpers for local Codex session discovery and resume.

It is probably unnecessary if you only use one official account and do not need provider switching or request observability.

## Main Features

- **Local proxy**: listens on `127.0.0.1:3211` by default.
- **Safe Codex patching**: only touches the local proxy fields in `~/.codex/config.toml`; unrelated Codex edits are preserved.
- **Provider / routing config**: `version = 5` route graph schema. Define providers once, then use routing entry/routes for order, pinning, grouping, or tag preference.
- **Session affinity and failover**: each Codex session tries to keep using the selected provider, then falls through to other route candidates when requests fail, upstreams are unavailable, or trusted balance snapshots are exhausted.
- **Balance and plan visibility**: probes common Sub2API, New API, and `/user/balance` endpoints; lookup failures are not treated as exhausted.
- **Outbound proxy compatibility**: the local proxy and outbound network proxy are separate layers; outbound requests currently follow system/environment proxy variables, with no first-class `config.toml` proxy section yet.
- **Request observability**: provider, model, tokens, cache tokens, cache hit rate, TTFB, duration, output rate, retry chain, and estimated cost.
- **TUI and GUI**: built-in TUI for terminal use; GUI for local or attached operation.

## Quick Start

### Install

```bash
cargo install cargo-binstall
cargo binstall codex-helper
```

This installs both `codex-helper` and the short alias `ch`.

Build from source:

```bash
cargo build --release
```

### Run

```bash
codex-helper
# or
ch
```

By default this will:

- start the local proxy;
- initialize or migrate `~/.codex-helper/config.toml`, backing up the old file as `.bak` first;
- patch Codex to use `model_providers.codex_proxy` when needed;
- open the TUI in interactive terminals;
- remove only the codex-helper proxy patch on exit.

Start the proxy without the TUI:

```bash
codex-helper serve --no-tui
```

Manage the Codex proxy patch explicitly:

```bash
codex-helper switch on
codex-helper switch status
codex-helper switch off
```

## Minimal Config

The recommended path is to edit config through CLI commands:

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

The resulting `~/.codex-helper/config.toml` stays small:

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

Common routing policies:

| Goal | Command | Notes |
| --- | --- | --- |
| Pin one provider | `codex-helper routing pin input` | Temporary manual steering |
| Ordered fallback | `codex-helper routing order input openai` | Best default for most users |
| Monthly first | `codex-helper routing prefer-tag --tag billing=monthly --order input,openai --on-exhausted continue` | Falls back after known exhaustion |
| Monthly stop-loss | Same command with `--on-exhausted stop` | Avoids silent pay-as-you-go spillover |
| Monthly pool + paygo fallback | Use nested route nodes in TOML | Keeps `monthly_pool -> paygo` explicit |

For complete config, migration, balance adapters, pricing, and GUI/TUI editing notes, see [docs/CONFIGURATION.md](docs/CONFIGURATION.md). The equivalent Chinese reference is [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md).

## Proxy Notes

codex-helper has two proxy layers:

- **Local proxy**: Codex connects to `127.0.0.1:3211`, then codex-helper chooses a provider through routing. When the Codex patch is enabled, requests still pass through this local proxy server even if you do not configure any outbound network proxy.
- **Outbound network proxy**: codex-helper may use a network proxy when connecting to provider endpoints, relays, or balance APIs. There is not yet a dedicated `config.toml` section for this; the underlying HTTP client follows system/environment variables such as `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY`.

See [Local Proxy Vs Outbound Proxy](docs/CONFIGURATION.md#local-proxy-vs-outbound-proxy) for details.

## Common Commands

```bash
# provider / routing
codex-helper provider list
codex-helper provider show input
codex-helper provider disable input
codex-helper provider enable input
codex-helper routing show
codex-helper routing explain

# sessions
codex-helper session list
codex-helper session recent
codex-helper session last
codex-helper session transcript <SESSION_ID> --tail 40

# request logs and usage
codex-helper usage summary
codex-helper usage tail --limit 20
codex-helper usage find --errors --limit 10

# pricing
codex-helper pricing list
codex-helper pricing sync-basellm --model gpt-5 --dry-run

# diagnostics
codex-helper status
codex-helper doctor
codex-helper --version
```

## UI Entry Points

### TUI

`codex-helper` opens the TUI by default in interactive terminals.

Useful pages:

- `Overview`: proxy status, current sessions, and recent requests.
- `Routing` / `Stations`: route graph, provider order, balance/plan, tags, health, and routing preview.
- `Sessions`: session identity, effective route, route affinity, and per-session overrides.
- `Stats` / `Requests`: tokens, cache tokens, cache hit rate, latency, retries, cost, and request logs.

Shortcut hints are shown at the bottom. Under v5 config, durable provider/routing edits should go through the routing page, provider/routing CLI commands, or raw TOML. Press `R` after manual config edits to reload runtime config.

### GUI

When built with the GUI feature:

```bash
codex-helper-gui
# or from source:
cargo run --release --features gui --bin codex-helper-gui
```

The GUI can start or attach to a proxy, edit common single-endpoint providers, route nodes, and routing, and inspect requests, balances, pricing, sessions, health, breaker state, and control-plane status. Complex multi-endpoint providers, model mappings, and advanced fields should still be edited through CLI or raw TOML.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Balance adapters: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request filter: `~/.codex-helper/filter.json`
- Request log: `~/.codex-helper/logs/requests.jsonl`
- GUI config: `~/.codex-helper/gui.toml`

Codex-owned files remain owned by Codex:

- `~/.codex/auth.json`
- `~/.codex/config.toml`

codex-helper only touches the local proxy fields in `~/.codex/config.toml`.

## Design Boundaries

codex-helper intentionally avoids:

- one full Codex config per provider;
- guessing billing class from provider names;
- pretending speed-first or cost-first routing is reliable before real measurements exist;
- treating a balance lookup failure as provider exhaustion;
- letting UI saves silently drop advanced provider fields.

## More Docs

- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): English configuration reference, routing, balance adapters, pricing, migration.
- [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md): Chinese configuration reference with routing recipes, balance adapters, proxy notes, and migration.
- [CHANGELOG.md](CHANGELOG.md): release notes and upgrade notes.
- [docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md](docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md): comparison against cc-switch, aio-coding-hub, and all-api-hub.
- [docs/workstreams/codex-control-plane-refactor/README.md](docs/workstreams/codex-control-plane-refactor/README.md): control-plane design notes.

## References

codex-helper borrows good ideas from these projects while staying focused on Codex CLI local relay and control-plane workflows:

- [cc-switch](https://github.com/farion1231/cc-switch): provider UX, balance/quota templates, request usage visibility.
- [aio-coding-hub](https://github.com/dyndynjyxa/aio-coding-hub): multi-CLI gateway, request chain, cost stats, provider observability.
- [all-api-hub](https://github.com/qixing-jk/all-api-hub): Sub2API / New API balance, usage, and account adapter experience.

# codex-helper

A local relay proxy and operator console for Codex CLI, focused on two jobs: managing multiple relays and keeping Codex as close as possible to the native ChatGPT-backed experience while those relays are in use.

Some Codex features do not appear just because `/responses` can be forwarded. ChatGPT auth shape, OpenAI provider identity, `/models` metadata, `/responses/compact`, and hosted `image_generation` all affect what Codex decides to expose. Some sub2api-style and other relays also return shapes that work for normal chat but are not quite what Codex expects.

codex-helper keeps that compatibility layer local. Codex talks to the helper proxy, and the helper picks OpenAI or one of your relays through provider/routing config. It also handles model-list translation, client presets, capability diagnostics, balance visibility, and fallback policy.

Current release: `v0.18.0`

中文说明: [README.md](README.md)

![Built-in TUI dashboard](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## Who Is It For?

Use codex-helper if:

- you use multiple Codex/OpenAI-compatible relays and do not want to keep editing `~/.codex/config.toml`;
- you want monthly relays first, then pay-as-you-go or official providers as fallback;
- you want Codex to keep ChatGPT login/account behavior for the app or mobile flow, while model traffic uses your own relay or monthly quota;
- your sub2api-style or other relay works for ordinary chat but is shaky around `/models`, `/responses/compact`, hosted `image_generation`, or provider-specific model names;
- you want TUI/GUI visibility into provider choice, balance/plan, tokens, cache tokens, latency, retries, and estimated cost;
- you run a local proxy for long periods and need bounded runtime state plus rotated logs;
- you want quick helpers for local Codex session discovery and resume.

It is probably unnecessary if you only use one official account and do not need provider switching or request observability.

## Main Features

- **Local proxy**: listens on `127.0.0.1:3211` by default.
- **Safe Codex patching**: only touches the local proxy fields in `~/.codex/config.toml`; unrelated Codex edits are preserved.
- **Native Codex presets**: `chatgpt-bridge` keeps ChatGPT login shape, `imagegen-bridge` exposes hosted image generation, and `official-relay` / `official-imagegen` let relays that forward official Responses semantics try remote compaction v1; `responses_websocket` is a separate transport switch for Responses WebSocket v2.
- **OpenAI Images-compatible entrypoint**: the local proxy also exposes `POST /v1/images/generations` and JSON `POST /v1/images/edits`, translates them into Responses hosted `image_generation` requests, and keeps using the same provider routing / fallback chain for local skills and scripts.
- **Relay capability diagnostics**: TUI, CLI, and admin API checks for `/models`, `/responses`, and `/responses/compact`, then recommends the preset that matches the selected relay.
- **Provider / routing config**: `version = 5` route graph schema. Define providers once, then use routing entry/routes for order, pinning, grouping, or tag preference.
- **Session affinity and failover**: each Codex session tries to keep using the selected provider, then falls through to other route candidates when requests fail, upstreams are unavailable, or trusted balance snapshots are exhausted.
- **Balance and plan visibility**: probes common Sub2API, New API, and `/user/balance` endpoints; lookup failures are not treated as exhausted.
- **Outbound proxy compatibility**: the local proxy and outbound network proxy are separate layers; outbound requests currently follow system/environment proxy variables, with no first-class `config.toml` proxy section yet.
- **Request observability**: provider, model, tokens, cache tokens, cache hit rate, TTFB, duration, output rate, retry chain, and estimated cost.
- **TUI and GUI**: built-in TUI for terminal use; `codex-helper-gui`/egui remains available as an optional legacy GUI entrypoint. The Tauri desktop source lives under `apps/desktop` and has passed Windows packaged smoke, but v0.18.0 does not ship a public desktop installer yet; that release path is deferred until signing, release-channel, and rollback operations are ready.

## Quick Start

### Install

Recommended: install prebuilt binaries with the release installer scripts. Rust is not required.

macOS / Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Latias94/codex-helper/releases/download/v0.18.0/codex-helper-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Latias94/codex-helper/releases/download/v0.18.0/codex-helper-installer.ps1 | iex"
```

This installs `codex-helper`, the short alias `ch`, and the optional legacy GUI entrypoint `codex-helper-gui` (egui, deprecated but retained). The Tauri desktop client remains a source-tree preview in v0.18.0 and is not uploaded as a public release artifact; local validation can still build it from `apps/desktop` with `pnpm tauri:build`.

If you do not want to pipe a shell script, download the archive for your platform from [GitHub Releases](https://github.com/Latias94/codex-helper/releases) and verify it with the matching `.sha256` file.

Rust users can also install with `cargo-binstall`:

```bash
cargo install cargo-binstall
cargo binstall codex-helper
```

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

Advanced: run a resident/attached proxy. Only the explicit `--resident`/`daemon`/`tui` subcommands let the proxy outlive the current console:

```bash
codex-helper serve --resident
codex-helper daemon status
codex-helper daemon stop
codex-helper tui --codex
```

By default, `codex-helper serve` and the GUI follow “the console owns the proxy”: exiting the UI stops the proxy it started and restores the local client patch. `daemon status/stop` is only for resident proxies you explicitly started. The `tui` subcommand attaches read-only to an existing resident proxy, so exiting that attached TUI does not stop the proxy. For automatic restart after child crashes, run `codex-helper daemon supervise --codex`; the supervisor records lightweight crash markers under `~/.codex-helper/run/`.

`daemon status` best-effort shows the resident proxy owner marker (manual CLI, supervisor, or a future desktop/tray owner). The marker is only observability metadata: read or cleanup failures never block proxy startup or shutdown. A hidden managed sidecar mode is reserved for the future desktop shell, so ordinary users do not need to choose it manually.

The Tauri desktop client uses a more Clash-like resident-client lifecycle: closing the main window hides it to the tray, `Quit App` exits only the desktop process, and stopping the proxy remains an explicit `Stop Proxy` action. The Windows NSIS packaged path has passed isolated lifecycle smoke, but it is not part of the v0.18.0 public release; macOS/Linux packaged parity, signing, and rollback operations still need separate follow-up work.

Manage the Codex proxy patch explicitly:

```bash
codex-helper switch on
codex-helper switch on --preset chatgpt-bridge
codex-helper switch on --preset official-relay
codex-helper switch on --preset official-relay --responses-websocket
codex-helper switch on --preset official-imagegen
codex-helper switch status
codex-helper switch off
```

NAS / remote relay targets:

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url http://nas.local:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN \
  --preset official-relay

ch relay list
ch relay status nas
ch relay nas
ch relay nas --no-tui
ch relay nas --attach-only
ch relay off
```

Plain `ch` still starts the local foreground helper. `ch relay local` is the explicit target form for the same local flow. `ch relay <name>` patches the local Codex client to the remote proxy and attaches a local TUI to that target's admin API. Use `--no-tui` for switch-only and `--attach-only` for observe-only. Admin tokens are read from the environment variable named by `--admin-token-env`; token values are not written to `~/.codex-helper/config.toml`. The container/NAS side should set `advertised-admin-base-url`, or the client should pass `--admin-url` when adding the target.

A remote target does not give the server access to this client's local Codex transcript/session files. Container deployments keep host-local transcript/session capabilities disabled unless those paths are explicitly mounted and enabled by server policy.

Preset choices:

| Preset | Use it when | Effect |
| --- | --- | --- |
| `default` | You only need the local proxy, multiple providers, and fallback | Codex sends model requests to the local helper; helper picks the upstream |
| `chatgpt-bridge` | You are already signed in to ChatGPT in official Codex and want app/mobile account behavior, but model traffic should use a relay | Keeps the ChatGPT auth shape while upstream credentials still come from helper config |
| `imagegen-bridge` | The relay does not support official provider identity, but you want Codex to expose hosted `image_generation` | Writes the empty `{}` auth facade and does not require official login |
| `official-relay` | The relay forwards official OpenAI Responses semantics, especially `/responses/compact` | Makes Codex treat the local helper as an OpenAI provider so it can try remote compaction v1 |
| `official-imagegen` | The relay is backed by an official subscription account and supports both `/responses/compact` and hosted image generation | Combines OpenAI provider identity with the imagegen auth facade |

`chatgpt-bridge` requires a completed ChatGPT login in official Codex first. If `~/.codex/auth.json` lacks the full token, email, and account metadata, codex-helper refuses the patch instead of leaving Codex in a half-login state.

`official-relay` and `official-imagegen` are experimental. They only change how Codex chooses client-side capabilities; the relay still has to support the underlying endpoints. Real request credentials come from `~/.codex-helper/config.toml`, and the bridge presets do not forward Codex ChatGPT tokens to third-party relays that do not have helper-side credentials. Legacy names `official-relay-bridge` / `official-imagegen-bridge` are still accepted as aliases, but are no longer the recommended spelling.

To avoid degrading capable relays, codex-helper normalizes compressed HTTP request bodies before routing by default (`zstd`, `gzip` / `x-gzip`, `br`, and `deflate`). For Codex `/responses`, `/responses/compact`, and Responses WebSocket, helper also completes missing `session_id`, `x-session-id`, official `session-id` / `thread-id`, and `prompt_cache_key` fields from existing request evidence: header session ids, body `session_id`, `prompt_cache_key`, or `metadata.session_id`. `previous_response_id` is only used for stale-response repair, not as a session identity source. It does not invent a synthetic session id and does not overwrite session fields the client already sent.

Selected provider endpoint affinity is persisted under helper state so a helper restart does not silently move a Codex remote-compaction session to a different provider endpoint. State-bound compact requests, including v1 compact bodies carrying `encrypted_content`, `previous_response_id`, or `compaction_summary` and remote compaction v2 requests carrying `compaction_trigger`, use the known route affinity or fail with an explicit continuity error. Helper keeps this provider-opaque: it does not infer whether a relay is OpenAI, sub2api, New API, or another intermediary. This keeps relay stickiness through `/responses`, `/responses/compact`, and v2 compact's `/responses` request shape; it does not add compact or WebSocket support to relays that lack those endpoints. For rare relays that require the original compressed Codex body, run helper with `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough`.

Codex request semantics also include two targeted repairs: if an upstream explicitly says a `previous_response_id` response no longer exists, helper removes that field and retries the same upstream once; if a relay ignores `Accept-Encoding: identity` and returns gzip JSON, helper decodes it before forwarding plain JSON. `service_tier` remains observational and attribution-only: logs distinguish requested / effective / actual values, but helper default config does not rewrite the client's fast-mode request tier.

Assuming the upstream supports the required endpoints, `official-imagegen` is the most complete preset. If the upstream also passes Responses WebSocket v2 smoke, adding `responses_websocket` is the closest current setup to the official experience:

```text
default
< chatgpt-bridge / imagegen-bridge
< official-relay
< official-imagegen
< official-imagegen + responses_websocket
```

Do not enable the strongest combination blindly: `official-imagegen` requires the relay to support `/responses`, `/responses/compact`, and hosted `image_generation`; `responses_websocket` additionally requires a passing WebSocket live smoke.

If the upstream is known to support Responses WebSocket v2, enable `responses_websocket = true` or `--responses-websocket` separately; it is a transport switch, not a preset.

The proxy also exposes OpenAI Images-compatible generation and reference-image edit entrypoints for skills or scripts that should not depend on whether the Codex client exposed its hosted tool:

```bash
curl 'http://127.0.0.1:3211/v1/images/generations' \
  -X POST \
  -H 'Content-Type: application/json' \
  --data-raw '{
    "model": "gpt-image-2",
    "prompt": "a cat under neon lights on a rainy night",
    "size": "3840x2160",
    "output_format": "png",
    "quality": "high"
  }'
```

Reference-image mode uses JSON `POST /v1/images/edits`. The `images` array accepts objects such as `{"image_url":"..."}` or `{"file_id":"..."}`, and may also contain direct image URL / data URL strings. Helper turns those references into Responses `input_image` content:

```bash
curl 'http://127.0.0.1:3211/v1/images/edits' \
  -X POST \
  -H 'Content-Type: application/json' \
  --data-raw '{
    "model": "gpt-image-2",
    "prompt": "draw the reference character as a messy full-page sketchbook sheet",
    "images": [
      {"image_url": "data:image/png;base64,..."}
    ],
    "size": "2160x2880",
    "output_format": "png",
    "quality": "high",
    "input_fidelity": "high"
  }'
```

Internally both entrypoints still use `/v1/responses` plus hosted `image_generation`, so the real upstream must support that capability. The first version supports a single generated result (`n=1`). JSON edits do not parse masks; JSON requests with `mask` and multipart edits pass through as ordinary proxy requests. Responses use the OpenAI Images-style `data[0].b64_json` shape.

Note: any change to `~/.codex/config.toml` is only picked up by newly started Codex sessions. After changing it, fully restart the Codex App, TUI, or `codex exec` session.

If you want Codex to stay logged into ChatGPT while the actual conversation/model traffic goes through a relay, split the setup into two layers:

1. Use `chatgpt-bridge` to keep the Codex App on the ChatGPT auth path.
2. `codex-helper switch on --preset chatgpt-bridge` points Codex's own `~/.codex/config.toml` at the local `codex_proxy`.
3. Configure `codex.providers.*` and `codex.routing` in `~/.codex-helper/config.toml` so codex-helper selects your relay.
4. If the relay expects prefixed model names, add `model_mapping` on the provider.

This split is for setups where Codex App, mobile, and subscription-gated account checks should still see ChatGPT auth, while day-to-day conversation, tool, and imagegen model usage consumes your relay or monthly quota.

The Codex-side local proxy entry is normally written by `switch on`; avoid hand-editing it over unrelated Codex settings:

```toml
# ~/.codex/config.toml
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
```

The codex-helper side only owns upstreams and routing:

```toml
# ~/.codex-helper/config.toml
version = 5

[codex.client_patch]
preset = "chatgpt-bridge"
responses_websocket = false

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"

[codex.routing]
entry = "relay_first"

[codex.routing.routes.relay_first]
strategy = "ordered-failover"
children = ["relay"]
```

Codex App mobile remote control is a separate path, not the same as `chatgpt-bridge`:

```bash
codex-helper switch remote-control enable
codex-helper switch remote-control status
codex-helper switch remote-control check-logs
```

This writes `remote_connections = true` under `~/.codex/config.toml`'s `[features]` table, does not write `remote_control = true`, and then backs up and updates `local_app_server_feature_enablement.remote_control` inside `~/.codex/sqlite/codex-dev.db`. After that, fully restart the Codex app, then use `check-logs` to confirm `experimentalFeature/enablement/set` appeared at least once with `errorCode=null`. Mobile login still requires MFA on the ChatGPT account.

If a relay expects provider-prefixed model names, add provider-scoped model mapping:

```bash
codex-helper provider add relay --base-url https://relay.example/v1 --auth-token-env RELAY_API_KEY --supported-model gpt-5.5 --model-map gpt-5.5=openai/gpt-5.5
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

Provider or endpoint concurrency caps can protect relay accounts with small upstream limits:

```toml
[codex.providers.input.limits]
max_concurrent_requests = 5
limit_group = "input-account"
```

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
codex-helper session list --truncate 120
codex-helper session search "remote_control"
codex-helper session search "remote_control" --truncate 120
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
codex-helper codex relay-capabilities --preset official-imagegen --model gpt-5.5
codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5
codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --provider ciii --compact-v2
codex-helper codex relay-evidence --limit 20
codex-helper --version
```

## UI Entry Points

### TUI

`codex-helper` opens the TUI by default in interactive terminals.

Useful pages:

- `Overview`: proxy status, current sessions, and recent requests.
- `Routing` / `Stations`: route graph, provider order, balance/plan, tags, health, and routing preview.
- `Sessions`: session identity, effective route, route affinity, and per-session overrides.
- `Usage` / `Requests`: provider usage, recent endpoint samples, balance/quota state, tokens, cache tokens, latency, retries, cost, and request logs.

Shortcut hints are shown at the bottom. Under v5 config, durable provider/routing edits should go through the routing page, provider/routing CLI commands, or raw TOML. Press `R` after manual config edits to reload runtime config.

### GUI

When built with the GUI feature:

```bash
codex-helper-gui
# or from source:
cargo run --release --features gui --bin codex-helper-gui
```

The egui GUI is deprecated and kept as a legacy fallback. It can still start or explicitly attach to a proxy, edit common single-endpoint providers, route nodes, and routing, and inspect requests, balances, pricing, sessions, health, breaker state, and control-plane status. By default, a GUI-started proxy stops when the GUI exits; attaching to an existing proxy must be selected explicitly, and closing the GUI only detaches instead of stopping someone else’s process. Complex multi-endpoint providers, model mappings, and advanced fields should still be edited through CLI or raw TOML.

The new Tauri desktop client lives under `apps/desktop` and uses React 19, Tailwind CSS 4, shadcn/ui-style components, and TanStack Router/Query/Table. It already implements Dashboard, Providers, Usage, Settings, read-only admin data, safe control actions, close-to-tray semantics, single instance, launch-at-login settings, lightweight single-config import/export, config/log/cache path openers, common provider edit forms, and a Windows NSIS packaged sidecar build. Windows packaged smoke now covers tray Show/Hide/Quit, Detach, Stop Proxy, second-launch focus, launch-at-login registration, config import/export, and provider editing. v0.18.0 does not publish the desktop installer; the public desktop release remains gated on signing keys, HTTPS release endpoints, artifact hosting, and rollback operations. See [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md) for the desktop packaging contract.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Balance adapters: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request filter: `~/.codex-helper/filter.json`
- Request log: `~/.codex-helper/logs/requests.jsonl`
- Codex relay diagnostic evidence: `~/.codex-helper/logs/codex_relay_evidence.jsonl`
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
- [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md): Tauri desktop packaging, sidecar, and release-gate notes.
- [docs/workstreams/tauri-desktop-client/REPLACEMENT_READINESS.md](docs/workstreams/tauri-desktop-client/REPLACEMENT_READINESS.md): Tauri desktop readiness, parity gaps, and follow-on split before egui removal.
- [docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md](docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md): comparison against cc-switch, aio-coding-hub, and all-api-hub.
- [docs/workstreams/codex-control-plane-refactor/README.md](docs/workstreams/codex-control-plane-refactor/README.md): control-plane design notes.

## References

codex-helper borrows good ideas from these projects while staying focused on Codex CLI local relay and control-plane workflows:

- [cc-switch](https://github.com/farion1231/cc-switch): provider UX, balance/quota templates, request usage visibility.
- [aio-coding-hub](https://github.com/dyndynjyxa/aio-coding-hub): multi-CLI gateway, request chain, cost stats, provider observability.
- [all-api-hub](https://github.com/qixing-jk/all-api-hub): Sub2API / New API balance, usage, and account adapter experience.

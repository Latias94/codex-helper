# codex-helper

A local relay proxy and operator console for Codex CLI, focused on multi-relay routing, durable request lifecycle, and observability.

Some Codex features do not appear just because `/responses` can be forwarded. Provider adapter, `/models` metadata, `/responses/compact`, WebSocket, and hosted `image_generation` are facts of the selected provider contract. Some sub2api-style and other relays also return shapes that work for normal chat but are not quite what Codex expects.

codex-helper keeps that compatibility layer local. Codex talks to the helper proxy, and the helper picks OpenAI or one of your relays through provider/routing config. It also handles model-list translation, provider-owned capability diagnostics, balance visibility, and fallback policy.

Current release: `v0.21.0`

中文说明: [README.md](README.md)

![Built-in TUI dashboard](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## Who Is It For?

Use codex-helper if:

- you use multiple Codex/OpenAI-compatible relays and do not want to keep editing `~/.codex/config.toml`;
- you want monthly relays first, then pay-as-you-go or official providers as fallback;
- you need an explicit, recoverable local proxy switch, including journaled recovery for Codex features that require an `auth.json` facade instead of a manual hack;
- your sub2api-style or other relay works for ordinary chat but is shaky around `/models`, `/responses/compact`, hosted `image_generation`, or provider-specific model names;
- you want TUI or desktop visibility into provider choice, balance/plan, tokens, cache tokens, latency, retries, and estimated cost;
- you run a local proxy for long periods and need bounded runtime state plus rotated logs;
- you want quick helpers for local Codex session discovery and resume.

It is probably unnecessary if you only use one official account and do not need provider switching or request observability.

## Main Features

- **Local proxy**: listens on `127.0.0.1:3211` by default.
- **Complete Codex client patch**: `[codex.client_patch]` and `switch on --preset ...` can declare provider identity, remote compaction, Responses WebSocket, `/models` translation, and hosted image generation. Presets that need an auth facade preserve the exact original `auth.json` in protected helper state and restore it through CAS/journaling; conflicting external edits produce `recovery_required`. Model cache and SQLite remain outside helper ownership.
- **Provider-owned capability contract**: Responses, compact, WebSocket, hosted-tool, and model decisions come from captured provider/catalog facts rather than client patch assumptions.
- **OpenAI Images-compatible entrypoint**: the local proxy also exposes `POST /v1/images/generations` and JSON `POST /v1/images/edits`, translates them into Responses hosted `image_generation` requests, and keeps using the same provider routing / fallback chain for local skills and scripts.
- **Relay capability diagnostics**: explicit, process-local CLI actions perform bounded `/models`, `/responses`, and `/responses/compact` checks and show provider contract, observations, continuity, and mismatches without changing configuration or routing.
- **Provider / routing config**: `version = 6` route graph schema. Define providers once, then use routing entry/routes for order, pinning, grouping, or tag preference.
- **Session affinity and failover**: each Codex session tries to keep using the selected provider, then falls through to other route candidates when requests fail, upstreams are unavailable, or trusted balance snapshots are exhausted.
- **Provider signal control loop**: rate limits, quota responses, transport failures, and exhausted balances are first recorded as provider signals, then converted into helper-owned temporary policy actions projected into routing. Manual disables have higher precedence, and automatic actions never mutate Codex auth or third-party account files.
- **Request-scope isolation**: conversation inference, remote compact, Responses WebSocket, and hosted-image compatibility requests are economic, route-facing traffic. `/models`, the files/uploads/batches/containers resource families, unknown endpoints, and non-POST inference-like HTTP requests may fall back only within the current request; they do not affect shared cooldown or session affinity and do not enter economic summaries.
- **Balance and plan visibility**: probes common Sub2API, New API, and `/user/balance` endpoints; lookup failures are not treated as exhausted.
- **Outbound proxy compatibility**: the local proxy and outbound network proxy are separate layers; outbound requests currently follow system/environment proxy variables, with no first-class `config.toml` proxy section yet.
- **Request observability**: provider, model, tokens, cache tokens, cache hit rate, TTFB, duration, output rate, retry chain, provider signal / policy action evidence, and estimated cost.
- **TUI and Desktop**: built-in TUI for terminal use; the old `codex-helper-gui`/egui entrypoint has been removed. The Tauri desktop source lives under `apps/desktop` and has passed Windows packaged smoke, but the current public release still does not ship a desktop installer; that release path is deferred until signing, release-channel, and rollback operations are ready.

## Quick Start

### Install

Recommended: install prebuilt binaries with the release installer scripts. Rust is not required.

macOS / Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Latias94/codex-helper/releases/latest/download/codex-helper-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Latias94/codex-helper/releases/latest/download/codex-helper-installer.ps1 | iex"
```

This installs `codex-helper` and the short alias `ch`. The old egui GUI entrypoint `codex-helper-gui` has been removed. The Tauri desktop client remains a source-tree preview and is not uploaded as a public release artifact; local validation can still build it from `apps/desktop` with `pnpm tauri:build`.

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
- load the only supported `version = 6` `~/.codex-helper/config.toml`, automatically backing up and migrating an existing v5 file first;
- on the first `ch` run, when the helper has no Codex provider or route, import only Codex's currently selected provider and persist only its base URL and credential-reference name;
- open the TUI in interactive terminals;
- stop the proxy started by the current foreground console on exit.

Both entrypoints use the same runtime but have different client-switch contracts. `codex-helper` never changes the Codex client automatically. `ch` is the compatibility entrypoint and performs journaled `switch on` only after a local Codex runtime is verified ready. First-run onboarding never persists a credential value; ambiguous authentication or an unsafe third-party origin fails closed, and existing helper providers/routes are left unchanged. A newly started foreground runtime owns an ephemeral lease and restores the helper-managed projection through CAS after `q` stops it while retaining other valid TOML edits made by Codex at runtime; `--resident` keeps the switch applied. When a matching native service is already running, `ch` authorizes attachment through the install receipt, signed runtime identity, helper/Codex homes, ports, and install generation. The owner marker is advisory: a missing or corrupt marker does not block attachment, while a present, parseable marker must agree with the service identity or attachment is rejected before switching. Exiting that TUI neither stops the service nor restores the switch. An arbitrary process occupying the same port is never adopted.

Automatic configuration migration only converts supported legacy syntax into version 6 and
preserves the exact source backup. It never copies, deletes, or reinterprets a credential value.
Moving a value into the OS credential store is a separate, explicit operation such as
`codex-helper credential import relay.primary --from-env RELAY_TOKEN`.

Start the proxy without the TUI:

```bash
codex-helper serve --no-tui
```

Advanced: run a background service or attached proxy. Only an explicitly installed service or the `--resident`/`daemon`/`tui` subcommands let the proxy outlive the current console:

```bash
codex-helper service install --codex
codex-helper service status
codex-helper daemon status
codex-helper daemon stop --codex
codex-helper tui --codex
codex-helper service stop
```

By default, the built-in `codex-helper serve` TUI follows “the console owns the proxy”: exiting the UI stops the proxy it started but never runs `switch on/off`. `daemon status` is read-only. `daemon stop` allows the same user to stop only a manually started `serve --resident` runtime through a single-use signed loopback action. It does not restore the old unauthenticated shutdown route and cannot be called remotely. A supervisor-owned, system-service-owned, or desktop-owned runtime rejects the action and directs the operator to Ctrl-C in the supervisor terminal, `service stop`, or the desktop Stop Proxy action respectively. An explicit `service stop` first restores a matching helper-managed Codex switch, while `service restart` preserves it for the same returning target. A same-identity `service install --no-start` also restores the switch before replacing a running service with a stopped installation. Missing or legacy receipts are never guessed over an existing platform registration. The `tui` subcommand attaches to an existing resident proxy. On the daemon host, a locally signed operator capability can perform the balance refresh, routing, and session actions explicitly advertised by that daemon; it falls back to read-only when local signing is unavailable. `RemoteObserver` is always read-only and never sends operator mutations. Exiting an attached TUI does not stop the proxy. For automatic restart after child crashes, run `codex-helper daemon supervise --codex`; the supervisor records lightweight crash markers under `~/.codex-helper/run/`.

`daemon status` best-effort shows the resident proxy owner marker (manual CLI, supervisor, or a future desktop/tray owner). The marker is only observability metadata: read or cleanup failures never block proxy startup or shutdown. `daemon status --json` has an explicit `schema_version` and retains both root-level operator-summary paths and the complete `operator_read_model`. A hidden managed sidecar mode is reserved for the future desktop shell, so ordinary users do not need to choose it manually.

The Tauri desktop client uses a more Clash-like resident-client lifecycle: closing the main window hides it to the tray, and `Quit App` exits only the desktop process; neither stops the runtime. Stopping the runtime remains an explicit local CLI/service operation outside the desktop query-only control plane. The Windows NSIS packaged path has passed isolated lifecycle smoke, but it is not part of the public release yet; macOS/Linux packaged parity, signing, and rollback operations still need separate follow-up work.

Switch the Codex client to helper explicitly:

```bash
codex-helper switch on
codex-helper switch on --port 4321
codex-helper switch on --base-url https://relay.example/v1
codex-helper switch on --preset imagegen-bridge
codex-helper switch on --preset official-imagegen --compaction remote-v2 --responses-websocket
codex-helper switch status
codex-helper switch off
```

NAS / remote relay targets:

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url https://nas.example.com:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN \
  --preset official-relay \
  --responses-websocket

ch relay list
ch relay status nas
ch relay nas
ch relay local --no-tui
ch relay nas --attach-only
ch relay off
```

Plain `ch` is the local compatibility entrypoint, and `ch relay local` is the explicit target form of the same automatic-switch flow. For a named Codex target, `ch relay <name>` applies a journaled local Codex switch to that target before opening its read-only TUI. `--no-tui` performs only the switch; `--attach-only` performs observation only and explicitly leaves the client unchanged. A named Codex target can override selected global client-patch fields through `relay add`; omitted fields continue to inherit `[codex.client_patch]`. `ch relay off` restores the journaled helper-owned projection while retaining other valid runtime edits. Claude targets have no Codex client-switch action and reject Codex client patches. Admin tokens are read from the environment variable named by `--admin-token-env`; token values are not written to `~/.codex-helper/config.toml`. Remote admin URLs must use HTTPS; HTTP is accepted only for loopback, including a trusted tunnel terminated on the client. A remote target must provide its trusted `--admin-url` explicitly when it is added; proxy responses and redirects cannot replace that authority.

Container and server runtimes do not provide access to a client's local transcript/session files. Local `session` commands read only the Codex session files on the machine where the command runs.

The client switch points Codex at one helper URL and applies the complete `[codex.client_patch]`. `--preset default|chatgpt-bridge|imagegen-bridge|official-relay|official-imagegen` temporarily overrides the preset; `--compaction`, `--responses-websocket[=false]`, `--translate-models[=false]`, and `--hosted-image-generation` override the other fields. Without overrides, all five fields come from `~/.codex-helper/config.toml`. A client patch controls which capabilities Codex exposes and sends; it cannot make a relay implement those protocols, so runtime decisions still use the provider/catalog contract.

`switch on` records the original Codex selector, helper stanza, relevant feature flags, and an auth facade when required. `chatgpt-bridge` accepts only a complete, verifiable existing ChatGPT login and preserves its tokens. Image-generation presets may temporarily present a semantic empty `{}` auth facade for Codex versions from the 0.20.3 era. Current Codex image/web extension gating instead comes from `requires_openai_auth = false` plus a non-empty helper actor marker; `{}` is not that gate. Original auth bytes and any sensitive headers or comments in an inactive `codex_proxy` stanza never enter the JSON journal: each lives in a private helper-state backup, while the journal stores only random filenames and fingerprints. `switch off` restores helper-owned selector/stanza/feature keys in the current valid TOML while retaining unrelated provider, feature, project, and other runtime edits; `auth.json` is restored byte-for-byte through no-replace CAS. After an auth-bearing patch is off, status remains `Off` while the private backup/journal is retained, so another `switch off` can repair a semantically equivalent facade that Codex writes later; the next `switch on` safely adopts that recovery point. Managed-field conflicts, unattributable auth edits, or a missing/mismatched backup after the original projection was replaced produce `recovery_required` without replacing a competing file. The helper capability marker is consumed locally and never reaches an upstream, while real actor authorization can pass only to an official OpenAI origin without configured helper credentials.

Client patching never reads or changes `models_cache.json` or Codex SQLite and does not restore the retired `remote-control` SQL hack. It manages only the recorded `config.toml` sections and optional `auth.json` facade during an explicit `switch on/off` lifecycle.

When upgrading from 0.20.3 or earlier, a current `switch off` safely and automatically restores the selector/provider stanza and any verifiable auth facade managed by a remaining `~/.codex/codex-helper-switch-state.json`; `switch on` performs the same recovery before creating its new journal. Recovery writes only while the current files still match the old helper patch. Malformed or unknown state and legacy/current journal conflicts preserve the original state and fail closed. The legacy file may contain original auth content, so do not delete, edit, or share it. The new release does not undo `remote_connections` or Codex SQLite state written by the removed `switch remote-control enable`, and that database must not be cleaned with an SQL hack. See [Configuration Compatibility](docs/CONFIGURATION.md#configuration-compatibility) for the full sequence, v5-to-v6 migration, and retired fields.

Relay capabilities come from the selected provider adapter, catalog, and bounded observations rather than switch configuration. Inspect the provider contract, live `/models` / `/responses` / `/responses/compact` results, continuity, and mismatches with:

```bash
codex-helper codex relay-capabilities --model gpt-5.5 --provider ciii --endpoint default
```

Third-party relays must configure helper-owned authentication explicitly. Version 6 can bind bearer or `X-API-Key` auth to a native credential, an absolute read-only secret file, an environment variable, or a compatibility inline value; selecting `auth_token_ref` / `api_key_ref` disables fallback to legacy sources of the same kind. Installed desktop services should use the logged-in user's Credential Manager, Keychain, or Secret Service. Docker/headless servers use environment variables or mounted secrets; the server rejects native references and never creates a plaintext or SQLite fallback. See [Provider Fields](docs/CONFIGURATION.md#provider-fields) for commands, precedence, and readiness semantics. Codex client authentication may pass only to the official OpenAI origin, preventing account headers from leaking to a relay.

To avoid degrading capable relays, codex-helper normalizes compressed HTTP request bodies before routing by default (`zstd`, `gzip` / `x-gzip`, `br`, and `deflate`). For Codex `/responses`, `/responses/compact`, and Responses WebSocket, helper also completes missing `session_id`, `x-session-id`, official `session-id` / `thread-id`, and `prompt_cache_key` fields from existing request evidence: header session ids, body `session_id`, `prompt_cache_key`, or `metadata.session_id`. `previous_response_id` is only used for stale-response repair, not as a session identity source. It does not invent a synthetic session id and does not overwrite session fields the client already sent.

Selected provider endpoint affinity is persisted under helper state so a helper restart does not silently move a Codex remote-compaction session to a different provider endpoint. State-bound compact requests include v1 compact bodies carrying `encrypted_content`, `previous_response_id`, or `compaction_summary` and remote compaction v2 requests carrying `compaction_trigger`. On a multi-endpoint graph, `hard` affinity fails missing provable affinity with an explicit continuity error; the default `fallback-sticky` policy instead tries the current route graph, leaves state validity to the upstream, and records the successful endpoint as new affinity. Helper keeps this provider-opaque: it does not infer whether a relay is OpenAI, sub2api, New API, or another intermediary. This keeps relay stickiness through `/responses`, `/responses/compact`, and v2 compact's `/responses` request shape; it does not add compact or WebSocket support to relays that lack those endpoints. For rare relays that require the original compressed Codex body, run helper with `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough`.

Codex request semantics also include two targeted repairs: if an upstream explicitly says a `previous_response_id` response no longer exists, helper removes that field and retries the same upstream once; if a relay ignores `Accept-Encoding: identity` and returns gzip JSON, helper decodes it before forwarding plain JSON. `service_tier` remains observational and attribution-only: logs distinguish requested / effective / actual values, but helper default config does not rewrite the client's fast-mode request tier.

Hosted image generation, remote compaction, and Responses WebSocket all require real upstream protocol support. Live smoke requires explicit acknowledgement and is diagnostic only; it never enables a client feature or changes routing.

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

To send model traffic through a relay, point the client at helper and let helper select the upstream:

1. Run `codex-helper switch on` to point Codex's `~/.codex/config.toml` at local `codex_proxy`.
2. Configure `codex.providers.*` and `codex.routing` in `~/.codex-helper/config.toml`.
3. Add provider-scoped `model_mapping` if the relay expects prefixed model names.

This path does not proxy Codex login. Only a client patch that requires an auth facade temporarily changes the `auth.json` client view during the explicit switch lifecycle and restores the original bytes on switch-off; Codex remains responsible for creating and maintaining the login credentials.

The Codex-side local proxy entry is normally written by `switch on`; avoid hand-editing it over unrelated Codex settings:

```toml
# ~/.codex/config.toml
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
```

The codex-helper side only owns upstreams and routing:

```toml
# ~/.codex-helper/config.toml
version = 6

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"

[codex.routing]
entry = "relay_first"

[codex.routing.routes.relay_first]
strategy = "ordered-failover"
children = ["relay"]
```

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
version = 6

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

For complete config, compatibility behavior, balance adapters, pricing, and query-only TUI/desktop operator views, see [docs/CONFIGURATION.md](docs/CONFIGURATION.md). The equivalent Chinese reference is [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md).

## Proxy Notes

codex-helper has two proxy layers:

- **Local proxy**: Codex connects to `127.0.0.1:3211`, then codex-helper chooses a provider through routing. After an explicit `switch on` points Codex at helper, requests still pass through this local proxy server even if you do not configure an outbound network proxy.
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
codex-helper session search "rate limit"
codex-helper session search "rate limit" --truncate 120
codex-helper session recent
codex-helper session last
codex-helper session transcript <SESSION_ID> --tail 40

# request logs and usage
codex-helper usage quota --target local
codex-helper usage quota --target local --json
codex-helper usage summary
codex-helper usage tail --limit 20
codex-helper usage find --errors --limit 10
codex-helper usage chain --trace-id <TRACE_ID> --json

# pricing
codex-helper pricing list
codex-helper pricing status
codex-helper pricing force-refresh
codex-helper pricing import-basellm --model gpt-5 --dry-run

# diagnostics
codex-helper status
codex-helper doctor
codex-helper codex relay-capabilities --model gpt-5.5 --provider ciii --endpoint default
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
- `Routing`: provider/endpoint order, configured/effective/routable state, automatic controls, capacity, and compact balance/quota context. Use `routing show` / `routing explain` for the full route graph and candidate paths.
- `Sessions`: session identity, effective route, route affinity, and per-session overrides.
- `Usage`: remote shared quota-pool used/remaining, 15/60-minute rates, required rate until reset, pace, ETA, plus local-day requests, tokens, estimated cost, and project attribution.
- `Requests`: committed request/attempt facts, recent endpoint samples, tokens, cache tokens, latency, retries, request chains, and cost.

TUI and desktop consume the same typed, redacted `OperatorReadModel` and use only `GET` / `HEAD` against a remote runtime control plane. The model distinguishes `ready`, `stale`, `disconnected`, and `auth_required`; connection or authentication failures never synthesize a fallback view from local config, SQLite, or an empty runtime. Remote operator clients and `RemoteObserver` are read-only and never inspect or change Codex configuration on the observer machine. A daemon-host-local `LocalAttached` console may use signed local operator capabilities and may use `n` / `o` or preset shortcuts in Settings to change Codex client files on that same machine; those local file operations are not remote control-plane mutations. Edit durable provider/routing intent with local CLI commands or `config.toml`. Terminal client-switch paths include explicit `switch on/off`, `n` / `o` in integrated or LocalAttached TUI Settings, and the local `ch` / `ch relay` compatibility flows backed by the same journal/CAS contract.

The target daemon exclusively owns its remote quota sampler, so attached clients never start a second sampler. A remote observer cannot force refreshes or mutations, while an authenticated local loopback-attached TUI may delegate a refresh to the daemon. A remote pool counter may include other computers using the same account or key and is the source of truth for shared total burn; project attribution comes from request-ledger facts committed by that daemon to `state.sqlite` and never scales local prices to match a remote delta. See [Usage Page](docs/CONFIGURATION.md#usage-page) for source/scope/confidence, coverage, raw-unit, and conversion-generation limits.

### Desktop Preview

The new Tauri desktop client lives under `apps/desktop` and uses React 19, Tailwind CSS 4, shadcn/ui-style components, and TanStack Router/Query/Table. It renders the typed, redacted `OperatorReadModel` and keeps local proxy lifecycle, explicit Codex switch, close-to-tray semantics, single instance, and launch-at-login settings; it does not import config, edit providers, or mutate provider/routing/config through the remote control plane. The Windows NSIS packaged sidecar has passed isolated smoke, but the public release still does not ship the desktop installer; signing keys, HTTPS release endpoints, artifact hosting, and rollback operations remain release gates. See [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md) for the packaging contract.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Runtime state: `~/.codex-helper/state/state.sqlite`
- Balance adapters (optional and operator-owned; missing files use in-memory built-ins, and invalid input is never overwritten): `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Request filter: `~/.codex-helper/filter.json`
- Post-commit request log: `~/.codex-helper/logs/requests.jsonl`
- Optional full HTTP debug log: `~/.codex-helper/logs/requests_debug.jsonl`
- Codex relay diagnostic evidence: `~/.codex-helper/logs/codex_relay_evidence.jsonl`

Full HTTP request/response diagnostics are disabled by default. Read the [configuration reference for the environment variables and security boundary](docs/CONFIGURATION.md#full-http-request-and-response-diagnostics) before enabling them. Authentication headers and URI queries are sanitized, but explicitly captured request and response bodies do not receive field-level redaction.

Codex files remain authoritative to Codex:

- `~/.codex/auth.json`
- `~/.codex/config.toml`

An explicit local `switch on/off` action manages the recorded client-patch sections in `~/.codex/config.toml` and may temporarily manage an `auth.json` facade when required. Private backup plus CAS restores the original auth exactly. Codex model cache and SQLite remain untouched.

## Design Boundaries

codex-helper intentionally avoids:

- one full Codex config per provider;
- guessing billing class from provider names;
- pretending speed-first or cost-first routing is reliable before real measurements exist;
- treating a balance lookup failure as provider exhaustion;
- letting UI saves silently drop advanced provider fields.

## More Docs

- [docs/CONFIGURATION.md](docs/CONFIGURATION.md): English configuration reference covering routing, balance adapters, pricing, configuration compatibility, and query-only operator views.
- [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md): Chinese configuration reference with routing recipes, balance adapters, proxy notes, configuration compatibility, and query-only operator views.
- [CHANGELOG.md](CHANGELOG.md): release notes and upgrade notes.
- [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md): Tauri desktop packaging, sidecar, and release-gate notes.
- [docs/workstreams/codex-routing-scheduler-observability-refactor/README.md](docs/workstreams/codex-routing-scheduler-observability-refactor/README.md): fearless refactor design for routing scheduler state, throttle/overload outcomes, concurrency limits, and TUI metrics.
- [docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md](docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md): comparison against cc-switch, aio-coding-hub, and all-api-hub.
- [docs/workstreams/codex-control-plane-refactor/README.md](docs/workstreams/codex-control-plane-refactor/README.md): control-plane design notes.

## References

codex-helper borrows good ideas from these projects while staying focused on Codex CLI local relay and control-plane workflows:

- [cc-switch](https://github.com/farion1231/cc-switch): provider UX, balance/quota templates, request usage visibility.
- [aio-coding-hub](https://github.com/dyndynjyxa/aio-coding-hub): multi-CLI gateway, request chain, cost stats, provider observability.
- [all-api-hub](https://github.com/qixing-jk/all-api-hub): Sub2API / New API balance, usage, and account adapter experience.

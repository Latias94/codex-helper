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
- Codex relay diagnostic evidence: `~/.codex-helper/logs/codex_relay_evidence.jsonl`

Codex-owned files remain owned by Codex:

- `~/.codex/auth.json`
- `~/.codex/config.toml`

`switch on/off` and one-command startup only patch the local Codex proxy section. They do not overwrite unrelated Codex config changes.

## Relay Targets

Relay targets are client-side bookmarks for local or remote codex-helper runtimes. They live in `~/.codex-helper/config.toml` and are used by `ch relay ...`; provider/routing config still belongs to the server runtime that receives traffic.

```toml
[relay_targets.nas]
service = "codex"
proxy_url = "http://nas.local:3211"
admin_url = "http://nas.local:4211"
admin_token_env = "CODEX_HELPER_NAS_ADMIN_TOKEN"
client_preset = "official-relay"
responses_websocket = false
```

Equivalent CLI:

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url http://nas.local:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN \
  --preset official-relay
```

`local` is built in and resolves to the normal loopback ports for the current `default_service`, so `ch relay local` preserves the normal local foreground flow. Named targets are remote by default: `ch relay nas` patches this machine's Codex config to the target proxy and opens an attached TUI against the target admin API. `--no-tui` switches only; `--attach-only` observes only.

`admin_token_env` stores the environment variable name, not the token value. For Docker/NAS targets, prefer setting `advertised-admin-base-url` on the server so `relay add` can discover a reachable admin URL; otherwise pass `--admin-url` explicitly.

## Codex Client Preset

The default preset only points `~/.codex/config.toml` `model_provider` at the local `codex_proxy`. To keep ChatGPT account auth and mobile/desktop account features while routing model requests through codex-helper, enable ChatGPT bridge:

```toml
version = 5

[codex.client_patch]
preset = "chatgpt-bridge"
# Optional transport switch. Only valid with official relay presets.
responses_websocket = false
# Optional compaction strategy: auto | local | remote-v1 | remote-v2.
compaction = "auto"
```

Legacy `mode = "..."` config is still accepted for existing users, but codex-helper rewrites saved/generated config as `preset = "..."`.

You can also switch it temporarily from the CLI:

```bash
codex-helper switch on --preset chatgpt-bridge
codex-helper switch on --preset imagegen-bridge
codex-helper switch on --preset official-relay
codex-helper switch on --preset official-relay --responses-websocket
codex-helper switch on --preset official-imagegen --compaction local
codex-helper switch on --preset official-imagegen
codex-helper switch on --preset default
```

The legacy CLI spelling `--mode ...` is also accepted as an alias. On startup, `codex-helper serve` uses `[codex.client_patch]` when Codex is not already switched to codex-helper. If Codex is already switched, the existing client preset is preserved; use `switch on --preset ...` or the TUI Settings `B`/`I`/`F`/`D` keys to change it explicitly.

By default, the console owns the proxy lifecycle: `codex-helper serve` stops its proxy and restores the local client patch when the built-in TUI exits, and the GUI stops any proxy it started when the GUI exits. For long-running local proxy use, start `codex-helper serve --resident`. Resident mode keeps the client patch active when the console exits, exposes `/__codex_helper/api/v1/runtime/shutdown`, and can be inspected with `codex-helper daemon status` or stopped with `codex-helper daemon stop`. Use `codex-helper tui --codex` or `codex-helper tui --claude` to attach a read-only terminal dashboard to an existing resident proxy; quitting that dashboard exits only the console. The GUI can also attach explicitly from its setup/overview pages, but it no longer silently adopts the helper port from Codex/Claude on startup. If you want a foreground watchdog, `codex-helper daemon supervise --codex` starts a resident child, restarts it with bounded backoff after crashes, and records crash markers in `~/.codex-helper/run/`.

Resident runtimes write a best-effort owner marker under `~/.codex-helper/run/` so `daemon status` can distinguish manual CLI, supervisor, and future desktop/tray-owned sidecars. These marker files are advisory metadata only: stale, corrupt, or missing markers should not stop a proxy from starting, exiting, or being stopped explicitly. The desktop-managed sidecar mode is intentionally hidden until a visible desktop/tray shell exists; normal `serve` and GUI startup remain non-resident by default.

`chatgpt-bridge` writes `requires_openai_auth = true` and `supports_websockets = false` into `~/.codex/config.toml`, and changes only two `~/.codex/auth.json` fields: `auth_mode` becomes `"chatgpt"` and `OPENAI_API_KEY` becomes `null`. It requires an existing official Codex ChatGPT login state; if `auth.json` has no complete token/email/account metadata, codex-helper refuses the patch before writing `config.toml` or `auth.json`. Existing Codex apps usually need a restart before they read the changed client config.

`imagegen-bridge` is an explicit experimental hack preset. It writes an empty `{}` `~/.codex/auth.json` facade so Codex's default auth resolution still treats the session as ChatGPT-backed and exposes the hosted `image_generation` tool, while actual upstream credentials still come from codex-helper routing (`auth_token_env`, `auth_token`, `api_key_env`, or `api_key`). It does not require an official ChatGPT login and does not write an explicit `auth_mode`. Before enabling it, codex-helper verifies that the Codex service has at least one enabled upstream and that at least one upstream credential is actually available to the current process. For env-based credentials, setting only the env var name in config is not enough; the env var value must also be present when you run `switch on` or start `serve`. codex-helper stores the previous `auth.json` in its switch state and restores it when switching back to `default` or running `switch off`, but only if the current `auth.json` still matches the helper-written facade. If the user or Codex changed `auth.json` meanwhile, codex-helper leaves it untouched.

`official-relay` is an experimental official-relay preset for relays that forward OpenAI Responses semantics, especially sub2api-style relays that support `/responses/compact`. It writes `name = "OpenAI"` into `~/.codex/config.toml` so Codex chooses the remote compaction path by default. It keeps `supports_websockets = false` unless the separate WebSocket switch is enabled. It does not write `requires_openai_auth` and does not patch `auth.json`; upstream credentials still must come from codex-helper routing. If the relay rejects `/responses/compact` with 404/405/501 or an unsupported-compact error, explicitly set `compaction = "local"` to make the Codex client return to local compaction, or use a relay account that advertises compact support.

For all presets, codex-helper normalizes HTTP request `Content-Encoding` by default before it inspects or forwards a request. Supported request encodings are `zstd`, `gzip` / `x-gzip`, `br`, and `deflate`; after a successful decode, helper forwards ordinary JSON and removes stale `Content-Encoding` / `Content-Length`. This is a transport compatibility layer, not a compact fallback: the upstream relay must still implement `/responses/compact`, hosted tools, or WebSocket support itself. If you hit a rare relay that requires the exact compressed Codex request body, start helper with `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` to preserve the original body and header.

When Codex does not send stronger session headers (`session_id`, `session-id`, `conversation_id`, or `thread-id`), codex-helper also uses decoded JSON `prompt_cache_key` as the session-affinity key. This mirrors sub2api-style stickiness so normal `/responses` traffic and later `/responses/compact` requests stay on the same selected route without asking users to classify the relay implementation.

`official-imagegen` is the hybrid experimental preset for relays backed by official OpenAI subscriptions. It writes the same OpenAI provider identity as `official-relay` so Codex uses the remote compaction path by default, and writes the same empty `{}` auth facade as `imagegen-bridge` so Codex exposes hosted `image_generation`. By default it keeps `supports_websockets = false`, does not write `requires_openai_auth`, and still strips Codex client auth before forwarding unless the selected upstream has its own helper-side credential. This preset only makes Codex expose and send the official hosted tool; the relay account still has to support both `/responses/compact` and hosted image generation calls.

`compaction` is a separate compaction strategy, not another preset. `auto` keeps the preset default: `default` / `imagegen-bridge` lean toward Codex local compaction, while `official-relay` / `official-imagegen` use the remote compaction path by default. `local` forces the provider identity back to `codex-helper` so the Codex client performs local compaction; `remote-v1` forces OpenAI provider identity and disables `remote_compaction_v2`, making Codex use `/responses/compact`; `remote-v2` writes `[features].remote_compaction_v2 = true`, while helper still uses `[codex.compaction].remote_v2_downgrade = true` to fall back to v1 when the upstream cannot produce a valid v2 stream.

`responses_websocket = true` is a transport switch, not a separate preset. It is only valid with `official-relay` and `official-imagegen`. When enabled, codex-helper writes `supports_websockets = true` into Codex's provider config and handles the WebSocket upgrade itself on `/responses`, `/v1/responses`, and `/backend-api/codex/responses`. The relay path reads the first `response.create` frame, applies the same model override, model mapping, request filter, routing selection, session affinity, concurrency snapshot, and auth injection as normal helper traffic, injects `OpenAI-Beta: responses_websockets=2026-02-06`, then bridges frames bidirectionally to the selected upstream. Keep it disabled unless your upstream relay also supports Responses WebSocket v2.

Assuming the relay supports the required endpoints, the capability ladder is:

```text
default
< chatgpt-bridge / imagegen-bridge
< official-relay
< official-imagegen
< official-imagegen + responses_websocket
```

`official-imagegen` is the most complete preset, but it is also the most demanding: the relay must support `/responses`, `/responses/compact`, and hosted `image_generation`. Only enable `responses_websocket` after a WebSocket live smoke passes for the selected upstream.

## OpenAI Images-Compatible Endpoints

The proxy also exposes OpenAI Images-style endpoints for local skills or scripts:

- `POST /v1/images/generations` and `/images/generations` for text-to-image generation.
- JSON `POST /v1/images/edits` and `/images/edits` for reference-image generation.

codex-helper translates these requests into a non-streaming `/v1/responses` call with a hosted
`image_generation` tool, then converts a successful `image_generation_call.result` back into
`data[0].b64_json`.

Example:

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

This endpoint intentionally reuses normal provider routing, model mapping, retry/fallback, auth
injection, and request logging. The selected upstream must still support hosted Responses image
generation.

Reference-image edits accept JSON with an `images` array. Each item can be an object with
`image_url` or `file_id`, or a direct image URL / data URL string. codex-helper turns these into
Responses `input_image` content:

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

Both generation and JSON edits support one generated result (`n` absent or `1`). JSON edits do not
parse masks; JSON requests with `mask` and multipart edits pass through as ordinary proxy requests.

You can actively inspect a relay's Codex capability profile through the local admin API:

In the built-in TUI, open Settings (`6`) and press `C` to run the same bounded relay diagnostic
against the current Codex runtime. The Settings page shows the selected target, expected
capabilities, observed `/models` / `/responses` / `/responses/compact` support, mismatches,
warnings, and the recommended preset. The TUI action is diagnostic-only; it never changes the
preset automatically.

```bash
curl -s http://127.0.0.1:4211/__codex_helper/api/v1/codex/relay-capabilities \
  -H 'content-type: application/json' \
  -d '{"patch_preset":"official-imagegen","compaction":"local","model":"gpt-5.5"}'
```

For API compatibility the response JSON field is still named `patch_mode`; requests accept either `patch_mode` or `patch_preset`, and accept both preset names such as `official-imagegen` and legacy mode names such as `official-imagegen-bridge`. Requests and responses also include `compaction` so diagnostics evaluate the same `auto` / `local` / `remote-v1` / `remote-v2` strategy that `switch on` would apply.

Use the admin port for your Codex proxy port (`proxy_port + 1000`; the default Codex proxy is
`3211`, so the default admin port is `4211`). The endpoint is `POST` on purpose: it sends one
bounded active probe to the selected upstream's `/models`, `/responses`, and `/responses/compact`
endpoints. `/models` is read-only; the two Responses probes send `{}` and classify validation
errors as endpoint support. The endpoint does not use normal routing, retry, request ledger,
session affinity, passive health, or runtime health state, so it is a diagnostic action rather than
a request storm amplifier.

The response includes:

- `expected`: what Codex should expose for the requested preset and model metadata.
- `compaction`: the compaction strategy used when computing the expected Codex client profile.
- `observed`: what the relay actually returned for `/models`, `/responses`, and
  `/responses/compact`, including confidence and whether helper translation is required.
- `mismatches`: places where Codex will try a capability that the relay did not prove.
- `recommendation`: the conservative preset recommendation for the observed relay.
- `continuity`: the selected provider endpoint's state-continuity domain, whether that domain was
  explicit, and warnings for official relay presets that may carry encrypted compact state.

Recommendation rules are intentionally conservative:

| Observed relay state | Recommended preset |
| --- | --- |
| `/responses` works, `/responses/compact` works, selected model is image-capable | `official-imagegen` |
| `/responses` works, `/responses/compact` works, selected model is not image-capable | `official-relay` |
| `/responses` works, `/responses/compact` is unsupported, selected model is image-capable | `imagegen-bridge` |
| `/responses` works, `/responses/compact` is unsupported, no image capability is proven | `default` |
| `/responses/compact` is unknown | avoid official relay presets until compact is proven |
| `/responses` is unavailable | `default`; no preset can compensate for a missing Responses endpoint |

For sub2api-style relays, a raw OpenAI `/models` response (`data: [...]`) is fine only if
codex-helper translates it into the Codex `models: [...]` catalog before Codex sees it. The
diagnostic response reports this as `observed.models.translation_required = true`. For non-sub2api
relays, the same rules apply: the relay can either return Codex-shaped model metadata directly or
return an OpenAI model list that codex-helper can translate. If the selected model is absent or its
metadata does not prove image input, the recommendation will not assume hosted image generation.

Hosted `image_generation` is not actively probed by this diagnostic endpoint because that can spend
quota or create image artifacts. Responses WebSocket support is opt-in through
`responses_websocket = true` / `--responses-websocket`; bridge presets keep it disabled by default.
Remote compaction v2 is not enabled by default. If you set `compaction = "remote-v2"` or enable Codex
`[features].remote_compaction_v2 = true` yourself, helper recognizes the
`compaction_trigger` request shape for logging and route-continuity protection,
but the upstream relay must still support v2 compaction response items or rely on helper's v2-to-v1 downgrade fallback.

Official relay presets deliberately separate two ideas:

- `name = "OpenAI"` tells Codex to use the official Responses protocol surface, including
  `/responses/compact` for remote compaction v1.
- It does not prove that two helper provider endpoints share upstream encrypted response state.

By default, each provider endpoint is its own continuity domain. For relay chains such as sub2api,
New API, or another OpenAI-compatible gateway, do not use host name, base URL, provider brand, or
same-domain routing as proof that encrypted compact state can move across endpoints. If two
configured endpoints intentionally front the same upstream account/state store, set the same
`continuity_domain` on those providers or endpoints:

```toml
[codex.providers.relay_hk]
base_url = "https://hk.relay.example/v1"
auth_token_env = "RELAY_HK_KEY"
continuity_domain = "relay-cluster-a"

[codex.providers.relay_us]
base_url = "https://us.relay.example/v1"
auth_token_env = "RELAY_US_KEY"
continuity_domain = "relay-cluster-a"
```

Only endpoints with the same explicit `continuity_domain` are allowed to fail over for
provider-state-bound compact after a known affinity exists. Leave the field unset when each endpoint
represents a different relay account, different upstream OpenAI account, or an opaque reseller.
Direct `https://api.openai.com/v1` setups with a single authenticated account usually do not need
this field because provider-endpoint affinity is already the domain boundary.

When validation-only diagnostics are inconclusive, you can run a stronger live smoke check. This is
a real upstream request, not a background health check. It is manual, cost-bearing, and requires the
literal acknowledgement string before codex-helper sends any upstream traffic:

```bash
curl -s http://127.0.0.1:4211/__codex_helper/api/v1/codex/relay-live-smoke \
  -H 'content-type: application/json' \
  -d '{
    "acknowledgement": "run-live-codex-relay-smoke",
    "model": "gpt-5.5"
  }'
```

With no `cases` field, live smoke only checks remote compaction v1 through `/responses/compact`.
Remote compaction v2, hosted image generation, and Responses WebSocket are never part of the default
case set. To explicitly test Codex remote compaction v2 compatibility for the selected
relay/provider chain, include `remote_compaction_v2`. The smoke sends `POST /responses` with
`stream: true`, one `compaction_trigger` input item, and `x-codex-beta-features:
remote_compaction_v2`; it passes only when the stream contains exactly one compaction output item
and `response.completed`:

```bash
curl -s http://127.0.0.1:4211/__codex_helper/api/v1/codex/relay-live-smoke \
  -H 'content-type: application/json' \
  -d '{
    "acknowledgement": "run-live-codex-relay-smoke",
    "model": "gpt-5.5",
    "provider_id": "ciii",
    "endpoint_id": "default",
    "cases": ["remote_compaction_v2"]
  }'
```

To explicitly test the hosted tool request path:

```bash
curl -s http://127.0.0.1:4211/__codex_helper/api/v1/codex/relay-live-smoke \
  -H 'content-type: application/json' \
  -d '{
    "acknowledgement": "run-live-codex-relay-smoke",
    "model": "gpt-5.5",
    "cases": ["responses_compact", "hosted_image_generation"]
  }'
```

To explicitly test the selected upstream's Responses WebSocket v2 path, include
`responses_websocket`. The smoke opens `GET /responses` as a WebSocket, injects
`OpenAI-Beta: responses_websockets=2026-02-06`, sends one minimal `response.create` frame, and
passes when the relay returns a `response.*` event or a Codex WebSocket protocol event such as
`codex.rate_limits`:

```bash
curl -s http://127.0.0.1:4211/__codex_helper/api/v1/codex/relay-live-smoke \
  -H 'content-type: application/json' \
  -d '{
    "acknowledgement": "run-live-codex-relay-smoke",
    "model": "gpt-5.5",
    "provider_id": "ciii",
    "endpoint_id": "default",
    "cases": ["responses_websocket"]
  }'
```

In the TUI Settings page, press `X` twice within the confirmation window for compact-only live
smoke, or `Y` twice for compact plus hosted image-generation live smoke. Both actions use the
currently selected Codex runtime target and inferred model unless the API request supplies explicit
fields.

The same diagnostics are available without starting the TUI or admin listener:

```bash
codex-helper codex relay-capabilities \
  --preset official-imagegen \
  --compaction local \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default

codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5

codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --compact-v2

codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --image

codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --websocket

codex-helper codex relay-evidence --limit 20
```

For the CLI, omitting optional case flags runs the default compact smoke. Supplying `--compact-v2`,
`--image`, `--websocket`, or any combination runs only those explicit optional cases, so an optional smoke does not
accidentally spend an additional compact request.

Targeting uses the normal selected runtime target by default. For route-graph configs, diagnostics
can target a provider endpoint directly with `provider_id` / `endpoint_id` in the API body or
`--provider` / `--endpoint` in the CLI. Legacy `--station` / `--upstream-index` is still available
for station-shaped configs, but provider targeting cannot be combined with station targeting.

Live smoke is intentionally isolated from normal routing behavior. It selects one upstream, sends at
most one request/connection per selected case, bypasses route retry/failover, and does not write
request ledger entries, route affinity, passive health, runtime health, balance state, or
patch-preset changes. Image responses are summarized only: codex-helper reports whether an
`image_generation_call` appeared, but does not store raw image bytes or base64 payloads.

Capability diagnostics and live smoke append sanitized summaries to
`~/.codex-helper/logs/codex_relay_evidence.jsonl`. This evidence store is local operator memory,
not routing truth. It does not feed request ledger summaries, load balancing, session affinity,
passive health, balance exhaustion, retry policy, or automatic patch-preset changes. Use
`codex-helper codex relay-evidence --json` when you want machine-readable records for bug reports or
relay comparisons.

To diagnose whether remote compaction v1 is active, inspect the codex-helper request ledger after a Codex compaction happens:

```bash
codex-helper usage find --path responses/compact --limit 20
codex-helper usage find --path responses --limit 20
```

An official compact hit normally appears as `POST /responses/compact` in codex-helper logs. Ordinary local fallback compaction appears as a normal `POST /responses` request. Remote compaction v2, when Codex enables it, also travels through ordinary `/responses` with a structured `compaction_trigger` input item rather than `/responses/compact`; helper logs it with `codex_bridge.remote_compaction_v2_request = true` and applies state-bound route-continuity rules. `compaction = "remote-v2"` explicitly enables v2; default `auto` does not write that feature flag. When `responses_websocket` is enabled, normal turn streaming uses a WebSocket `GET /responses`-style upgrade rather than an HTTP `POST /responses`.

Switching back to `default` removes the bridge-only fields from `codex_proxy` and restores helper-managed auth patches when it is safe to do so.

Safety rule: in bridge presets, upstream providers should configure their own `auth_token_env` / `auth_token` or API key equivalent. If an upstream has no helper-side secret, codex-helper strips Codex client auth headers to avoid forwarding ChatGPT/facade auth to third-party relays.

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
affinity_policy = "fallback-sticky"
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

Route graph session affinity is runtime state with a small durable ledger for Codex route continuity. The TOML config chooses the affinity policy and can optionally bound fallback stickiness:

- `fallback-sticky` is the default used by the generated config template and Codex bootstrap import. It keeps a session on the last successful fallback provider while that provider remains viable, which is safer for official relay features such as remote compaction that may carry upstream-account-bound encrypted state. Set `fallback_ttl_ms` to cap how long a lower-priority fallback affinity can be reused, or `reprobe_preferred_after_ms` to force a preferred-group reprobe after a fallback target change.
- `preferred-group` applies session affinity only inside the currently best available preference group, so a session that temporarily falls back to paygo returns to monthly as soon as a monthly provider is viable again.
- `off` ignores automatic route affinity.
- `hard` treats an existing affinity target as strict for that route graph; if the target is unavailable, no alternate candidate is selected.

For each request with a session id, codex-helper keys affinity by `session_id + service + route_graph_key`. While the route graph is unchanged, the same session can keep using the previously selected provider/endpoint according to the policy. This improves upstream prompt-cache locality for relay providers that cache by account or upstream target without letting automatic stickiness override user preference by default.

Successful route affinity is also persisted to:

```text
~/.codex-helper/state/session-route-affinities.json
```

The ledger stores helper-owned provider endpoint identity only; it does not store or infer upstream relay implementation details. Set `CODEX_HELPER_SESSION_ROUTE_AFFINITY_LEDGER=off` to disable this persistence, or set it to a path to use a custom ledger file.

For Codex remote compaction, helper treats compact v1 requests that mention state-bound fields such as `encrypted_content`, `previous_response_id`, or `compaction_summary`, and compact v2 requests with a structured `compaction_trigger`, as provider-state-bound. Under the default `fallback-sticky` route affinity policy, a state-bound compact request without existing route affinity is still tryable: helper follows the configured route graph, records the successful provider endpoint as the session affinity, and lets upstream decide whether the compact state is valid. Under `hard` affinity, or on the legacy multi-upstream path, missing affinity remains fail-closed with an explicit continuity error. If a known affinity endpoint itself fails, `fallback-sticky` may continue along the route graph and update affinity, while `hard` blocks cross-endpoint movement unless an explicit shared `continuity_domain` permits it. Non-state-bound compact can still use normal provider fallback according to the route policy.

Affinity is not a hard pin:

- request retry, provider health, capability mismatch, cooldown, and trusted balance exhaustion still apply;
- if the sticky provider fails, ordinary and non-state-bound requests continue through the current route graph and then stick to the next successful provider;
- provider-state-bound compact honors the route affinity policy: `fallback-sticky` stays tryable and updates affinity after a successful fallback, while `hard` stays within the affinity continuity domain unless an explicit shared `continuity_domain` permits movement;
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

### Provider Concurrency Limits

Use `limits.max_concurrent_requests` when an upstream relay account only allows a small number of simultaneous requests. This is a local-process cap: one running codex-helper process tracks active requests and skips saturated candidates during routing. It is not a distributed quota across several codex-helper processes.

```toml
[codex.providers.relay.limits]
max_concurrent_requests = 5
limit_group = "relay-account"
```

`limit_group` is optional. Without it, the cap is scoped to that provider endpoint. Use the same `limit_group` on several provider endpoints when they share one upstream account quota. Endpoint-level `limits` override provider-level `limits`:

```toml
[codex.providers.relay]
alias = "Relay account"
auth_token_env = "RELAY_API_KEY"

[codex.providers.relay.limits]
max_concurrent_requests = 5
limit_group = "relay-account"

[codex.providers.relay.endpoints.hk]
base_url = "https://hk.relay.example/v1"

[codex.providers.relay.endpoints.us]
base_url = "https://us.relay.example/v1"

[codex.providers.relay.endpoints.us.limits]
max_concurrent_requests = 2
limit_group = "relay-us"
```

When a candidate is saturated, routing treats it as temporarily unavailable and continues to the next fallback. Saturation does not count as a provider failure, does not open cooldown, and does not poison session affinity. `routing explain` reports `concurrency_saturated` with the active count and limit.

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

For authentication, first decide which HTTP header the provider expects:

- **OpenAI and most OpenAI-compatible relays** use bearer auth: `Authorization: Bearer <key>`.
  Configure `auth_token_env` for normal use, or `auth_token` only for local scratch configs.
  This is true even when the provider's dashboard calls the secret an "API key".
- Use `api_key_env` / `api_key` only when the provider explicitly documents an
  `X-API-Key` header.
- Prefer the `*_env` fields so secrets stay out of `~/.codex-helper/config.toml`.
  The value in config is the environment variable name, not the secret itself; the variable must
  be set in the process that runs codex-helper.
- If an inline value and an env reference are both configured for the same header family, the
  inline value wins. If both bearer and `X-API-Key` credentials are configured, codex-helper sends
  both headers; avoid that unless the relay explicitly requires it.

Use `model_mapping` when the model requested by Codex differs from the model name expected by a specific relay. The mapping is provider-scoped: codex-helper rewrites the request body `model` only after that provider is selected, so other providers are not affected.

```toml
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
supported_models = { "gpt-5.5" = true }
model_mapping = { "gpt-5.5" = "openai/gpt-5.5" }
```

For OpenAI itself, use the same bearer form:

```toml
[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
```

PowerShell example:

```powershell
$env:OPENAI_API_KEY = "sk-..."
codex-helper
```

A single `*` wildcard is supported, which is useful when a relay wants a provider prefix for a whole model family:

```toml
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
supported_models = { "gpt-*" = true }
model_mapping = { "gpt-*" = "openai/gpt-*" }
```

The provider CLI can write the same fields:

```bash
codex-helper provider add relay \
  --base-url https://relay.example/v1 \
  --auth-token-env RELAY_API_KEY \
  --supported-model gpt-5.5 \
  --model-map gpt-5.5=openai/gpt-5.5
```

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

## Runtime Safeguards

Codex `/responses` and `/responses/compact` SSE streams have an idle watchdog so an upstream that returns HTTP 200 and then stops producing bytes does not leave Codex waiting forever.

- `CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS` controls the per-chunk idle timeout for Codex Responses SSE streams.
- Default: `900` seconds.
- `0` disables the watchdog.
- Values above `86400` seconds are clamped to 24 hours.
- On timeout, codex-helper finishes the client stream with a synthetic `response.failed` SSE event and records `codex_helper_error=upstream_stream_idle_timeout`.

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

For compact diagnostics, filter by request path:

```bash
codex-helper usage find --path responses/compact --limit 20
```

The same filter is available through the local admin API as `GET /__codex_helper/api/v1/request-ledger/recent?path=responses/compact`.

The control trace is enabled by default and is written to:

```text
~/.codex-helper/logs/control_trace.jsonl
```

It records routing selection events such as the compiled route plan, provider endpoint, preference group, skipped higher-priority groups, pinned-route decisions, retry options, and failover reasons. When a lower-priority preference group is selected, the `route_graph_selection_explain` event lists each higher-priority provider endpoint that was skipped and the structured reasons such as `unsupported_model`, `cooldown`, `usage_exhausted`, `runtime_disabled`, or `attempt_avoided`. Set `CODEX_HELPER_CONTROL_TRACE=0` to turn it off, or `CODEX_HELPER_CONTROL_TRACE_PATH` to write it somewhere else. The older `retry_trace.jsonl` file is only written when `CODEX_HELPER_RETRY_TRACE=1`.

Request/debug logs, `control_trace.jsonl`, and the optional `retry_trace.jsonl` share the bounded JSONL retention controlled by `CODEX_HELPER_REQUEST_LOG_MAX_BYTES` and `CODEX_HELPER_REQUEST_LOG_MAX_FILES` (defaults: 50 MiB per active file and 10 rotated files). Oversized active JSONL files rotate on first write, and rotated files are pruned by count and total budget.

Other local helper logs use the same bounded storage primitive with separate knobs:

- `runtime.log`: `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES` (defaults: 20 MiB, 10 files).
- `gui.log`: `CODEX_HELPER_GUI_LOG_MAX_BYTES` / `CODEX_HELPER_GUI_LOG_MAX_FILES` (defaults: 20 MiB, 10 files).
- `codex_relay_evidence.jsonl`: `CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_BYTES` / `CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_FILES` (defaults: 20 MiB, 10 files).

For route-continuity diagnosis, control trace fields are intentionally provider-opaque:

- `continuity.class` / `continuity_class`: `stateless_or_session_preferred` or `provider_state_bound`.
- `affinity.source`: `session_route_affinity` when a known affinity constrained selection, or `none`.
- `provider_failover_allowed`: whether helper may move to another provider endpoint for this request.
- `provider_failover_blocked_reason`: why provider failover was blocked, for example `provider_state_bound` or `state_bound_compact_missing_affinity`.
- `balance_signal_authoritative`: currently `false` for compact continuity blocks. A balance probe can explain routing demotion, but it does not prove that a state-bound compact request is safe to move to another provider endpoint.

If a state-bound compact request has no restored route affinity and the request returns a local continuity error, look for a `route_continuity_blocked` event with `reason = "state_bound_compact_missing_affinity"`. That means the active policy refused to bootstrap by choosing a provider endpoint; it does not mean helper identified the relay as sub2api, New API, OpenAI, or any other backend. Under `fallback-sticky`, no-affinity compact requests are normally sent through the configured route graph instead of producing this local block.

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

For a monthly-first setup, the generated default is `affinity_policy = "fallback-sticky"`, because relay providers often bind cache and encrypted response state to an upstream account. If you prefer automatic return to the best monthly group after an outage, explicitly set `affinity_policy = "preferred-group"`. If the route keeps using paygo unexpectedly, look for one of these causes:

- an explicit session/global route target override is set;
- the monthly provider is disabled or missing auth;
- the requested model is unsupported by the monthly provider;
- the monthly endpoint is cooling down after retryable failures;
- trusted balance data marks the endpoint `usage_exhausted`;
- the config uses `affinity_policy = "fallback-sticky"` or `hard`.

Trusted balance exhaustion is a provider-endpoint runtime signal. It can demote a monthly endpoint for the current request/refresh window, but it is not a permanent session preference. If every candidate is currently blocked by trusted exhaustion or cooldown, Codex streaming turns receive a retryable `response.failed` SSE with a bounded delay instead of repeatedly hitting depleted upstreams; the helper also queues a throttled balance refresh so recovered relays can re-enter routing. If a provider reports misleading zero balances for an active subscription, set `trust_exhaustion_for_routing = false` for that usage provider or fix the balance extractor.

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

During migration, codex-helper writes missing route-graph affinity as `affinity_policy = "fallback-sticky"` so the on-disk config is explicit. Existing configs can still set either policy depending on whether official relay continuity or fastest return to the preferred group matters more; configs that explicitly keep `preferred-group` may be called out in migration previews so operators notice the trade-off.

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

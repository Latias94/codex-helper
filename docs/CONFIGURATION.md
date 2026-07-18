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

The public config is expressed in `provider`, `endpoint`, and `route graph` terms. Runtime routing uses those identities directly.

## Local Proxy Vs Outbound Proxy

There are two different proxy layers:

- Local proxy: Codex connects to codex-helper, usually at `127.0.0.1:3211`. This still happens when you do not configure an outbound network proxy.
- Outbound proxy: codex-helper connects to provider endpoints, relay dashboards, or balance APIs through a network proxy.

Current outbound proxy support comes from the underlying HTTP client's system/environment proxy behavior. `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` may affect provider and balance requests. There is not yet a first-class `config.toml` outbound proxy section. See [Outbound Proxy](#outbound-proxy) for the current behavior and the intended future model.

## File Locations

- Main config: `~/.codex-helper/config.toml`
- Runtime state: `~/.codex-helper/state/state.sqlite`
- Balance adapters: `~/.codex-helper/usage_providers.json`
- Pricing overrides: `~/.codex-helper/pricing_overrides.toml`
- Post-commit request debug log: `~/.codex-helper/logs/requests.jsonl`
- Routing/control trace: `~/.codex-helper/logs/control_trace.jsonl`
- Codex relay diagnostic evidence: `~/.codex-helper/logs/codex_relay_evidence.jsonl`

Codex-owned files remain owned by Codex:

- `~/.codex/auth.json`
- `~/.codex/config.toml`

Only an explicit local `switch on/off` action may patch `~/.codex/config.toml`, and it is limited to the helper-owned provider selector and `model_providers.codex_proxy` stanza. codex-helper never reads or writes the Codex model cache or SQLite. Ordinary runtime operation does not write `auth.json`; however, when a provider explicitly configures `auth_token_env` / `api_key_env` and the running process lacks that variable, a Codex runtime may read only the same-named top-level string field from `$CODEX_HOME/auth.json`. The only auth-writing exception is the one-time legacy switch recovery below, which may restore an auth facade managed by the old helper while it strictly matches the old patch.

## Automatic Configuration Migration

Normal startup automatically upgrades historical helper configuration to the current `version = 5` TOML contract:

- A v1-v4 or unversioned `config.toml` is validated, backed up to `config.toml.bak`, and atomically replaced with canonical version 5 TOML.
- If `config.toml` is absent but `config.json` exists, the JSON source is backed up to `config.json.bak` and a new canonical `config.toml` is written. The original `config.json` is not deleted. `config init` uses this same migration path and prints the migration report instead of claiming that it wrote a blank template.
- Historical station-shaped JSON is validated against the nullable fields accepted by the last published JSON loader. `null` remains valid only for those optional fields; a non-optional `null`, or any `null` in a later provider-shaped JSON file that had no published nullable contract, fails closed.
- The migrator understands the historical station, group, routing, and version 4 route-graph shapes. It moves flat `[retry]` fields `max_attempts`, `backoff_ms`, `backoff_max_ms`, `jitter_ms`, `on_status`, `on_class`, and `strategy` into `[retry.upstream]` only when that table is absent. If `[retry.upstream]` already exists, it is the complete historical override and all flat retry fields are ignored with a warning. The migrator removes retired settings and validates the complete result against the current typed schema before replacing the canonical file.
- Malformed input, a version newer than 5, an invalid migrated result, a symbolic-link source, or a source changed while migration is being prepared fails closed. The source is not replaced.
- Concurrent automatic startups wait for the active config writer, then recheck whether migration is still required. Explicit init, save, and migrate mutations remain fail-fast when another writer owns the lock.

To inspect the exact output and migration warnings before startup writes anything, use the explicit preview command. Applying it requires both write flags:

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Startup uses the same path to clean a current version 5 file that still contains retired fields; `config migrate` lets you preview or apply that cleanup explicitly. Explicit migration of an already-clean version 5 file is a no-op and does not replace an existing legacy backup. Backups are safety copies, not a versioned archive: preserve a backup elsewhere before another migration or forced initialization if you need long-term history.
Inline `auth_token` and `api_key` values are redacted in preview output; the validated file written by migration retains them.

## Relay Targets

Relay targets are client-side bookmarks for local or remote codex-helper runtimes. They live in `~/.codex-helper/config.toml` and are used by `ch relay ...`; provider/routing config still belongs to the server runtime that receives traffic.

```toml
[relay_targets.nas]
service = "codex"
proxy_url = "http://nas.local:3211"
admin_url = "https://nas.example.com:4211"
admin_token_env = "CODEX_HELPER_NAS_ADMIN_TOKEN"
```

Equivalent CLI:

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url https://nas.example.com:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN
```

`local` is built in and resolves to the normal loopback ports for the current `default_service`. `ch relay local` starts the normal local foreground flow. Named targets are remote by default: `ch relay nas` starts or attaches to the selected runtime and opens its read-only TUI; it never changes Codex client configuration. `--no-tui` omits the console, while `--attach-only` requires an already-running runtime. To point Codex at a target, run `codex-helper switch on --base-url <PROXY_URL>` as a separate explicit action.

`admin_token_env` stores the environment variable name, not the token value. A remote admin URL must use HTTPS; HTTP is accepted only for loopback. A trusted SSH/Tailscale tunnel can expose the remote admin listener on a client loopback URL. Remote targets must set `admin_url` explicitly; runtime responses and redirects cannot replace that configured authority. URLs containing userinfo, query credentials, fragments, or paths are rejected.

## Fleet Observer Registry

The Fleet page is read-only. It can observe local and remote runtimes, but it does not send interrupts, messages, approvals, or TTY attaches to remote nodes.

Fleet targets live under `[fleet.nodes.*]` and are separate from `relay_targets`:

```toml
[fleet.nodes.workstation]
label = "Workstation"
admin_url = "https://workstation.example.com:4211"
admin_token_env = "CODEX_HELPER_WORKSTATION_ADMIN_TOKEN"
enabled = true

[fleet.nodes.mini]
label = "Mac mini"
admin_url = "https://mac-mini.tailnet.example.ts.net:4211"
admin_token_env = "CODEX_HELPER_MAC_MINI_ADMIN_TOKEN"
enabled = true
```

`admin_token_env` names the environment variable that holds the admin token. Do not put a raw token string there. Non-loopback nodes require HTTPS and `admin_token_env`; when using a trusted encrypted tunnel, terminate it on a client loopback URL.

`ch tui` renders the Fleet page at `9`, with `r` for refresh, `Tab` to switch between nodes and work units, and `t` to switch between tree and flat work-unit views.

## Explicit Codex Client Switch

Client switching is a separate local action from starting, selecting, or diagnosing a runtime. No server, relay bookmark, TUI refresh, desktop action, or capability result changes Codex configuration implicitly.

```bash
codex-helper switch on                         # http://127.0.0.1:3211
codex-helper switch on --port 4321
codex-helper switch on --base-url https://relay.example/v1
codex-helper switch status
codex-helper switch off
```

`switch on` records the original selector and helper stanza, then writes only the helper-owned `model_providers.codex_proxy` stanza and selects it. `switch off` restores only the recorded selector/stanza. The recovery journal lives under `~/.codex-helper/state/`; an external edit that makes the current file match neither the original nor helper-applied fingerprint moves the switch to `recovery_required` and leaves the file untouched for human reconciliation.

Except for the one-time v0.20.3 legacy-state recovery below, the switch never reads or changes `~/.codex/auth.json`, `models_cache.json`, Codex SQLite, unrelated providers, feature flags, compaction settings, WebSocket settings, or hosted-tool settings. Provider capabilities come from the selected provider contract and live observations, not from switch configuration.

### Upgrading From 0.20.3 Or Earlier

Releases through 0.20.3 used a different switch implementation and stored recovery data in `~/.codex/codex-helper-switch-state.json`. That state can contain the original provider selector and, for the old bridge presets, the original `auth.json` content. The current binary reads it only during an explicit `switch on` or `switch off` for one-time safe recovery; the new journal itself stores no auth content.

Use this upgrade order if the legacy state file exists:

1. Upgrade codex-helper and first run `codex-helper switch status`. This is read-only: it reports the legacy state as `recovery_required` without printing or rewriting its authentication material.
2. Run `codex-helper switch off` to restore the pre-upgrade Codex configuration. It automatically restores the selector/provider stanza and any verifiable auth facade managed by the old helper.
3. To keep using the helper, the new `codex-helper switch on` may be run directly instead: it performs the same legacy recovery first, then creates a new journal and applies the new target.

Recovery runs under the exclusive switch-operation lock; do not run old and new switch commands concurrently. A config or auth file is restored only while it still matches the old helper patch, so external edits are preserved. Malformed or unknown legacy state, or a legacy state that conflicts with a current journal, fails closed without rewriting either state. The legacy file is deleted only after durable recovery completes; until then, do not delete, edit, share, or paste it into an issue. Running `switch off` with the old binary remains valid when it is still available, but is no longer an upgrade prerequisite.

The removed `switch remote-control enable` command also had persistent side effects outside codex-helper: it could add `[features].remote_connections = true` to `~/.codex/config.toml` and enable a `remote_control` row in Codex App SQLite. Upgrading does not undo either value, and the current helper never reads or writes that database. If the TOML key was added solely for the old helper workflow and is no longer wanted, back up the Codex config before removing that key manually. Do not edit Codex SQLite with ad hoc SQL; leave database cleanup to Codex-supported controls or Codex support guidance.

The 0.20.3 `~/.codex-helper/state/session-route-affinities.json` file is generated runtime state rather than user configuration. The new release neither imports it into `state.sqlite` nor rewrites or deletes it. Finish any active state-bound session that depends on multi-endpoint affinity before upgrading, then start a new session after the upgrade. Do not try to repair an old session by sending an ordinary Responses request with the same session key: the current route may select a different provider endpoint from the one that owns the upstream state. If an old state-bound request is attempted without restored affinity, `fallback-sticky` may select through the current graph and leave validity to the upstream, while `hard` fails closed locally on a multi-endpoint graph.

Proxy lifecycle is independent. `codex-helper serve` is foreground by default, `--resident` keeps it running after the console exits, and `codex-helper tui` attaches a read-only console. None of these commands run `switch on` or `switch off`. Resident runtimes write advisory owner markers under `~/.codex-helper/run/`; inspect them with the read-only `codex-helper daemon status`. Manage an installed local runtime with `codex-helper service start/stop/restart`; there is no remote HTTP shutdown command.

codex-helper normalizes HTTP request `Content-Encoding` before inspection and forwarding. Supported encodings are `zstd`, `gzip` / `x-gzip`, `br`, and `deflate`; after decoding, helper forwards ordinary JSON and removes stale `Content-Encoding` / `Content-Length`. Set `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` only when an upstream requires the exact compressed body.

When Codex sends no stronger session header (`session_id`, `session-id`, `conversation_id`, or `thread-id`), decoded JSON `prompt_cache_key` is used as the session-affinity key so normal Responses and compact requests can remain on the same selected provider endpoint.

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

The CLI capability diagnostic is an explicit, manual, process-local operator action. Run it from a shell:

```bash
codex-helper codex relay-capabilities \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default
```

The command accepts only the optional canonical provider-endpoint selector (`--provider` with optional `--endpoint`) and an optional model. With no selector, it uses the current runtime target. Legacy station names and positional upstream indexes are rejected. Client assumptions such as `--preset`, `--mode`, and `--compaction` are also rejected. The bounded diagnostic probes the selected endpoint's `/models`, `/responses`, and `/responses/compact` endpoints without using normal retry/failover, request accounting, affinity, passive health, or policy state.

The response includes:

- required `provider_id`, `endpoint_id`, and `provider_endpoint_key` identity, plus provider adapter, captured catalog revision, request dialects, and selected model;
- `expected`, the provider-owned capability decisions for Responses, compact, hosted image generation, WebSocket, ultra mapping, web search, apply patch, and reasoning summaries;
- `observed`, the validation-only `/models`, `/responses`, and `/responses/compact` results, confidence, and translation evidence;
- `continuity`, including the selected continuity domain, endpoint counts, affinity policy, warnings, and recommendations;
- `mismatches`, where observed endpoint behavior disagrees with the captured provider contract.

Capability results never change client configuration, provider configuration, routing, or policy state. JSON output is available with `--json`.

For sub2api-style relays, a raw OpenAI `/models` response (`data: [...]`) is fine only if
codex-helper translates it into the Codex `models: [...]` catalog before Codex sees it. The
diagnostic response reports this as `observed.models.translation_required = true`. For non-sub2api
relays, the same rules apply: the relay can either return Codex-shaped model metadata directly or
return an OpenAI model list that codex-helper can translate. If the selected model is absent or its
metadata is not authoritative, model-scoped capability decisions remain `unknown`.

Hosted `image_generation` is not actively probed by this diagnostic endpoint because that can spend
quota or create image artifacts, so the contract reports it without fabricating live evidence.
Responses WebSocket support comes from the captured provider/model catalog. If Codex sends a
`compaction_trigger`, helper recognizes the remote-compaction-v2 request shape for lifecycle and
route-continuity protection, but the upstream still has to return valid v2 compaction items.

The provider contract and continuity model deliberately separate two ideas:

- Endpoint capability may prove the Responses and `/responses/compact` protocol surfaces.
- Protocol support does not prove that two provider endpoints share upstream encrypted response state.

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
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5
```

With no optional case flag, live smoke only checks remote compaction v1 through `/responses/compact`.
Remote compaction v2, hosted image generation, and Responses WebSocket are never part of the default
case set. To explicitly test Codex remote compaction v2 compatibility for the selected
relay/provider chain, pass `--compact-v2`. The smoke sends `POST /responses` with
`stream: true`, one `compaction_trigger` input item, and `x-codex-beta-features:
remote_compaction_v2`; it passes only when the stream contains exactly one compaction output item
and `response.completed`:

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default \
  --compact-v2
```

To explicitly test the hosted tool request path:

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --image
```

To explicitly test the selected upstream's Responses WebSocket v2 path, pass
`--websocket`. The smoke opens `GET /responses` as a WebSocket, injects
`OpenAI-Beta: responses_websockets=2026-02-06`, sends one minimal `response.create` frame, and
passes when the relay returns a `response.*` event or a Codex WebSocket protocol event such as
`codex.rate_limits`:

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default \
  --websocket
```

Use `codex-helper codex relay-evidence --limit 20` to inspect local sanitized summaries.

For the CLI, omitting optional case flags runs the default compact smoke. Supplying `--compact-v2`,
`--image`, `--websocket`, or any combination runs only those explicit optional cases, so an optional smoke does not
accidentally spend an additional compact request.

Targeting uses the current runtime target by default. Diagnostics may target one canonical provider
endpoint with `--provider` and optional `--endpoint`; legacy `--station` and `--upstream-index`
selectors are not accepted.

Live smoke is intentionally isolated from normal routing behavior. It selects one provider endpoint, sends at
most one request/connection per selected case, bypasses route retry/failover, and does not write
request ledger entries, route affinity, passive health, runtime health, balance state, or
client/config changes. Image responses are summarized only: codex-helper reports whether an
`image_generation_call` appeared, but does not store raw image bytes or base64 payloads.

Capability diagnostics and live smoke append sanitized summaries to
`~/.codex-helper/logs/codex_relay_evidence.jsonl`. This evidence store is local operator memory,
not routing truth. It does not feed request ledger summaries, load balancing, session affinity,
passive health, balance exhaustion, retry policy, or client switching. Use
`codex-helper codex relay-evidence --json` when you want machine-readable records for bug reports or
relay comparisons.

Stored and printed evidence identifies the target only by `provider_id`, `endpoint_id`, and
`provider_endpoint_key`; configured upstream base URLs and raw upstream payloads are neither stored
nor printed. Evidence can be filtered by canonical provider ID with `relay-evidence --provider`.

To diagnose whether remote compaction v1 is active, inspect the codex-helper request ledger after a Codex compaction happens:

```bash
codex-helper usage find --path responses/compact --limit 20
codex-helper usage find --path responses --limit 20
```

An HTTP compact request appears as `POST /responses/compact`; remote compaction v2 travels through ordinary `/responses` with a structured `compaction_trigger` item. A WebSocket turn uses a `GET /responses`-style upgrade. The request ledger records the path and captured provider endpoint without inferring client-side capability settings.

Authentication is origin-scoped. Client authentication may pass only to the official OpenAI origin; third-party relays must configure helper-side `auth_token_env`, `auth_token`, or equivalent API-key credentials, and Codex client account headers are stripped before forwarding. A remote third-party Codex endpoint without helper credentials now returns 503 before any upstream I/O by default. Set provider-level `allow_anonymous = true` only when the relay intentionally requires no authentication. Loopback endpoints allow anonymous access automatically, while the official OpenAI origin continues to receive safe client credential passthrough. A declared `auth_token_env` or `api_key_env` reference that cannot be resolved still fails closed even when anonymous access is enabled.

This is a backward-compatible addition within version 5, so existing files do not need a schema rewrite. Providers that already configure helper credentials need no migration. A provider that previously relied on anonymous remote access must add credentials or record that intent explicitly:

```toml
[codex.providers.anonymous_relay]
base_url = "https://relay.example/v1"
allow_anonymous = true
```

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

### Reasoning Guard: Catch Reasoning Token Anomaly Buckets

If a Codex relay occasionally returns a successful response with `reasoning_tokens = 516`, `1034`,
`1552`, or another `518*n-2` boundary, goes straight to a final answer, and produces visibly
degraded answers, enable the retry reasoning guard. The guard only uses upstream usage metadata as a
high-confidence signal; it does not try to judge whether the answer text is correct.

```toml
[retry.reasoning_guard]
# Master switch. Defaults to false; the guard only acts when explicitly enabled.
enabled = true
# Fixed anomaly buckets: trigger when reasoning tokens exactly match one of these values.
reasoning_equals = [516, 1034, 1552]
# Sequence anomaly bucket: also match reasoning_tokens = 518*n-2. Defaults to n<=4; set 0 to disable.
boundary_sequence_max_n = 4
# Match action: retry rewrites the response to a local 502; block rejects; observe only logs.
action = "retry"
# Streaming inspection mode: strict-buffer holds SSE until terminal usage is available.
stream_mode = "strict-buffer"
# Maximum extra upstream rounds per client request caused by this guard.
max_guard_retries = 1
# What to do if the response still matches after the retry budget: pass it through or block it.
on_retry_exhausted = "pass"
# Limit the guard to Codex/Responses-compatible paths.
paths = ["/v1/responses", "/responses", "/v1/chat/completions", "/chat/completions"]
# Emit control-trace events for matches so TUI Requests and logs can explain the decision.
log_matches = true
```

- The guard is disabled by default, so existing configs keep their current behavior. When enabled,
  the default fixed match list is `reasoning_equals = [516, 1034, 1552]`, and the guard also matches
  `518*n-2` boundaries where `n <= 4`. Override `reasoning_equals` for a custom fixed list, or set
  `boundary_sequence_max_n = 0` to disable sequence matching.
- The recommended starting point is the example above: `action = "retry"` plus
  `stream_mode = "strict-buffer"` lets codex-helper catch the anomalous response before it reaches
  Codex. Use `action = "observe"` first if you only want to measure match frequency.
- `action = "retry"` rewrites a matching successful response into a local 502 and lets the normal
  `[retry]` upstream/provider policy handle it. `max_guard_retries = 1` means one extra upstream
  request per client request due to this guard. If the response still matches after that budget,
  the default `on_retry_exhausted = "pass"` forwards the final upstream response to Codex so the
  helper does not interrupt the task. Set it to `"block"` only when you prefer hard rejection.
- `stream_mode = "strict-buffer"` buffers matching SSE responses until the terminal usage block is
  available. This prevents anomalous output from being sent before the guard can inspect
  `reasoning_tokens`, at the cost of losing live streaming for those guarded requests.
- Runtime config reload applies to this guard: every new request checks for config file changes
  before building its retry plan; in-flight requests keep the config snapshot they started with.
- The TUI Requests page shows hits in the `RG` column. The details pane's `Retry / route chain`
  shows `decision=failed_reasoning_guard`, `class=reasoning_guard_triggered`, and
  `reason=reasoning_tokens=<matched value>`. A final response passed after retry exhaustion is
  recorded as a normal completion, with control-trace event `action=exhausted-pass`.

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
- `round-robin`: spread selections across children with smooth weights derived from each candidate's remaining local concurrency capacity. An existing viable session affinity wins before the rotation cursor.
- `tag-preferred`: split children into preferred groups by `prefer_tags`, then fallback to the rest. `on_exhausted = "continue"` allows paid fallback after trusted exhaustion; `on_exhausted = "stop"` prevents automatic spillover.
- `manual-sticky`: use one explicit `target`. The target can be a route node, provider, or provider endpoint.

Most users should prefer `ordered-failover` for fixed priority, `round-robin` for concurrent relay pools, and `tag-preferred` for "monthly first" business intent.

## Session Affinity

Route graph session affinity is runtime state with a small durable ledger for Codex route continuity. The TOML config chooses the affinity policy and can optionally bound fallback stickiness:

- `fallback-sticky` is the default used by the canonical version 5 config template. It keeps a session on the last successful fallback provider while that provider remains viable, which is safer for official relay features such as remote compaction that may carry upstream-account-bound encrypted state. Set `fallback_ttl_ms` to cap how long a lower-priority fallback affinity can be reused, or `reprobe_preferred_after_ms` to force a preferred-group reprobe after a fallback target change.
- `preferred-group` applies session affinity only inside the currently best available preference group, so a session that temporarily falls back to paygo returns to monthly as soon as a monthly provider is viable again.
- `off` ignores automatic route affinity.
- `hard` treats an existing affinity target as strict for that route graph; if the target is unavailable, no alternate candidate is selected.

Within one helper runtime store, each session id has at most one durable provider/key binding. The record also carries a versioned canonical SHA-256 route-graph key that validates whether the binding still applies to the current graph; it is not a second database-key dimension. Scheduling presets, `max_concurrent_requests`, `limit_group`, provider display aliases, and route-node display metadata do not change that graph key, while route selection rules, provider endpoint identity, or configured `auth_token` / `api_key` credentials do. Adjusting capacity controls therefore preserves an existing durable binding, although the current scheduling preset still controls how a request behaves while its bound key is saturated. When the graph key changes, the old binding is ignored and the next successful route replaces that session's single record. Client-passthrough account headers and external credential fallbacks are not part of this durable identity, so changing either requires a new session.

Successful route affinity is committed to the helper-owned runtime database:

```text
~/.codex-helper/state/state.sqlite
```

The runtime store records helper-owned provider endpoint identity only; it does not store or infer upstream relay implementation details. Affinity persistence shares the runtime store ownership and durability settings and cannot be redirected to a separate JSON ledger.

With a `round-robin` route and an affinity policy such as `fallback-sticky`, the rotation is consulted only when the request's session does not already have a viable affinity. The first request for a cold session owns an in-process provisional reservation; until that request succeeds and commits durable affinity, another concurrent follower for the same session receives local HTTP 429 backpressure and never selects a different key. With `affinity_policy = "off"`, no session reservation or durable route affinity is applied. Several different sessions may bind to the same provider/key. `max_concurrent_requests` limits simultaneous requests, not the number of sessions assigned to that key.

For Codex remote compaction, helper treats compact v1 requests that mention state-bound fields such as `encrypted_content`, `previous_response_id`, or `compaction_summary`, and compact v2 requests with a structured `compaction_trigger`, as provider-state-bound. Under the default `fallback-sticky` route affinity policy, a state-bound compact request without existing route affinity is still tryable: helper follows the configured route graph, records the successful provider endpoint as the session affinity, and lets upstream decide whether the compact state is valid. Under `hard` affinity, missing affinity remains fail-closed with an explicit continuity error. If a known affinity endpoint itself fails, `fallback-sticky` may continue along the route graph and update affinity, while `hard` blocks cross-endpoint movement unless an explicit shared `continuity_domain` permits it. Non-state-bound compact can still use normal provider fallback according to the route policy.

Affinity is not a hard pin:

- request retry, provider health, capability mismatch, cooldown, and trusted balance exhaustion still apply;
- if the sticky provider fails, ordinary and non-state-bound requests continue through the current route graph and then stick to the next successful provider;
- provider-state-bound compact honors the route affinity policy: `fallback-sticky` stays tryable and updates affinity after a successful fallback, while `hard` stays within the affinity continuity domain unless an explicit shared `continuity_domain` permits movement;
- if provider tags, route node strategy, children, entry, provider endpoint identity, or configured `auth_token` / `api_key` credentials change, the route graph key changes, the old affinity no longer matches, and the next successful route replaces the session's single binding; changing only `scheduling_preset`, `max_concurrent_requests`, `limit_group`, or display aliases/metadata preserves it;
- route graph decisions use route/provider/endpoint controls rather than a second station-shaped override path.

This means monthly pools such as `monthly_pool -> paygo` normally keep a conversation on one monthly provider until that provider stops being viable, instead of round-robining every request and reducing upstream cache hit rate.

## Recipes

Pick one recipe first. You can refine fields later. For Claude, replace `codex` with `claude`.

| User Goal | Start With | Why |
| --- | --- | --- |
| I only have one upstream and want the dashboard/logs | [One Provider](#one-provider) | Smallest config; no accidental fallback |
| I have several relays and want the first working one | [Ordered Fallback](#ordered-fallback) | Simple left-to-right fallback |
| I have several relays with different concurrency limits | [Capacity-Weighted Round Robin](#capacity-weighted-round-robin) | Spreads new sessions by remaining local capacity, then keeps session affinity |
| I have several monthly relays and one pay-as-you-go backup | [Monthly Pool With Paygo Fallback](#monthly-pool-with-paygo-fallback) | Preserves the monthly pool as one preferred group |
| I have several monthly relays and several paid relay backups | [Monthly Pool With Relay Fallback Pool](#monthly-pool-with-relay-fallback-pool) | Keeps monthly and paid fallback pools explicit |
| I want all monthly-tagged providers before anything paid | [Monthly First By Tag](#monthly-first-by-tag) | Uses metadata instead of hard-coding a named pool |
| I would rather fail than spend pay-as-you-go money | [Monthly Only](#monthly-only) | Stops after trusted monthly exhaustion |
| I need to force one provider temporarily | [Manual Pin](#manual-pin) | Explicit and easy to undo |
| One provider account has multiple upstream endpoints | [Multiple Endpoints For One Provider](#multiple-endpoints-for-one-provider) | Keeps one provider identity with endpoint-level routing |

Routing decisions use runtime provider endpoints. Diagnostics and balance DTOs expose `provider_endpoint_key`, `provider_id`, and `endpoint_id` directly.

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

### Capacity-Weighted Round Robin

Use `round-robin` when independent relay accounts should share new sessions in proportion to their available concurrency. This example gives `input` 20 local request slots and `ciii` 15:

```toml
version = 5

[codex.providers.input]
base_url = "https://input.example/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.input.limits]
max_concurrent_requests = 20

[codex.providers.ciii]
base_url = "https://ciii.example/v1"
auth_token_env = "CIII_API_KEY"

[codex.providers.ciii.limits]
max_concurrent_requests = 15

[codex.routing]
entry = "relay_pool"
affinity_policy = "fallback-sticky"
scheduling_preset = "balanced"

[codex.routing.routes.relay_pool]
strategy = "round-robin"
children = ["input", "ciii"]

[retry]
profile = "balanced"
```

At idle, new sessions are selected in a smooth 20:15 ratio, equivalent to 4:3 over time. The weight is recalculated from `max_concurrent_requests - active_requests`, so traffic shifts toward the provider with more remaining capacity; a saturated candidate is skipped for immediate selection, while `scheduling_preset` may wait for capacity before trying again. Once a session succeeds on one provider, affinity keeps later requests on that provider while it remains viable. This is session-to-key affinity, not a one-session-per-key allocation: each key may serve many sessions concurrently.

The cursor and active-request counters are process-local. Run one codex-helper process when these values must represent the whole pool; multiple helper processes do not coordinate a distributed limit. A candidate without `max_concurrent_requests` has weight 1, so configure explicit limits on every child when capacity-proportional distribution matters.

If a `round-robin` child is itself an `ordered-failover` route, the best group from every child joins the shared rotation first. Lower fallback groups become eligible only after all candidates in the preceding group are unavailable. Keep the graph flat when every provider belongs to the same capacity pool.

### Provider Concurrency Limits

Use `limits.max_concurrent_requests` when an upstream relay account only allows a small number of simultaneous requests. This is a local-process cap: one running codex-helper process tracks active requests and applies the route's configured queue/failover policy. It is not a distributed quota across several codex-helper processes.

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

Every endpoint in the same explicit `limit_group` must declare the same limit because the group is one shared local semaphore. Do not put independent accounts such as the 20-slot and 15-slot providers above into one group. For round-robin weighting, all candidates that share a `limit_group` observe the same active pool; the group prevents multiple endpoint aliases from multiplying one account's real capacity.

`scheduling_preset` under `[codex.routing]` or `[claude.routing]` controls what happens when the selected candidate reaches that local cap:

```toml
[codex.routing]
entry = "main"
affinity_policy = "fallback-sticky"
scheduling_preset = "balanced"
```

| Preset | Saturated-candidate behavior |
| --- | --- |
| `continuity-first` | Wait up to 8 seconds for capacity before trying another candidate |
| `balanced` | Wait up to 2 seconds, then continue through the configured fallback order; this is the new default |
| `throughput-first` | Do not wait; immediately try the next available candidate |

Version 0.20.3 and earlier immediately failed over when the local cap was saturated. Set `scheduling_preset = "throughput-first"` explicitly to preserve that behavior after upgrading. Saturation does not count as a provider failure, open a cooldown, or poison session affinity. If every candidate remains saturated or unavailable after the selected wait policy, the request exits through the normal route-unavailable path instead of inventing a provider. For shared upstream accounts, put the same `limit_group` on every endpoint that consumes the same quota so the runtime treats them as one concurrency pool.

## Route Strategies

| Strategy | Best For | UI Mental Model |
| --- | --- | --- |
| `ordered-failover` | Simple fallback chains and named pools | Reorder child routes/providers |
| `round-robin` | Concurrent relay pools with per-provider capacity | Rotate new sessions by remaining capacity |
| `tag-preferred` | Monthly-first, region-first, vendor-class-first setups | Choose preferred tags, then fallback |
| `manual-sticky` | Debugging or strict manual selection | Pick one target |

`manual-sticky` still respects saturation and availability for the pinned target itself. It does not change the route graph's fallback rules for other requests, and it should not be used as a queueing policy.

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
| `allow_anonymous` | Explicitly permit credential-free routing to a remote third-party Codex endpoint | Default off; use only for intentionally unauthenticated relays; it does not bypass an unresolved explicit credential reference |
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
  The value in config is a credential reference name, not the secret itself. Resolution order is
  inline value, the environment of the running codex-helper process, then the explicitly referenced
  client credential field. Codex looks for an exactly same-named top-level string field in
  `$CODEX_HOME/auth.json`; Claude looks under the `env` object in `$CLAUDE_HOME/settings.json`.
  Without a configured `*_env` reference, these files are not scanned and fields are not guessed.
- If an inline value and an env reference are both configured for the same header family, the
  inline value wins. If both bearer and `X-API-Key` credentials are configured, codex-helper sends
  both headers; avoid that unless the relay explicitly requires it.

### Native credentials in OS services

For an installed desktop service, a native credential is the portable default because it is read in the service's real user context and is never copied into TOML, argv, the service definition, or the install receipt:

```bash
codex-helper credential create relay.primary
codex-helper provider set-auth relay --kind bearer --native relay.primary --codex
codex-helper service install --codex
codex-helper service status --json
```

Interactive create/set uses masked input and confirmation. Use explicit `--stdin` only for automation. `provider set-auth` stores the typed reference, not its value. The native backend is Windows Credential Manager, the macOS user Keychain, or the Linux session's Secret Service implementation (for example GNOME Keyring or KWallet).

`service install`, `service start`, and `service restart` first evaluate configuration and credentials offline. A blocked preflight fails before replacing or stopping the current service. After launch, the command polls the signed loopback operator model and verifies the service kind, helper home, client home, and non-secret install generation from the committed receipt. `ready` succeeds, `degraded` succeeds with a warning, and `blocked` returns nonzero while leaving the daemon and local admin listener running for diagnosis. No readiness check sends upstream traffic.

`service install --no-start` verifies only the installer process context. It deliberately reports the installed service context as unverified until `service start` or `service restart`. `service status` keeps OS state separate from `receipt_state`, `credential_context`, and `runtime_identity_verified`; JSON remains available when the process is stopped, the receipt is absent/legacy/invalid, or the admin endpoint is unreachable. Reinstall to replace an absent or legacy receipt.

The generated Scheduled Task, LaunchAgent, and systemd user unit contain only the executable, selected homes, listen settings, and install generation. They do not capture arbitrary shell environment variables. An `--environment` binding is therefore valid only when that variable is independently present in the actual service manager environment. For predictable service deployments, use a native binding or an absolute `--secret-file` path readable by the service user. On Linux, a missing session bus, unavailable Secret Service, or locked collection is reported as blocked; codex-helper does not create a file or SQLite fallback.

On Windows, `service install` registers a SID-scoped per-user Scheduled Task instead of a LocalSystem SCM service. Its actual name resembles `codex-helper-S-1-5-21-...`; `service start/stop/status/uninstall` resolves the current Principal SID automatically, so normal CLI use does not need that name. It runs with `InteractiveToken` and `LeastPrivilege`, starts at that user's logon, and may also be run through `service start/restart`. To migrate an older SCM installation, run the new binary once from an elevated PowerShell:

```powershell
& $helper service install --codex --no-start
& $helper service restart
& $helper service status
```

Installation first preflights the executable, paths, SID, PowerShell/ScheduledTasks commands, credentials, and XML read-back, then registers and queries the new SID-scoped task. It publishes the matching receipt only after that verification and before retiring any older installation. It retires a fixed-name task owned by the current SID and the legacy SCM service only after the new owner SID, action, logon trigger, and least-privilege settings are verified. Definition, receipt, permission, query, registration, and verification errors fail closed and restore the previous artifacts when rollback can be proven. A fixed-name `codex-helper` task owned by another SID is never overwritten or deleted. If status still reports `legacy LocalSystem SCM service` or `legacy fixed-name per-user scheduled task`, rerun installation from an elevated terminal.

On macOS, the receipt targets the logged-in user's `gui/<uid>` LaunchAgent domain and the same user's Keychain. On Linux, it targets `systemctl --user` and the user's session bus. Installing as one user and starting or inspecting as another intentionally fails identity/readiness verification instead of falling back to another credential store.

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

Profiles define request defaults only; provider selection belongs in `[codex.routing]`.

## Balance Adapters

Most relay users do not need to write `usage_providers.json` just to see balances. The file is optional and operator-owned: when it is absent, codex-helper uses in-memory built-ins without creating it. An unreadable or invalid file produces an explicit load error and is never replaced or rewritten. If no explicit adapter matches an upstream, codex-helper tries common relay probes:

1. `sub2api_usage`: `GET /v1/usage` on the normalized provider origin with the model API key.
2. `new_api_token_usage`: `GET /api/usage/token/` on the normalized provider origin with the model API key.
3. `new_api_user_self`: `GET /api/user/self` on the normalized provider origin with dashboard-style auth.
4. `openai_balance_http_json`: `GET /user/balance` on the normalized provider origin with the model API key.

RightCode hosts (`www.right.codes` / `right.codes`) are special-cased before the generic relay probes. The built-in `rightcode_account_summary` adapter calls `GET https://www.right.codes/account/summary`, uses bearer auth, reads wallet `balance`, and matches subscription daily quota by the upstream path prefix such as `/codex`.

Explicit adapters are still useful when a relay needs independent dashboard credentials, a provider-kind-specific field, a custom endpoint, or safer exhaustion handling.

Request-driven balance observations are coalesced with a 60-second delay by default, and the same provider is auto-polled at most once every 600 seconds. Explicit `poll_interval_secs` values below 120 seconds are raised to 120 seconds. Operator clients read the last committed observation; they do not trigger a remote refresh.

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
      "endpoint": "/v1/organization/costs",
      "poll_interval_secs": 600,
      "refresh_on_request": false,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

`OPENAI_ADMIN_KEY` must be an organization-level admin key; a normal model API key is not a stable substitute.

`endpoint` accepts a literal absolute URL or a literal path relative to the normalized provider origin. Endpoint templates are not supported. For `openai_organization_costs`, codex-helper supplies a bounded 30-day `start_time` and `limit=30` on every poll, so the endpoint should not embed those query parameters. Generic `headers` and `variables` are not part of the schema; an adapter containing them fails to load instead of being silently accepted. Credentials must use `token_env` or a provider-kind-specific typed field.

Sub2API API-key telemetry:

```json
{
  "providers": [
    {
      "id": "input-monthly",
      "kind": "sub2api_usage",
      "domains": ["ai.input.im"],
      "poll_interval_secs": 600,
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
      "poll_interval_secs": 600,
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
      "endpoint": "/api/user/self",
      "token_env": "RIGHTCODE_NEWAPI_ACCESS_TOKEN",
      "require_token_env": true,
      "new_api_user_id_env": "RIGHTCODE_NEWAPI_USER_ID",
      "quota_pool_id": "rightcode-shared-account",
      "quota_reset_timezone": "Asia/Shanghai",
      "poll_interval_secs": 600,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

`new_api_user_id_env` names the environment variable whose value is sent through the fixed `New-Api-User` header. It is accepted only by `new_api_user_self`; the variable must be set and non-empty when configured. Arbitrary request headers are intentionally unsupported.

Important balance behavior:

- Lookup failure is displayed as `unknown`, not exhausted, and does not change route graph config.
- Known exhausted snapshots can demote automatic routing only when `trust_exhaustion_for_routing = true`.
- Terminal errors such as inactive accounts, invalid keys, insufficient balance, or exhausted quota temporarily disable that provider target and suppress follow-up balance requests for 6 hours to avoid repeatedly hitting unusable accounts.
- Sub2API lazy subscription-window zeros are displayed as lazy reset state before a real request refreshes the period; they should not be confused with a durable package design choice.
- Sub2API subscription-mode `remaining` is a period-limit capacity signal, not a wallet balance. A zero `remaining` means at least one configured subscription window is currently exhausted; when the current daily/today window is exhausted, codex-helper suppresses follow-up balance requests and temporarily skips that target even if the package signal is display-only.
- New API conversion first probes the same origin's public `GET /api/status` and reads `quota_per_unit`, then falls back to the adapter's positive `quota_divisor`. If neither is available, codex-helper keeps the counters in `raw` units instead of claiming an exact USD conversion. Token usage snapshots with `unlimited_quota = true` are never treated as exhausted.
- RightCode `balance` is shown as wallet balance. Matched `subscriptions[*].total_quota` and `remaining_quota` are shown as daily quota; `reset_today = false` means codex-helper includes today's fresh daily quota before displaying remaining quota.
- If a provider reports misleading zero balances for active subscriptions, set `trust_exhaustion_for_routing = false`.
- UI surfaces expose the last committed balance observation and its freshness. An integrated or loopback-attached TUI with the signed local-operator capability may ask the daemon to refresh observations; a remote observer and the desktop read path remain read-only.
- Balance HTTP calls are bounded and reuse the same outbound client as proxy runtime calls. A failed lookup should surface the probed origin and adapter kind in logs, for example whether `sub2api_usage` or `openai_balance_http_json` returned non-JSON.

The resident proxy runtime owns one quota sampler. It refreshes once on startup and normally schedules another pass about every five minutes with up to 10% positive jitter; provider polling throttles, reset/exhaustion suppression, and `Retry-After` may delay actual HTTP requests. Repeated all-provider failures use bounded exponential backoff. Valid semantic observations are committed to bounded RuntimeStore tables in `~/.codex-helper/state/state.sqlite` and resume across restarts; failures and offline gaps are not interpolated. Observer clients only read the canonical operator model and never start a competing sampler. A loopback-attached TUI may request an immediate daemon-owned refresh through the one-shot signed local-operator API when the daemon advertises that capability; it never performs provider polling itself.

## Usage Page

TUI page 5 is labeled `Usage`. It combines daemon-owned remote quota-window analytics with the existing local-day request view; it is not a durable multi-day analytics warehouse. The Tauri desktop `Usage` page continues to consume the local-day read model, and its recent request rows are drilldown samples rather than the source of truth for totals.

How to read it:

- Remote pool rows make scoped `used` or `observed since <time>`, `remaining`, and state first-viewport signals. The selected pool also shows 15/60-minute burn rates, required rate until reset, faster/on-pace/slower status, exhaustion ETA, reset, source, scope, identity confidence, and freshness. A direct remote total may still be shown when there are too few continuous samples for rates or ETA.
- Only a proven calendar-day window may be called `today` or use a `midnight` reset label. Rolling, custom, monthly, resetless, and reset-unknown counters keep their own window wording; a resetless wallet has an ETA when possible but no required reset pace.
- The daemon-owned background sampler refreshes quota observations. A local attached TUI may request a signed daemon-owned refresh with `g`; remote observers and desktop read clients remain read-only. One provider failure leaves its last committed value visibly offline/stale and does not clear other pools or interrupt redraw.
- The remote pool counter is authoritative for total burn in its declared account/key/subscription scope and can include traffic from other computers. RuntimeStore request facts committed to `state.sqlite` are authoritative for this daemon's project attribution. Reconciliation uses `external = max(remote - local, 0)`, retains a negative signed gap when local exceeds remote, and never multiplies local request prices or distributes external usage across projects.
- Project rows normalize new requests to a Git root when possible, with explicit fallback/unknown and omitted rows. New request costs retain their selected tier and effective pricing source/generation; older reconstructed rows lower coverage instead of being presented as captured billing facts. The local-day provider/endpoint/model/session context and 24-hour activity remain available below the remote quota panels.
- Identity confidence reflects the evidence used to recognize a shared pool. Proven remote ownership is high confidence; an explicit `quota_pool_id` or installation-local keyed credential fingerprint is medium; endpoint-only or conflicting evidence is low/ambiguous. Ambiguous pools remain separate and are not summed into an exact shared total. Credentials and full fingerprints are not exposed.
- Reconciliation requires aligned remote/local windows, USD units, the same conversion generation, and adequate committed-request and price coverage. Raw units, divisor changes, incompatible generations, window mismatch, truncated/reconstructed records, unpriced or unmatched requests, deduplication/boundary uncertainty, and arithmetic overflow keep the available values visible but make the difference unavailable or incomplete. A coverage warning is not a claim that earlier usage was zero.
- `unknown` means there is no trusted remote data or the lookup failed; `stale`, `offline`, `exhausted`, `error`, and `unlimited` are distinct states. Derived rates and predictions freeze or become unavailable when freshness or sample continuity is insufficient.
- In the desktop Usage table, the per-row `Chain` action loads the sanitized request chain only on demand. Use it for single-request diagnosis after the totals show an unusual pattern.
- The `Routing` page keeps compact balance context and route-eligibility controls. `Enter` opens local runtime actions for new-session preference and endpoint Enabled / Draining / Disabled state; these actions never rewrite route configuration or move existing sessions. Use TUI `Usage` for pool burn and pace; use Routing for provider-endpoint eligibility.
- The `Sessions` page owns explicit changes to an existing session binding. Its Clear/Rebind menu is available only through the integrated TUI or a loopback-attached TUI with the signed local-operator capability. The daemon rejects active sessions, stale affinity revisions, stale route graphs, conditional route graphs, unavailable targets, and cross-endpoint Rebind unless both endpoints share the same explicit `continuity_domain`. Clear removes the binding without immediately choosing a replacement; the next eligible request reruns current routing policy, so a state-bound request under `hard` affinity can fail for missing affinity while an ordinary request can establish a new affinity. If an idle session still holds a WebSocket and reselects another endpoint, the old socket returns `websocket_reconnect_required` before writing any application frame to the old upstream. Prefer starting a new Codex session when upstream state ownership is uncertain.

The same daemon-owned DTO is available from the canonical operator read model:

```bash
codex-helper usage quota --target local
codex-helper usage quota --target <RELAY_TARGET> --json
```

`--target` resolves a configured local or remote relay admin endpoint. The command performs a read-only canonical operator-model request; JSON mode serializes the daemon's bounded quota analytics and does not recalculate slopes, reset boundaries, or project reconciliation in the CLI.

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
| `endpoint` | Absolute balance URL or path relative to the normalized provider base URL |
| `token_env` | Environment variable used for adapter auth |
| `require_token_env` | Require `token_env` instead of falling back to the model API key |
| `new_api_user_id_env` | For `new_api_user_self`, environment variable containing the value for the fixed `New-Api-User` header |
| `poll_interval_secs` | Refresh throttle / cache window |
| `refresh_on_request` | Whether routed requests may trigger balance refresh |
| `trust_exhaustion_for_routing` | Whether exhausted snapshots may demote routing |
| `quota_pool_id` | Optional opaque operator label indicating that matching adapter views within the same origin and scope share one remote quota pool; do not put credentials here |
| `quota_reset_timezone` | Optional IANA timezone, such as `Asia/Shanghai`, for a provider-declared calendar-day reset when no absolute timestamp is returned |
| `quota_divisor` | Optional positive New API quota-units-per-USD fallback, used only when `/api/status` does not provide `quota_per_unit` |
| `extract` | JSON path extraction rules for custom balance fields |

## Pricing

Pricing is separate from relay config. BaseLLM is an estimate catalog, not a relay invoice or authoritative billed-usage source:

- Local overrides: `~/.codex-helper/pricing_overrides.toml`
- Automatic remote source: `https://basellm.github.io/llm-metadata/api/all.json`
- Effective precedence: `bundled < validated remote LKG < manual whole-model override`. A manual model row replaces the remote model, including its context tiers; rows stay namespaced by canonical provider.
- The resident daemon checks BaseLLM on startup and about every six hours using conditional requests. A candidate must pass bounded parsing and semantic/economic validation before becoming last-known-good (LKG); failures preserve the prior LKG, while suspicious economic changes are quarantined for explicit approval. LKG, last-check, and quarantine facts are committed through RuntimeStore in `state.sqlite`; there is no separate JSON cache authority. Automatic refresh never writes `pricing_overrides.toml`.
- A 0.20.3 override file with root `[models.*]` rows remains readable. Bare legacy rows are interpreted as `openai` and normalize to the provider-scoped version 2 shape on the first explicit pricing write. If a legacy row was intended for Anthropic/Claude pricing, move it manually under `[providers.anthropic.models.*]`; the old global catalog did not record enough provider identity to infer that safely.
- Operator commands:

```bash
codex-helper pricing status
codex-helper pricing status --json
codex-helper pricing force-refresh
codex-helper pricing force-refresh --approve-economic-changes --json
codex-helper pricing import-basellm --model gpt-5 --dry-run
```

`pricing status` works offline and remains available while the daemon owns the runtime writer; it distinguishes never-synced, fresh, stale, last-error, quarantined, read-only, and corrupt state. It also reports the last check, remote body/content/check generations, the effective revision, and manual shadow/reload status. `pricing force-refresh` validates and refreshes only the remote LKG and requires the resident runtime to be stopped because `state.sqlite` has one writer; while the daemon is running, its startup/six-hour task is the sole BaseLLM refresh owner. `--approve-economic-changes` approves the exact candidate hash from the last quarantine. `pricing import-basellm` is the explicit path that imports selected provider/model rows into manual overrides. `sync-basellm` remains only as a compatibility alias for `import-basellm`.

For BaseLLM context tiers, the threshold input is `ordinary input + cache read`; the 272,000 tier boundary is strict. Exactly 272,000 uses the base row, while 272,001 selects the tier for the whole request, with cache-read tokens counted once. Use manual pricing overrides for known local corrections or relay-specific multipliers, and compare estimated local cost with the remote billed counter instead of treating the estimate as an invoice.

## CLI Editing

Initialize the canonical config:

Normal startup, including the default TUI path, uses the canonical `~/.codex-helper/config.toml` with `version = 5`. When startup finds an older/unversioned TOML, or finds `config.json` while TOML is absent, it performs the validated migration described in [Automatic Configuration Migration](#automatic-configuration-migration). `config init` creates a current template when no helper configuration exists; on a JSON-only installation it migrates that file and prints the report instead. `--force` replaces an existing canonical file only after writing `config.toml.bak`.

Read-only loading may follow a valid `config.toml` symbolic link, but helper commands that rewrite the typed configuration refuse a final-file link so an atomic replacement cannot detach or retarget it. Point `CODEX_HELPER_HOME` (or the whole `.codex-helper` directory) at a stable linked directory instead if the configuration is managed in dotfiles. Mutations are serialized with a helper-owned lock, backups inherit the source file permissions, and a dangling or retargeted configuration directory fails closed.

```bash
codex-helper config init
codex-helper config init --force
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

Manage providers:

```bash
codex-helper provider add input --base-url https://ai.input.im/v1 --auth-token-env INPUT_API_KEY --tag billing=monthly
codex-helper provider add openai --base-url https://api.openai.com/v1 --auth-token-env OPENAI_API_KEY --tag billing=paygo
# Only for a remote relay that intentionally requires no authentication
codex-helper provider add anonymous-relay --base-url https://relay.example/v1 --allow-anonymous
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
codex-helper routing set --policy round-robin --order input,ciii
codex-helper routing clear-target
codex-helper routing show
codex-helper routing explain
```

The CLI preserves existing route graph structure when it only edits the entry node. Advanced nested graph authoring is still best done in TOML until dedicated route-node commands are added.

Use `--claude` on provider/routing commands when editing the Claude service instead of Codex.

`routing show` reads persisted config. `routing list` and `routing explain` read the compiled runtime candidate view.
Use `routing explain --model <MODEL> --json` to inspect the same selected route, candidate order, route paths, and structured skip reasons exposed by the runtime admin explain API.
In that response, `provider_endpoint_key`, `provider_id`, `endpoint_id`, `route_path`, and `preference_group` are the canonical routing identity.

## Inspect Routing And Logs

Use these commands before editing TOML by hand:

```bash
codex-helper routing show
codex-helper routing explain --json
codex-helper routing explain --model <MODEL> --json
```

`routing show` answers "what is saved in config". `routing explain` answers "what the runtime would try now", including candidate order, route paths, and skip reasons such as disabled provider, unsupported model, cooldown, or trusted balance exhaustion.

Provider eligibility is derived from committed provider observations:

- Closed provider adapters normalize observations by endpoint origin, route scope, account fingerprint, config revision, incarnation, and generation.
- Only an authoritative, identity-matched exhausted observation can create an automatic block. HTTP errors, transport failures, parse failures, and passive request health never create or clear quota eligibility.
- Observation history, automatic actions, and the eligibility projection commit atomically to `~/.codex-helper/state/state.sqlite` before the new policy revision appears in routing and `GET /__codex_helper/api/v1/operator/read-model`.
- Manual eligibility remains higher priority than automatic block or recovery.
- codex-helper does not mutate Codex auth files, ChatGPT login state, relay account files, or provider dashboards as an automatic quota response.

The authoritative request and attempt lifecycle is committed to:

```text
~/.codex-helper/state/state.sqlite
```

When a request retries or switches provider, committed attempts retain `provider_id`, `endpoint_id`, `route_path`, `decision`, `status_code`, and `error_class`. Request-ledger reads and usage rollups query those committed facts. `logs/requests.jsonl` is optional post-commit debug output only; failure or rotation cannot affect accounting, and production readers never replay it.

For compact diagnostics, filter by request path:

```bash
codex-helper usage find --path responses/compact --limit 20
```

The read-only operator bundle publishes recent committed requests in `data.recent_requests`. Use `codex-helper usage find` for local filtered searches; the remote control plane does not expose a general ledger-query endpoint.

To inspect one request or session as a route-control timeline, use the request-chain export:

```bash
codex-helper usage chain --trace-id <TRACE_ID> --json
codex-helper usage chain --request-id <REQUEST_ID>
codex-helper usage chain --session <SESSION_ID> --limit 20 --json
```

The same read model is available through the local admin API:

```text
GET /__codex_helper/api/v1/request-ledger/chain?trace_id=<TRACE_ID>
GET /__codex_helper/api/v1/request-ledger/chain?request_id=<REQUEST_ID>
GET /__codex_helper/api/v1/request-ledger/chain?session=<SESSION_ID>&limit=20
```

The request-chain export is an allowlisted diagnostic view. It includes request identity, status, sanitized route attempts, stable provider signal / policy action codes, and timeline events. It intentionally omits sensitive raw fields such as client address, cwd, upstream base URL, provider trace internals, and raw upstream payload details. Large session exports are capped and marked `truncated` instead of streaming the whole local log.

The control trace is enabled by default and is written to:

```text
~/.codex-helper/logs/control_trace.jsonl
```

It records routing selection events such as the compiled route plan, provider endpoint, preference group, skipped higher-priority groups, pinned-route decisions, retry options, and failover reasons. When a lower-priority preference group is selected, the `route_graph_selection_explain` event lists each higher-priority provider endpoint that was skipped and the structured reasons such as `unsupported_model`, `cooldown`, `usage_exhausted`, `runtime_disabled`, or `attempt_avoided`. Set `CODEX_HELPER_CONTROL_TRACE=0` to turn it off, or `CODEX_HELPER_CONTROL_TRACE_PATH` to write it somewhere else.

Request/debug logs and `control_trace.jsonl` share the bounded JSONL retention controlled by `CODEX_HELPER_REQUEST_LOG_MAX_BYTES` and `CODEX_HELPER_REQUEST_LOG_MAX_FILES` (defaults: 50 MiB per active file and 10 rotated files). Oversized active JSONL files rotate on first write, and rotated files are pruned by count and total budget.

Other local helper logs use the same bounded storage primitive with separate knobs:

- `runtime.log`: `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES` (defaults: 20 MiB, 10 files).
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
- Route graph decisions use `provider_endpoint_key`, `provider_id`, `endpoint_id`, and `route_path` as their canonical identity.

For a monthly-first setup, the generated default is `affinity_policy = "fallback-sticky"`, because relay providers often bind cache and encrypted response state to an upstream account. If you prefer automatic return to the best monthly group after an outage, explicitly set `affinity_policy = "preferred-group"`. If the route keeps using paygo unexpectedly, look for one of these causes:

- the monthly provider is disabled or missing auth;
- the requested model is unsupported by the monthly provider;
- the monthly endpoint is cooling down after retryable failures;
- trusted balance data marks the endpoint `usage_exhausted`;
- the config uses `affinity_policy = "fallback-sticky"` or `hard`.

Trusted balance exhaustion is a provider-endpoint runtime signal. It creates an owned balance policy action for the canonical provider endpoint and can demote a monthly endpoint for the current request/refresh window, but it is not a permanent session preference. A fresh non-exhausted balance snapshot clears only the balance action owned by codex-helper; it does not clear manual eligibility or unrelated response-based cooldowns. If every candidate is currently blocked by trusted exhaustion or cooldown, Codex streaming turns receive a retryable `response.failed` SSE with a bounded delay instead of repeatedly hitting depleted upstreams; the helper also queues a throttled balance refresh so recovered relays can re-enter routing. If a provider reports misleading zero balances for an active subscription, set `trust_exhaustion_for_routing = false` for that usage provider or fix the balance extractor.

Use the control trace when a lower-priority group is selected:

```text
~/.codex-helper/logs/control_trace.jsonl
```

Look for `route_graph_selection_explain`. It records the selected provider endpoint, selected preference group, skipped higher-priority groups, and per-candidate skip reasons. Route/provider/endpoint identifiers are the only routing control vocabulary.

## Operator UI

TUI and desktop consume the same typed, redacted `OperatorReadModel`. They use only `GET` / `HEAD` against a remote runtime control plane:

- Provider views show names, aliases, enabled state, tags, committed balance/eligibility facts, expanded fallback order, canonical endpoint keys, and policy provenance.
- Routing views show the compiled entry, candidate order, route paths, skip reasons, continuity, and captured revisions.
- Requests and sessions show provider choice, route affinity, retry chain, token/cache evidence, and committed economics.
- `ready`, `stale`, `disconnected`, and `auth_required` states remain explicit; clients never fabricate a local fallback view.

On page `2 Routing` in an integrated TUI or an authenticated loopback-attached TUI, select a candidate and press `Enter` to open runtime actions. Setting a new-session preference affects only sessions that do not already have affinity; `a`, `Backspace`, or `Delete` restores automatic capacity-aware selection, while `g` forces a full balance/quota refresh. Endpoint draining/disabling is a separate maintenance action. None of these operations rewrites `config.toml` or silently migrates an existing session.

Remote operator clients and the remote control plane are query-only. The local signed operator interface may refresh balances and apply ephemeral routing or idle-session-affinity controls, but it never edits durable provider or routing intent. Edit that intent through local CLI commands or `config.toml`. An attached TUI neither handles `n` / `o` nor inspects or changes local Codex configuration. In terminal workflows, client switching is available only through a separate explicit local `switch on/off` CLI action or `n` / `o` on the integrated local TUI Settings page; neither path is a remote control-plane operation.

## Configuration Compatibility

`version = 5` in `~/.codex-helper/config.toml` remains the only public runtime contract. Older versioned or unversioned TOML and legacy `config.json` are migration inputs rather than long-lived compatibility formats: startup converts them once, creates the source-specific `.bak`, and subsequently loads canonical version 5 TOML. The runtime does not maintain a parallel legacy reader.

Migration preserves representable provider/routing intent, removes known retired settings, warns about known lossy conversions and unrecognized root/service fields retained verbatim, and validates the version 5 result before replacement. Flat `max_attempts`, `backoff_ms`, `backoff_max_ms`, `jitter_ms`, `on_status`, `on_class`, and `strategy` under `[retry]` are moved to `[retry.upstream]` only when that table is absent. If `[retry.upstream]` exists, it is retained as the complete historical override and all flat retry fields are ignored with a warning. Use `config migrate --dry-run` to review that conversion, especially for heavily customized files. A future schema version is never downgraded automatically.

Provider, endpoint, route-graph, retry-profile, notification, fleet, and service-status settings from a normal 0.20.3 version 5 file remain usable. However, several optional fields that were also published under version 5 have been retired. The version number alone cannot identify them, so startup creates `config.toml.bak` and precisely removes all matching helper-config paths before loading. An explicit typed save still refuses to overwrite an uncleaned source directly, preventing unrelated commands from silently erasing fields.

| 0.20.3 input or behavior | Current behavior | Upgrade action |
| --- | --- | --- |
| `[codex.client_patch]` | Startup backs up the file and removes the table; presets, auth facades, compaction, hosted-tool switches, and WebSocket patching are no longer helper config | Preview with `config migrate --dry-run`; use only the explicit URL switch described above |
| `[codex.compaction]` / `[claude.compaction]` | Startup backs up the file and removes either table; the shared v0.20.3 schema accepted the Claude table even though it had no Claude runtime effect | Preview the cleanup; helper no longer performs remote-v2-to-v1 downgrade |
| `[ui.usage_forecast]` | Startup backs up the file and removes the table; the old local forecast was removed | Use committed quota pace and reset-window views instead |
| `codex.profiles.*.station` / `claude.profiles.*.station` | Startup backs up the file and removes every matching profile field | Express provider selection in the service route graph |
| `[retry].allow_cross_station_before_first_output` | Startup backs up the file and removes the retired retry field | Failover is controlled by the canonical route/retry policy |
| `relay_targets.*.client_preset` / `responses_websocket` | Startup backs up the file and removes every matching relay-target field | A relay bookmark stores network/admin connection data only |
| server `advertised-admin-base-url` / `host-local-session-history` and matching CLI flags | Server config parsing rejects these keys; CLI flags no longer exist | Remove them; configure each client's trusted relay `admin_url` explicitly |
| `usage_providers.json` endpoint templates, `headers`, or `variables` | The operator-owned file fails to load | Use literal relative/absolute endpoints and typed fields such as `new_api_user_id_env` |
| Remote `relay_targets.*` without `admin_url`, or with a non-loopback HTTP admin URL / missing token env | The bookmark remains in version 5 but target resolution fails closed; remote admin authority is never derived from `proxy_url` or a response | Set an explicit trusted HTTPS `admin_url` and valid `admin_token_env`, or terminate a trusted tunnel on loopback; this is not auto-rewritten |
| Enabled non-loopback `fleet.nodes.*` using HTTP or missing/invalid `admin_token_env` | Main configuration validation fails closed before the runtime starts | Use HTTPS plus a valid token environment name, or a trusted loopback tunnel; update the file manually |
| One explicit `limit_group` with missing or different `max_concurrent_requests` values | Route-graph compilation and normal version 5 loading fail closed without rewriting the source | Give every candidate in that group the same limit, or split independent 20-slot and 15-slot accounts into different groups |
| Root `[models.*]` rows in `pricing_overrides.toml` | They remain readable as OpenAI rows and normalize to provider-scoped schema v2 on the first explicit pricing write | No action for OpenAI; manually move Anthropic/Claude rows under `[providers.anthropic.models.*]` |

An older route graph without `scheduling_preset` now defaults to `balanced`, which waits up to two seconds for local concurrency capacity. Set `scheduling_preset = "throughput-first"` to retain the 0.20.3 behavior of immediately trying the next candidate when the selected local limit is saturated.

## Design Boundaries

codex-helper intentionally avoids:

- one full Codex config per provider;
- inferring billing class from provider names;
- pretending speed-first or cost-first routing is reliable before real measurements exist;
- keeping `level` as the main user-facing priority control;
- treating balance lookup failure as provider exhaustion;
- silently writing an alternate station-shaped schema from TUI or desktop forms;
- using a special `pool-fallback` syntax when nested route nodes express the same intent more cleanly.

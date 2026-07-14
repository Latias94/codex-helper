# Codex Responses WebSocket Relay — Design

Status: Historical (superseded by the canonical relay runtime on 2026-07-13)
Last updated: 2026-05-19

> This document records the original client-patch WebSocket design. The
> `codex.client_patch.responses_websocket` setting and matching switch option
> were removed. Current WebSocket behavior is selected from canonical
> provider/endpoint capabilities and runtime routing. See
> [Configuration](../../CONFIGURATION.md) and the
> [canonical relay runtime modernization plan](../../plans/2026-07-10-002-refactor-canonical-relay-runtime-modernization-plan.md).

## Problem

`codex-helper` can currently make the local `codex_proxy` look like an official OpenAI Responses
provider for safe HTTP-only paths such as `/responses/compact`. Codex has another official path:
Responses WebSocket v2. Upstreams such as `sub2api` already expose a compatible WebSocket gateway,
but helper still patches `supports_websockets = false` because it does not own a WebSocket upgrade
and relay path.

If helper simply flips `supports_websockets = true`, Codex will connect to
`ws://127.0.0.1:<helper>/responses`, then the current HTTP fallback route will not forward the
upgrade correctly. That would turn an official feature into a flaky hack.

## Goal

Add an explicit, reversible, experimental Responses WebSocket transport switch backed by helper-owned
Responses WebSocket relay behavior. WebSocket is a transport option, not a new patch preset:

- Codex sees `name = "OpenAI"`, `wire_api = "responses"`, and `supports_websockets = true`.
- Helper accepts `GET /responses`, `GET /v1/responses`, and
  `GET /backend-api/codex/responses` WebSocket upgrades.
- Helper selects the upstream from existing routing configuration, strips unsafe Codex client auth
  when bridge preset requires it, injects helper upstream credentials, and relays frames
  bidirectionally.
- Existing default, ChatGPT bridge, imagegen bridge, and official relay presets remain unchanged unless
  `responses_websocket` is explicitly enabled.

## Non-goals

- Do not spoof official `encrypted_content` or synthesize compaction output locally.
- Do not make WebSocket support implicit for existing `official-relay` or
  `official-imagegen`; require the separate `responses_websocket` option.
- Do not bypass helper routing by requiring Codex to point directly at `sub2api`.
- Do not claim all relays support WebSocket; this is opt-in and diagnosable.

## Source findings

### Codex upstream behavior

- Codex enables Responses WebSocket only when the selected model provider advertises
  `supports_websockets`.
- It sends `OpenAI-Beta: responses_websockets=2026-02-06`.
- It uses `response.create` frames and may send `response.processed` frames.
- It falls back only for certain failures; helper should not rely on fallback as the primary path.

### sub2api behavior

`repo-ref/sub2api` supports Responses WebSocket v2:

- Routes: `GET /v1/responses`, `GET /responses`, `GET /backend-api/codex/responses`.
- Requires/configures `gateway.openai_ws.enabled` and
  `gateway.openai_ws.responses_websockets_v2`.
- Uses the same beta header value: `responses_websockets=2026-02-06`.
- Provides passthrough-style frame relay in the v2 path.

### helper current state

- `CodexPatchMode::{OfficialRelayBridge, OfficialImagegenBridge}` deliberately write
  `supports_websockets = false`.
- `proxy::router_setup` has ordinary HTTP fallback routes only.
- `axum` already has the `ws` feature, and `tokio-tungstenite` is present transitively through
  axum, but helper must depend on it explicitly before using it as an upstream WebSocket client.

## Architecture

### Patch preset + transport switch

Keep user-facing client patch presets focused on auth/provider identity only:

- `default`
- `chatgpt-bridge`
- `imagegen-bridge`
- `official-relay`
- `official-imagegen`

Add an orthogonal option, exposed as `codex.client_patch.responses_websocket = true` and
`codex-helper switch on --responses-websocket`. When enabled with an official bridge preset, helper
writes `supports_websockets = true`; otherwise official bridge presets keep `supports_websockets = false`.

### Relay path

1. HTTP router matches the three Responses WebSocket paths before the wildcard HTTP proxy route.
2. Handler accepts only WebSocket upgrades. Non-upgrade requests still fall through to the existing
   HTTP proxy behavior.
3. After upgrade, helper reads the first client frame before dialing upstream.
4. The first frame is parsed enough to extract `model` from `response.create`.
5. Helper builds the same routing context as an HTTP `/responses` request:
   - method `GET`
   - request path
   - handshake headers
   - extracted model
   - session id / client identity
6. Helper selects a single upstream using existing routing and model support checks.
7. Helper applies selected upstream model mapping and request filtering to `response.create` frames.
   `response.processed` and non-JSON frames pass through.
8. Helper builds `ws://` or `wss://` upstream URL from the selected upstream base URL and incoming
   path, using the same base-path de-duplication rule as HTTP target building.
9. Helper builds handshake headers from filtered client headers, strips WebSocket hop-by-hop and
   client account auth as needed, injects upstream auth, and ensures the Responses WebSocket beta
   header exists.
10. Helper connects upstream and relays frames bidirectionally until either side closes.

### Failover policy

First slice keeps failover conservative:

- Before an upstream WebSocket connection succeeds, helper may select the next routable target.
- After a WebSocket connection succeeds, the connection is sticky to that target until closed.
- Mid-connection failover is a follow-on because replaying an incremental WebSocket conversation is
  unsafe without official resume semantics.

### Diagnostics

Request logs should show:

- method/path/status/duration as usual,
- selected station/provider/upstream,
- patch preset and `responses_websocket_request = true` bridge metadata,
- route attempts for selected/failed WebSocket targets where practical.

## Risks

- WebSocket frames are long-lived; usage accounting may be incomplete until frame-level usage
  parsing is added.
- Some relays may implement WebSocket only for `ctx_pool`/account-bound modes; operators still need
  relay-side config.
- TLS support for direct `wss://` relay targets requires enabling TLS features on
  `tokio-tungstenite`.

## Recommended release posture

Ship behind explicit presets only. Keep HTTP-only official presets as the recommended stable path unless
the relay is known to support Responses WebSocket v2.

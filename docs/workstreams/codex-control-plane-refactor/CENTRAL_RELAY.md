# Central Relay Mode: LAN / Tailscale Deployment

> 中文速览：这份文档说明 `codex-helper` 作为“中心中转 + 控制平面”在局域网或 Tailscale 网络中的正确产品形态。重点不是远程附着宿主机桌面，而是让多台设备共享同一个 relay / station / profile 控制面，同时明确哪些能力天然可共享，哪些仍然是宿主机本地能力。

## Goal

`codex-helper` central relay mode is intended for:

- one always-on host running the relay and admin API
- multiple personal devices on LAN / Tailscale sending Codex traffic through that host
- one shared control plane for:
  - station switching
  - profiles
  - session-scoped overrides
  - health / breaker visibility

It is **not** intended to pretend every remote device can read the host's local session files.

## Product Shape

In this mode, the central host owns two surfaces:

- proxy traffic surface
- admin/control-plane surface

Recommended mental model:

- the proxy surface is what Codex clients talk to
- the admin surface is what GUI / future WebUI / operator tools talk to

The current split is:

- proxy port: normal relay traffic
- admin port: proxy port + `1000`

Example:

- proxy: `http://relay-host:4141`
- admin: `http://relay-host:5141`

## What Remote Devices Can Reliably Use

These capabilities are safe to treat as shared:

- station list and runtime state
- profile list and default profile switching
- session identity cards from observed traffic
- per-session overrides
- request history observed by the relay
- healthcheck / probe / breaker visibility

These capabilities work because the relay can observe or control them directly.

## What Stays Host-local

These capabilities remain host-local unless a future companion mode is added:

- direct browsing of `~/.codex/sessions`
- transcript file opening
- host filesystem path opening
- automatic enrichment from session files that only exist on another device

This means:

- a remote device can still see the observed session and effective route
- a remote device must not assume transcript/history file access exists

## Session Identity Expectations

For meaningful multi-device control, the relay should distinguish:

- `session_id`
- device/client identity
- observed effective route
- optional host-local enrichment

Operationally:

- session control is only meaningful when the client sends a stable `session_id`
- device attribution helps answer which machine produced the traffic
- host-local enrichment should be shown as optional, never guaranteed

## Security Boundary

Current remote admin model:

- loopback access does not require a token
- non-loopback admin access requires `CODEX_HELPER_ADMIN_TOKEN`
- remote admin clients must send header `x-codex-helper-admin-token`

Recommended deployment rule:

- do not expose admin routes broadly without the token
- prefer Tailscale / trusted LAN over public internet exposure

## Recommended Deployment Steps

1. Run `codex-helper` on the central host.
2. Choose the relay host as the shared station/profile control point.
3. Set `CODEX_HELPER_ADMIN_TOKEN` on the central host if non-loopback admin access is needed.
4. Point remote GUI / future tools at the admin base URL.
5. Point Codex clients on each device at the proxy base URL.
6. Verify `/__codex_helper/api/v1/capabilities` before enabling richer remote controls.

## Capability Truthfulness

Remote clients should trust the capability response.

If the relay reports a local-only feature as unavailable, the UI should:

- keep the page usable
- explain why the feature is unavailable remotely
- avoid presenting host-local actions as if they can succeed

This is required to keep the product honest in shared-relay mode.

## Operational Limitations

Current limitations to treat as expected:

- relay host failure affects all connected devices
- remote devices cannot upload host-local transcript data automatically
- session identity is only as good as the upstream client/session headers being observed
- sticky session continuity still matters more than aggressive cross-station failover

## Recommended Operator Checklist

- enable only the stations you are willing to route traffic to
- define a small profile set such as `daily`, `fast`, `deep`
- keep remote admin token configured before enabling non-loopback control
- validate session identity cards from at least two devices
- confirm local-only capability gating appears correctly on remote clients
- test healthcheck / probe / breaker visibility before daily use

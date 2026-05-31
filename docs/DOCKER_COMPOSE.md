# Docker Compose Deployment

This deployment runs `codex-helper-server`, a container-first central relay runtime. It starts the proxy and admin API only; it does not patch the host machine's `~/.codex/config.toml` or `~/.codex/auth.json`.

## Files

- `Dockerfile` builds the `codex-helper-server` binary with `cargo-chef`.
- `deploy/compose/codex-helper.yml` is the Synology-friendly Compose stack.
- `deploy/container/server.toml` controls server process settings: service, proxy bind, admin bind, and host-local session history policy.
- `deploy/container/config.toml` is the initial codex-helper route graph copied into the `/data` volume on first start.

## First Start

Create an environment file from `deploy/compose/.env.example`, set a long `CODEX_HELPER_ADMIN_TOKEN`, and provide the provider credentials referenced by `deploy/container/config.toml`.

```bash
docker compose --env-file deploy/compose/.env -f deploy/compose/codex-helper.yml up -d --build
```

The first start copies `/config/config.toml` into `/data/config.toml`. Later starts keep the existing `/data/config.toml`, so container updates do not overwrite runtime routing changes.

## Client Configuration

On each client machine that should use the NAS relay, prefer saving a relay target:

```bash
export CODEX_HELPER_NAS_ADMIN_TOKEN='same-long-token-as-compose'

ch relay add nas \
  --proxy-url http://NAS_IP_OR_TAILSCALE_IP:3211 \
  --admin-url http://NAS_IP_OR_TAILSCALE_IP:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN \
  --preset official-relay

ch relay nas
```

`ch relay nas` patches that client machine's Codex config to the NAS proxy and opens a local attached TUI backed by the NAS admin API. Use `ch relay nas --no-tui` when you only want to switch Codex, or `ch relay nas --attach-only` when you only want to inspect the NAS runtime. Use `ch relay off` to restore the local Codex/Claude client patch.

Manual Codex config is still possible, but it does not give you target status or attached TUI:

```toml
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://NAS_IP_OR_TAILSCALE_IP:3211"
wire_api = "responses"
```

Do not run `codex-helper switch on` inside the container. That command is for local desktop/client machines and writes local Codex files.

## Admin API

The Compose sample binds the admin API inside the container to `0.0.0.0:4211`, but publishes it on the NAS host as `127.0.0.1:4211` by default. Remote admin requests require the `x-codex-helper-admin-token` header with the `CODEX_HELPER_ADMIN_TOKEN` value.

Expose the admin port to LAN or Tailscale only when the host firewall and token policy are intentional.

If proxy clients should discover the admin API from `/.well-known/codex-helper-admin`, set `advertised-admin-base-url` in `deploy/container/server.toml` to the LAN or Tailscale URL clients can actually reach.

## Host-Local Capabilities

`host-local-session-history = false` is the default container policy. The server can still report shared proxy state such as recent requests and session identity observed through traffic, but it should not claim access to each remote client's local Codex transcript files.

# Docker Compose Deployment

This deployment runs `codex-helper-server`, a container-first central relay runtime. It starts the proxy and admin API only; it does not patch the host machine's `~/.codex/config.toml` or `~/.codex/auth.json`.

## Files

- `Dockerfile` builds the `codex-helper-server` binary with `cargo-chef`.
- `deploy/compose/codex-helper.yml` is the Synology-friendly Compose stack.
- `deploy/container/server.toml` controls server process settings: service, proxy bind, and admin bind.
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
  --admin-url https://NAS_ADMIN_HOST \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN

ch relay nas
```

`ch relay nas` only starts or attaches to the target runtime and opens a local read-only TUI backed by the NAS admin API; it never changes that client machine's Codex configuration. Use `ch relay nas --no-tui` to omit the console, or `ch relay nas --attach-only` to require an already-running runtime. Point Codex at the NAS proxy explicitly, and restore the recorded local configuration explicitly:

```bash
codex-helper switch on --base-url http://NAS_IP_OR_TAILSCALE_IP:3211
codex-helper switch off
```

`ch relay off` does not restore Codex configuration; it only reports the explicit switch command to run.

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

Do not expose the raw HTTP admin listener as a remote client URL. Put it behind an HTTPS reverse proxy, or tunnel it to a loopback port on each client. Remote access still requires the admin token.

Configure each remote relay target with an explicit trusted `--admin-url`. Proxy responses and redirects are never used to replace that authority; use an HTTPS reverse proxy or an explicit loopback tunnel URL.

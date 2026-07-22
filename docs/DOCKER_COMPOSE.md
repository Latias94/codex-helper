# Docker Compose Deployment

This deployment runs `codex-helper-server`, a container-first central relay runtime. It starts the proxy and admin API only; it does not patch the host machine's `~/.codex/config.toml` or `~/.codex/auth.json`.

## Files

- `Dockerfile` builds the `codex-helper-server` binary with `cargo-chef`.
- `deploy/compose/codex-helper.yml` is the Synology-friendly Compose stack.
- `deploy/compose/codex-helper.secrets.yml` is the opt-in mounted-secret overlay.
- `deploy/container/server.toml` controls server process settings: service, proxy bind, and admin bind.
- `deploy/container/config.toml` is the initial codex-helper route graph copied into the `/data` volume on first start.
- `deploy/container/config.secrets.toml` is the version 6 route graph used by the mounted-secret overlay.

## Environment Deployment

Create an environment file from `deploy/compose/.env.example`, set a long `CODEX_HELPER_ADMIN_TOKEN`, and provide the provider credentials referenced by `deploy/container/config.toml`.

```bash
docker compose --env-file deploy/compose/.env \
  -f deploy/compose/codex-helper.yml \
  pull codex-helper
docker compose --env-file deploy/compose/.env \
  -f deploy/compose/codex-helper.yml \
  up -d --no-build
```

The example pins `CODEX_HELPER_VERSION` to a release instead of tracking `latest`. Change that value deliberately when upgrading. The commands above use the prebuilt multi-architecture GHCR image and do not compile Rust on the deployment host.

The first start copies `/config/config.toml` into `/data/config.toml`. Later starts keep the existing `/data/config.toml`, so container updates do not overwrite runtime routing changes.

The environment sample intentionally starts from a version 5 config to exercise the supported automatic migration. The first config load writes a version 6 `/data/config.toml` and preserves the exact source as `/data/config.toml.bak`. It does not move or copy the environment credential. `OPENAI_API_KEY` must be non-empty; Compose leaves it optional at interpolation time so the same base file can be used with the mounted-secret overlay, while `--check` rejects an omitted value as blocked.

Before exposing ports, run the offline credential check through the same Compose configuration:

```bash
docker compose --env-file deploy/compose/.env \
  -f deploy/compose/codex-helper.yml \
  run --rm --no-deps --entrypoint sh codex-helper \
  -ceu 'test -f /data/config.toml || cp /config/config.toml /data/config.toml; exec codex-helper-server --config /config/server.toml --check --json'
```

The check loads and compiles routing and resolves only environment or mounted-file credentials. It opens no runtime database or listener and sends no client or upstream request. `ready` and `degraded` exit with status 0; `blocked` prints its redacted report and exits with status 1.

## Mounted-Secret Deployment

Local Compose `file:` secrets are read-only bind mounts. They are not encrypted host storage, and Compose ownership/mode overrides are not portable. Protect the host source yourself. On a conventional Linux host, the tested ownership for the image's effective UID/GID 10001 is:

```bash
sudo install -d -o root -g 10001 -m 0750 /srv/codex-helper-secrets
read -rsp 'OpenAI API key: ' OPENAI_API_KEY; printf '\n'
printf '%s\n' "$OPENAI_API_KEY" | sudo tee /srv/codex-helper-secrets/openai_api_key.new >/dev/null
unset OPENAI_API_KEY
sudo chown root:10001 /srv/codex-helper-secrets/openai_api_key.new
sudo chmod 0440 /srv/codex-helper-secrets/openai_api_key.new
sudo mv /srv/codex-helper-secrets/openai_api_key.new /srv/codex-helper-secrets/openai_api_key
```

Do not put a literal token in the Compose file, command line, or `.env`. Set only the absolute host path and the separate admin token:

```bash
export CODEX_HELPER_OPENAI_SECRET_FILE=/srv/codex-helper-secrets/openai_api_key
export CODEX_HELPER_ADMIN_TOKEN='long-random-admin-token'

docker compose \
  -f deploy/compose/codex-helper.yml \
  -f deploy/compose/codex-helper.secrets.yml \
  run --rm --no-deps --entrypoint sh codex-helper \
  -ceu 'test -f /data/config.toml || cp /config/config.toml /data/config.toml; exec codex-helper-server --config /config/server.toml --check --json'

docker compose \
  -f deploy/compose/codex-helper.yml \
  -f deploy/compose/codex-helper.secrets.yml \
  pull codex-helper
docker compose \
  -f deploy/compose/codex-helper.yml \
  -f deploy/compose/codex-helper.secrets.yml \
  up -d --no-build
```

The overlay mounts the source only at `/run/secrets/openai_api_key`, clears `OPENAI_API_KEY`, and uses a distinct `/data` volume so enabling it cannot overwrite an existing environment deployment's config. The secret value is read directly from the mount. It is never copied into `/data`, config, SQLite/WAL/SHM, logs, or operator JSON. One terminal LF or CRLF is accepted; empty, over-64-KiB, non-regular, unreadable, or malformed values are rejected with a stable category and the configured path only.

Verify the effective identity and mount after start:

```bash
docker compose \
  -f deploy/compose/codex-helper.yml \
  -f deploy/compose/codex-helper.secrets.yml \
  exec codex-helper sh -ceu '
    test "$(id -u)" = 10001
    test "$(id -g)" = 10001
    test -r /run/secrets/openai_api_key
    test ! -w /run/secrets/openai_api_key
  '
```

Docker Desktop and NAS ACL implementations can map host ownership differently. The check above is authoritative for that host; do not relax the file to world-readable merely to make a mount work.

### Rotation

Write a replacement next to the source, apply the same ownership/mode, and rename it atomically over the configured path. A file bind mount pins the old inode in the running container. The tested behavior is:

- runtime reload does not remount the host path;
- `docker compose restart` is the first tested operation that rebuilds the mount namespace, resolves the replacement path, and publishes the new credential generation;
- container recreation also exposes the replacement, but is not required by the tested Docker Engine path.

Use:

```bash
docker compose \
  -f deploy/compose/codex-helper.yml \
  -f deploy/compose/codex-helper.secrets.yml \
  restart codex-helper
```

Then rerun `--check`. Do not claim a rotation completed from process state alone; require a ready check after restart and verify the upstream accepts the new credential. The repository's `tools/docker-mounted-secret-smoke.sh` reproduces the ACL, invalid-input, reload/restart/recreate, operator, persistence, and canary-leakage gates on a real Linux Docker Engine.

Native credential references are deliberately unsupported by `codex-helper-server`, including builds where Cargo feature unification compiled a desktop native backend into another workspace package. Use `*_env` or an absolute `secret_file` reference for server deployments.

## Source Development

Use a local image build only when testing changes from this checkout:

```bash
docker compose --env-file deploy/compose/.env \
  -f deploy/compose/codex-helper.yml \
  up -d --build
```

This path runs `cargo-chef` and compiles the workspace. It is intentionally separate from the prebuilt-image production workflow above.

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

`ch relay nas` applies the client machine's journaled Codex switch to the saved `proxy_url`, then opens a local read-only TUI backed by the NAS admin API. Use `ch relay nas --no-tui` to perform only the client switch, or `ch relay nas --attach-only` to inspect the runtime without changing Codex. The equivalent long-form switch and the restore command are:

```bash
codex-helper switch on --base-url http://NAS_IP_OR_TAILSCALE_IP:3211
codex-helper switch off
```

`ch relay off` runs the same journaled restore as `codex-helper switch off`. Neither command runs inside the container; both operate on the client machine's Codex files.

Manual Codex config is still possible, but it does not give you target status or attached TUI:

```toml
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://NAS_IP_OR_TAILSCALE_IP:3211"
wire_api = "responses"
```

Do not run `codex-helper switch on` inside the container. That command is for local desktop/client machines and writes local Codex files.

The server image receives no client `~/.codex`, `~/.claude`, session, or local runtime mount. A remote client uses the hosted relay over its explicit proxy/admin URLs; the server cannot inspect or mutate that client's files.

## Admin API

The Compose sample binds the admin API inside the container to `0.0.0.0:4211`, but publishes it on the NAS host as `127.0.0.1:4211` by default. Remote admin requests require the `x-codex-helper-admin-token` header with the `CODEX_HELPER_ADMIN_TOKEN` value.

Do not expose the raw HTTP admin listener as a remote client URL. Put it behind an HTTPS reverse proxy, or tunnel it to a loopback port on each client. Remote access still requires the admin token.

Configure each remote relay target with an explicit trusted `--admin-url`. Proxy responses and redirects are never used to replace that authority; use an HTTPS reverse proxy or an explicit loopback tunnel URL.

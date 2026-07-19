#!/usr/bin/env bash
set -euo pipefail

image="${1:-codex-helper-server:smoke}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work_dir="$(mktemp -d)"
container="codex-helper-secret-smoke-$$"
compose_project="codex-helper-secret-smoke-$$"
admin_token="docker-smoke-admin-token"
use_sudo=false
upstream_pid=""

if [[ "$(uname -s)" == "Linux" && "$(id -u)" != "0" ]]; then
  use_sudo=true
fi

run_root() {
  if [[ "$use_sudo" == "true" ]]; then
    sudo -n "$@"
  else
    "$@"
  fi
}

cleanup() {
  if [[ -n "$upstream_pid" ]]; then
    kill "$upstream_pid" >/dev/null 2>&1 || true
    wait "$upstream_pid" >/dev/null 2>&1 || true
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  CODEX_HELPER_ADMIN_TOKEN="$admin_token" \
  CODEX_HELPER_OPENAI_SECRET_FILE="${secret_file:-/dev/null}" \
  CODEX_HELPER_IMAGE="${image%:*}" \
  CODEX_HELPER_VERSION="${image##*:}" \
    docker compose \
      -p "$compose_project" \
      -f "$repo_root/deploy/compose/codex-helper.yml" \
      -f "$repo_root/deploy/compose/codex-helper.secrets.yml" \
      down --volumes --remove-orphans >/dev/null 2>&1 || true
  run_root rm -rf "$work_dir"
}
trap cleanup EXIT

fail() {
  echo "docker mounted-secret smoke failed: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local pattern="$2"
  grep -F -- "$pattern" "$file" >/dev/null || fail "$file does not contain $pattern"
}

assert_no_canary_file() {
  local file="$1"
  [[ ! -f "$file" ]] && return
  if grep -a -F -f "$work_dir/canary-patterns" "$file" >/dev/null; then
    fail "provider credential leaked into $file"
  fi
}

assert_no_canary_tree() {
  local directory="$1"
  [[ ! -d "$directory" ]] && return
  if run_root grep -R -a -F -f "$work_dir/canary-patterns" "$directory" >/dev/null; then
    fail "provider credential leaked into $directory"
  fi
}

prepare_data_dir() {
  local directory="$1"
  mkdir -p "$directory"
  if [[ "$(uname -s)" == "Linux" ]]; then
    run_root chown -R 10001:10001 "$directory"
  else
    chmod -R u+rwX "$directory"
  fi
}

prepare_secret_file() {
  local file="$1"
  if [[ "$(uname -s)" == "Linux" ]]; then
    run_root chown 0:10001 "$file"
    run_root chmod 0440 "$file"
    [[ "$(run_root stat -c '%u:%g:%a' "$file")" == "0:10001:440" ]] \
      || fail "secret file ACL does not match root:10001 mode 0440"
  else
    chmod 0444 "$file"
  fi
}

write_secret_file() {
  local file="$1"
  run_root tee "$file" >/dev/null
  prepare_secret_file "$file"
}

prepare_secret_directory() {
  local directory="$1"
  mkdir -p "$directory"
  if [[ "$(uname -s)" == "Linux" ]]; then
    run_root chown 0:10001 "$directory"
    run_root chmod 0750 "$directory"
    [[ "$(run_root stat -c '%u:%g:%a' "$directory")" == "0:10001:750" ]] \
      || fail "secret directory ACL does not match root:10001 mode 0750"
  else
    chmod 0755 "$directory"
  fi
}

host_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    run_root sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

docker_check() {
  local data_dir="$1"
  local secret_source="$2"
  local stdout_file="$3"
  local stderr_file="$4"
  docker run --rm \
    --mount "type=bind,src=$data_dir,dst=/data" \
    --mount "type=bind,src=$secret_source,dst=/run/secrets/openai_api_key,readonly" \
    "$image" --check --json >"$stdout_file" 2>"$stderr_file"
}

expect_blocked_check() {
  local data_dir="$1"
  local secret_source="$2"
  local label="$3"
  local stdout_file="$work_dir/$label.json"
  local stderr_file="$work_dir/$label.stderr"
  local status

  set +e
  docker_check "$data_dir" "$secret_source" "$stdout_file" "$stderr_file"
  status=$?
  set -e
  [[ "$status" == "1" ]] || fail "$label check exited with $status instead of 1"
  assert_contains "$stdout_file" '"aggregate": "blocked"'
  assert_contains "$stdout_file" '"code": "invalid"'
  assert_contains "$stdout_file" '"reference": "/run/secrets/openai_api_key"'
}

wait_for_operator() {
  local output="$1"
  for _ in $(seq 1 40); do
    if docker exec "$container" curl -fsS \
      -H "x-codex-helper-admin-token: $admin_token" \
      http://127.0.0.1:4211/__codex_helper/api/v1/operator/read-model \
      >"$output" 2>/dev/null; then
      return
    fi
    sleep 0.5
  done
  docker logs "$container" >&2 || true
  fail "operator API did not become ready"
}

start_live_container() {
  local data_dir="$1"
  local secret_file="$2"
  docker run -d --name "$container" \
    -e "CODEX_HELPER_ADMIN_TOKEN=$admin_token" \
    --add-host host.docker.internal:host-gateway \
    --publish 127.0.0.1::3211 \
    --mount "type=bind,src=$data_dir,dst=/data" \
    --mount "type=bind,src=$secret_file,dst=/run/secrets/openai_api_key,readonly" \
    --mount "type=bind,src=$repo_root/deploy/container/server.toml,dst=/config/server.toml,readonly" \
    "$image" --config /config/server.toml >/dev/null
}

start_smoke_upstream() {
  local credential_fixture="$work_dir/upstream-credentials.json"
  local ready_file="$work_dir/upstream-ready.json"
  printf '{"old":"%s","new":"%s"}\n' "$old_canary" "$new_canary" \
    >"$credential_fixture"
  chmod 0600 "$credential_fixture"
  node "$repo_root/tools/docker-smoke-upstream.mjs" \
    --credentials "$credential_fixture" \
    --records "$work_dir/upstream-records.jsonl" \
    --ready "$ready_file" \
    >"$work_dir/upstream.stdout" \
    2>"$work_dir/upstream.stderr" &
  upstream_pid=$!
  for _ in $(seq 1 40); do
    [[ -f "$ready_file" ]] && break
    if ! kill -0 "$upstream_pid" 2>/dev/null; then
      fail "credential-capture upstream exited before becoming ready"
    fi
    sleep 0.25
  done
  [[ -f "$ready_file" ]] || fail "credential-capture upstream did not become ready"
  upstream_port="$(node -e '
    const fs = require("node:fs");
    const value = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    if (!Number.isInteger(value.port)) process.exit(1);
    process.stdout.write(String(value.port));
  ' "$ready_file")"
}

relay_endpoint() {
  local published
  published="$(docker port "$container" 3211/tcp | head -n 1)"
  [[ "$published" == 127.0.0.1:* ]] || fail "relay port is not loopback-published"
  printf '%s' "$published"
}

expect_relay_generation() {
  local phase="$1"
  local expected="$2"
  local response="$work_dir/relay-$phase.json"
  local probe_id="$container-$phase"
  local before=0
  [[ -f "$work_dir/upstream-records.jsonl" ]] \
    && before="$(wc -l <"$work_dir/upstream-records.jsonl" | tr -d ' ')"
  curl --connect-timeout 5 --max-time 30 -fsS \
    -H 'content-type: application/json' \
    -H "session_id: docker-smoke-$phase" \
    -H "x-codex-helper-smoke-probe: $probe_id" \
    --data '{"model":"gpt-5","input":"credential generation probe","stream":false}' \
    "http://$(relay_endpoint)/v1/responses" >"$response"
  node -e '
    const fs = require("node:fs");
    const records = fs.readFileSync(process.argv[1], "utf8")
      .trim()
      .split(/\r?\n/)
      .filter(Boolean)
      .slice(Number.parseInt(process.argv[2], 10))
      .map((line) => JSON.parse(line));
    const record = records.find((item) => item.probe_id === process.argv[3]);
    if (
      !record ||
      record.generation !== process.argv[4] ||
      !record.path.split("?", 1)[0].endsWith("/responses")
    ) {
      process.exit(1);
    }
  ' "$work_dir/upstream-records.jsonl" "$before" "$probe_id" "$expected" \
    || fail "$phase relay request did not use the expected $expected credential generation"
}

compose_secret() {
  CODEX_HELPER_ADMIN_TOKEN="$admin_token" \
  CODEX_HELPER_OPENAI_SECRET_FILE="$secret_file" \
  CODEX_HELPER_IMAGE="${image%:*}" \
  CODEX_HELPER_VERSION="${image##*:}" \
    docker compose \
      -p "$compose_project" \
      -f "$repo_root/deploy/compose/codex-helper.yml" \
      -f "$repo_root/deploy/compose/codex-helper.secrets.yml" \
      "$@"
}

old_canary="codex-helper-mounted-old-$(openssl rand -hex 32)"
new_canary="codex-helper-mounted-new-$(openssl rand -hex 32)"
environment_canary="codex-helper-environment-$(openssl rand -hex 32)"
printf '%s\n%s\n%s\n' \
  "$old_canary" \
  "$new_canary" \
  "$environment_canary" \
  >"$work_dir/canary-patterns"
chmod 0600 "$work_dir/canary-patterns"

# The original environment example remains valid and migrates its v5 config in place.
env_data="$work_dir/env-data"
mkdir -p "$env_data"
cp "$repo_root/deploy/container/config.toml" "$env_data/config.toml"
prepare_data_dir "$env_data"
printf 'OPENAI_API_KEY=%s\n' "$environment_canary" >"$work_dir/environment.env"
chmod 0600 "$work_dir/environment.env"
docker run --rm \
  --env-file "$work_dir/environment.env" \
  --mount "type=bind,src=$env_data,dst=/data" \
  "$image" --check --json >"$work_dir/environment-check.json"
assert_contains "$work_dir/environment-check.json" '"aggregate": "ready"'
[[ ! -e "$env_data/state" ]] || fail "environment check created runtime state"
run_root grep -F 'version = 6' "$env_data/config.toml" >/dev/null \
  || fail "environment config did not migrate to version 6"
run_root grep -F 'version = 5' "$env_data/config.toml.bak" >/dev/null \
  || fail "environment config migration did not preserve the version 5 backup"

# The overlay has no provider credential in its environment and mounts one server-only secret.
secret_dir="$work_dir/secrets"
prepare_secret_directory "$secret_dir"
secret_file="$secret_dir/openai_api_key"
printf '%s\n' "$old_canary" | write_secret_file "$secret_file"
compose_secret config >"$work_dir/compose.json"
assert_contains "$work_dir/compose.json" 'OPENAI_API_KEY: ""'
assert_contains "$work_dir/compose.json" 'target: openai_api_key'
assert_no_canary_file "$work_dir/compose.json"
# Command substitutions are evaluated by the container shell.
# shellcheck disable=SC2016
compose_secret run --rm --no-deps --entrypoint sh codex-helper \
  -ceu '
    test "$(id -u)" = 10001
    test "$(id -g)" = 10001
    test -r /run/secrets/openai_api_key
    test ! -w /run/secrets/openai_api_key
    ! (printf x >>/run/secrets/openai_api_key) 2>/dev/null
    test -f /data/config.toml || cp /config/config.toml /data/config.toml
    codex-helper-server --config /config/server.toml --check --json
    test ! -e /data/state
  ' >"$work_dir/compose-check.json"
assert_contains "$work_dir/compose-check.json" '"aggregate": "ready"'
assert_no_canary_file "$work_dir/compose-check.json"

secret_data="$work_dir/secret-data"
mkdir -p "$secret_data"
start_smoke_upstream
sed "s#https://api.openai.com/v1#http://host.docker.internal:${upstream_port}/v1#" \
  "$repo_root/deploy/container/config.secrets.toml" >"$secret_data/config.toml"
prepare_data_dir "$secret_data"
docker_check \
  "$secret_data" \
  "$secret_file" \
  "$work_dir/secret-check.json" \
  "$work_dir/secret-check.stderr"
assert_contains "$work_dir/secret-check.json" '"aggregate": "ready"'
[[ ! -e "$secret_data/state" ]] || fail "mounted-secret check created runtime state"

# Invalid mounted inputs fail with stable categories and no raw value.
empty_secret="$secret_dir/empty"
: | write_secret_file "$empty_secret"
expect_blocked_check "$secret_data" "$empty_secret" empty-secret

oversize_secret="$secret_dir/oversize"
head -c 65537 /dev/zero | tr '\0' x | write_secret_file "$oversize_secret"
expect_blocked_check "$secret_data" "$oversize_secret" oversize-secret

nonregular_secret="$secret_dir/nonregular"
run_root mkdir "$nonregular_secret"
expect_blocked_check "$secret_data" "$nonregular_secret" nonregular-secret

# Start the actual runtime as image UID/GID 10001 and exercise the authenticated operator API.
start_live_container "$secret_data" "$secret_file"
wait_for_operator "$work_dir/operator-old.json"
expect_relay_generation initial old
[[ "$(docker exec "$container" id -u)" == "10001" ]] || fail "container UID is not 10001"
[[ "$(docker exec "$container" id -g)" == "10001" ]] || fail "container GID is not 10001"
docker exec "$container" sh -c \
  'test -r /run/secrets/openai_api_key && test ! -w /run/secrets/openai_api_key && ! (printf x >>/run/secrets/openai_api_key) 2>/dev/null' \
  || fail "mounted secret is not readable and read-only for UID/GID 10001"
docker inspect "$container" >"$work_dir/inspect-old.json"
docker logs "$container" >"$work_dir/runtime-old.log" 2>&1
old_hash="$(host_sha256 "$secret_file")"
inside_hash="$(docker exec "$container" sha256sum /run/secrets/openai_api_key | awk '{print $1}')"
[[ "$inside_hash" == "$old_hash" ]] || fail "initial mounted inode does not contain the expected generation"

# Atomic file replacement pins the old inode in the running mount namespace. An in-process
# reload cannot remount it; an ordinary container restart is the first tested operation that
# resolves the source path again and exposes the new generation.
replacement="$secret_dir/.openai_api_key.new"
printf '%s\n' "$new_canary" | write_secret_file "$replacement"
run_root mv "$replacement" "$secret_file"
new_hash="$(host_sha256 "$secret_file")"
[[ "$new_hash" != "$old_hash" ]] || fail "replacement did not change the secret generation"
if [[ "$(uname -s)" == "Linux" ]]; then
  inside_hash="$(docker exec "$container" sha256sum /run/secrets/openai_api_key | awk '{print $1}')"
  [[ "$inside_hash" == "$old_hash" ]] \
    || fail "running container unexpectedly replaced the bind-mounted inode"
  docker exec "$container" touch /data/config.toml
  sleep 2
  inside_hash="$(docker exec "$container" sha256sum /run/secrets/openai_api_key | awk '{print $1}')"
  [[ "$inside_hash" == "$old_hash" ]] \
    || fail "runtime reload unexpectedly remounted the source path"
  expect_relay_generation reloaded old
fi

docker restart "$container" >/dev/null
wait_for_operator "$work_dir/operator-restarted.json"
inside_hash="$(docker exec "$container" sha256sum /run/secrets/openai_api_key | awk '{print $1}')"
[[ "$inside_hash" == "$new_hash" ]] \
  || fail "ordinary restart did not expose the atomically replaced source inode"
expect_relay_generation restarted new
docker logs "$container" >"$work_dir/runtime-restarted.log" 2>&1

docker rm -f "$container" >/dev/null
start_live_container "$secret_data" "$secret_file"
wait_for_operator "$work_dir/operator-recreated.json"
inside_hash="$(docker exec "$container" sha256sum /run/secrets/openai_api_key | awk '{print $1}')"
[[ "$inside_hash" == "$new_hash" ]] || fail "container recreation did not expose the new inode"
expect_relay_generation recreated new
docker exec "$container" codex-helper-server --check --json >"$work_dir/recreated-check.json"
assert_contains "$work_dir/recreated-check.json" '"aggregate": "ready"'

docker inspect "$container" >"$work_dir/inspect-recreated.json"
docker inspect --format '{{range .Mounts}}{{println .Destination}}{{end}}' "$container" \
  >"$work_dir/mount-destinations"
if grep -E '/(\.codex|\.claude)(/|$)' "$work_dir/mount-destinations" >/dev/null; then
  fail "server container received a client configuration or session mount"
fi
docker logs "$container" >"$work_dir/runtime-recreated.log" 2>&1

# Provider credentials must not reach config, migration backups, runtime state, logs,
# operator/check JSON, inspect output, or image layers.
assert_no_canary_tree "$env_data"
assert_no_canary_tree "$secret_data"
for artifact in \
  "$work_dir/environment-check.json" \
  "$work_dir/secret-check.json" \
  "$work_dir/secret-check.stderr" \
  "$work_dir/compose-check.json" \
  "$work_dir/empty-secret.json" \
  "$work_dir/empty-secret.stderr" \
  "$work_dir/oversize-secret.json" \
  "$work_dir/oversize-secret.stderr" \
  "$work_dir/nonregular-secret.json" \
  "$work_dir/nonregular-secret.stderr" \
  "$work_dir/operator-old.json" \
  "$work_dir/operator-restarted.json" \
  "$work_dir/operator-recreated.json" \
  "$work_dir/recreated-check.json" \
  "$work_dir/inspect-old.json" \
  "$work_dir/inspect-recreated.json" \
  "$work_dir/runtime-old.log" \
  "$work_dir/runtime-restarted.log" \
  "$work_dir/runtime-recreated.log" \
  "$work_dir/upstream-records.jsonl" \
  "$work_dir/upstream.stdout" \
  "$work_dir/upstream.stderr" \
  "$work_dir/relay-initial.json" \
  "$work_dir/relay-reloaded.json" \
  "$work_dir/relay-restarted.json" \
  "$work_dir/relay-recreated.json"; do
  assert_no_canary_file "$artifact"
done

docker save "$image" -o "$work_dir/image.tar"
assert_no_canary_file "$work_dir/image.tar"

if [[ "$(uname -s)" == "Linux" ]]; then
  echo "docker mounted-secret smoke passed"
else
  echo "docker mounted-secret smoke passed; Linux CI remains authoritative for host ACL and inode pinning"
fi

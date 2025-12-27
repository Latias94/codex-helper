# Changelog
All notable changes to this project will be documented in this file.

## [0.4.1] - 2025-12-27
### Changed
- `switch on` now sets `model_providers.codex_proxy.request_max_retries = 0` by default to avoid double-retry (Codex retries + codex-helper retries), while preserving any user-defined value.
- Proxy module refactor: split `proxy.rs` into `src/proxy/*` to make retry/streaming logic easier to reason about and test.

### Fixed
- Streaming (SSE) upstream disconnects now count as upstream failures and apply a cooldown penalty, improving failover when Codex retries the stream.
- `retry.attempts` no longer counts `all_upstreams_avoided` marker entries.

## [0.4.0] - 2025-12-23
### Added
- `session list/search` now shows rounds/turn counts and `last_response_at` (and includes these fields in `session export`).
- Session stats cache at `~/.codex-helper/cache/session_stats.json` to speed up `session list/search` (turn counts + timestamps).
- Retry now honors `Retry-After: <seconds>` when present.
- Codex bootstrap/import now imports all `model_providers.*` entries from `~/.codex/config.toml` as switchable configs (instead of only the active provider).
- Runtime-only upstream config overrides: per-session and global (applies to new requests only; does not interrupt in-flight streaming).
- TUI provider switch menu: `p` for session override, `P` for global override (with clear option).

### Changed
- Default retry status codes now include `429` (useful during high-demand throttling).
- `session list/search` candidate ordering prefers session file `mtime` (better behavior for resumed sessions).
- `rounds` is now computed as `min(user_turns, assistant_turns)` (best-effort).
- HTTP request body previews are now gated by `CODEX_HELPER_HTTP_LOG_REQUEST_BODY=1` (default off).
- Proxy hot path reduces copying/cloning: `Bytes` request bodies, lazy header-entry materialization, and zero-copy filtering when no active rules.
- TUI sessions list: improved layout and keeps selection visible while scrolling.
- Documentation refresh: remove untested Claude sections; highlight auto-retry and session-cache behavior.

## [0.3.0] - 2025-12-21
### Added
- Upstream retry with LB-aware failover (avoid previously-failed upstreams in the same request, and apply cooldown penalties for Cloudflare-like failures).
- Retry metadata in request logs: `retry.attempts` and `retry.upstream_chain` (only present when retries actually happen).
- Global retry config in `~/.codex-helper/config.json` under `retry` (env vars can override at runtime).
- Built-in TUI dashboard (iocraft-based; auto-enabled in interactive terminals; disable with `codex-helper serve --no-tui`).
- Runtime-only session overrides for `reasoning.effort` (applied to subsequent requests of the same Codex session; not persisted across restarts).
- Effort menu supports `low`/`medium`/`high`/`xhigh` and clear.
- Local control/status endpoints for the dashboard and debugging:
  - `GET/POST /__codex_helper/override/session`
  - `GET /__codex_helper/status/active`
  - `GET /__codex_helper/status/recent`
- Extra request log fields: `session_id`, `cwd`, and `reasoning_effort` when available.
- Non-2xx requests include a small header/body preview in logs by default (disable via `CODEX_HELPER_HTTP_WARN=0`).
- `http_debug.auth_resolution` records where upstream auth headers came from (never includes secrets), to help diagnose auth/config issues.
- `http_debug` is split to `requests_debug.jsonl` by default (disable via `CODEX_HELPER_HTTP_DEBUG_SPLIT=0`).
- `runtime.log` auto-rotates on startup when running with the built-in TUI (size/retention via `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES`).

### Changed
- Streaming responses are only proxied as SSE when upstream is `2xx`; non-2xx responses are buffered to enable classification/logging and optional retry before returning to the client.
- Retry defaults to 2 attempts; set `CODEX_HELPER_RETRY_MAX_ATTEMPTS=1` to disable.

### Fixed
- `cargo-binstall` metadata: correct `pkg-url`/`bin-dir` templates to match cargo-dist GitHub release artifacts (including Windows `.zip` layout), so `cargo binstall codex-helper` downloads binaries instead of building from source.
- Streaming requests now always clear `active_requests` and emit a final `finish_request` entry (fixes TUI stuck active sessions).
- `serve` always restores Codex/Claude config from backup on exit, even when startup fails after switching on.
- `switch on/off` now restores correctly when the original Codex/Claude config file did not exist (uses an "absent" sentinel backup instead of leaving clients pointed at a dead proxy).

## [0.2.0] - 2025-12-20
### Added
- Safe-by-default auth config: store secrets via env vars using `auth_token_env` / `api_key_env` (instead of writing tokens to disk).
- CLI support for env-based auth: `codex-helper config add --auth-token-env ...` / `--api-key-env ...`.
- Optional HTTP debugging logs (`http_debug`) with header/body previews, timing metrics, and Cloudflare/WAF detection hints.
- Request log controls:
  - automatic rotation and retention for `requests.jsonl` (and debug logs),
  - optional `CODEX_HELPER_REQUEST_LOG_ONLY_ERRORS=1`,
  - optional split debug log file `requests_debug.jsonl` (via `CODEX_HELPER_HTTP_DEBUG_SPLIT=1`).
- `doctor` checks for missing auth env vars and plaintext secrets in `~/.codex-helper/config.json`.

### Changed
- Codex bootstrap/import prefers recording the upstream `env_key` as `auth_token_env` (no longer persisting the token by default).
- Non-2xx terminal warnings no longer include response body previews unless explicitly enabled.

### Fixed
- Proxy auth handling for `requires_openai_auth=true` providers: preserve client `Authorization` when no upstream token is configured.
- Proxy URL construction when `base_url` includes a path prefix (avoid double-prefixing like `/v1/v1/...`).
- Hop-by-hop header filtering and safer response header forwarding for streaming/non-streaming responses.
- Request body filter fallback for invalid regex rules (avoid corrupting payloads).
- Session rollout filename UUID parsing, and deterministic `active_config()` fallback selection.

# Changelog
All notable changes to this project will be documented in this file.

> Starting from `0.5.0`, changelog entries are bilingual: **Chinese first, then English**.

## [0.10.0] - Unreleased
### 亮点 / Highlights
- TUI 全面升级：用 `ratatui v0.30` 重写内置面板，信息层级更清晰、布局更有“呼吸感”，并为后续功能页扩展预留了结构化模块。  
  TUI upgrade: rewrite the built-in dashboard with `ratatui v0.30`, with clearer information hierarchy, more breathable layout, and a modular structure for future features.
- TUI Configs 页：新增 Configs 视图用于查看 level 分组/启用状态，并可快速设置全局/会话级配置覆盖（pin 到单一 config）。  
  TUI Configs page: add a Configs view for level grouping/enabled status, with quick global/session overrides (pin to a single config).
- TUI 热编辑并即时生效：在 Configs 页可直接切换 `enabled`、调整 `level`，变更会立即影响路由，并自动落盘到配置文件（重启后仍生效）。  
  Live config edits in TUI: toggle `enabled` and adjust `level` directly in the Configs page; changes take effect immediately for routing and are persisted to the config file.
- TUI 多页监控与可解释性：新增 Sessions/Requests 页，支持快速筛选，并在请求详情中展示“上游尝试链路（route chain）”。  
  Multi-page monitoring + explainability: add Sessions/Requests pages with quick filters, and show the upstream attempt chain (route chain) in request details.
- TUI 用量与测速：新增 Stats 页展示 token/请求聚合与 Top config/provider；Configs 页支持一键健康检查（/models）并展示延迟与错误信息。  
  Usage + speed insights in TUI: add a Stats page with token/request rollups and top config/provider tables; add one-key health checks (/models) in Configs with latency/error details.
- 模型白名单与映射（支持通配符）：为每个上游增加 `supported_models` / `model_mapping`（兼容 JSON `supportedModels` / `modelMapping`），代理会自动过滤不支持的上游并在转发前重写 `model` 字段。  
  Model allowlist + mapping (wildcards supported): add per-upstream `supported_models` / `model_mapping` (JSON compatible via `supportedModels` / `modelMapping`); the proxy skips incompatible upstreams and rewrites `model` before forwarding.
- Level 分组（跨配置降级）：为每个配置增加 `level` / `enabled`，当存在多个 level 时，会按 level 从低到高（1→10）进行自动路由与故障降级。  
  Level-based config failover: add `level` / `enabled` to each config; when multiple levels exist, the proxy routes and fails over from lower to higher levels (1→10).

### 新增 / Added
- 启动时模型路由配置告警：提示未配置白名单/映射、仅配置映射未配置白名单、以及“映射目标不在白名单”等高风险配置。  
  Startup warnings for model routing config: warns about missing allowlist/mapping, mapping without allowlist, and invalid mapping targets.
- `config` 子命令增强：支持 `config set-level` / `config enable` / `config disable`，并在 `config list` 中显示 `level/enabled`。  
  Better `config` subcommands: add `config set-level` / `config enable` / `config disable`, and show `level/enabled` in `config list`.
- TUI Sessions 页：按会话维度查看活跃/错误/覆盖等状态，并提供联动筛选与快速重置。  
  TUI Sessions page: inspect sessions with active/error/override views, plus linked filters and quick reset.
- TUI Requests 页：按请求维度查看最新请求，支持错误筛选与 scope 切换，并展示每次重试的上游链路。  
  TUI Requests page: inspect recent requests with error filter and scope switch, and show the per-retry upstream chain.
- TUI Stats 页：展示按天聚合的 token 趋势、按 config/provider 的 Top 用量表；可通过 `CODEX_HELPER_PRICE_INPUT_PER_1K_USD` / `CODEX_HELPER_PRICE_OUTPUT_PER_1K_USD` 启用粗略成本估算。  
  TUI Stats page: show daily token rollups and top usage tables by config/provider; enable coarse cost estimates via `CODEX_HELPER_PRICE_INPUT_PER_1K_USD` / `CODEX_HELPER_PRICE_OUTPUT_PER_1K_USD`.
- TUI Configs 健康检查：`h` 检测选中 config，`H` 批量检测所有 config；结果会展示每个 upstream 的延迟与状态码。  
  TUI Configs health checks: `h` checks selected config, `H` checks all configs; results show per-upstream latency and status code.

### 测试 / Tests
- 新增用例覆盖：按模型过滤上游、以及请求体 `model` 自动映射。  
  Add tests for: skipping upstreams by model support, and request body `model` mapping.

## [0.5.0] - 2025-12-27
### 亮点 / Highlights
- Codex 任务结束通知：当 Codex 的一轮对话执行完成时，可以由 codex-helper 触发系统通知（默认关闭），并支持合并/限流以避免刷屏；也可配置自定义 Hook（exec，stdin 接收聚合 JSON）。  
  Codex turn-complete notifications: when a Codex turn completes, codex-helper can trigger a system notification (disabled by default) with merge/rate-limit to avoid spam; or call a custom hook (exec, aggregated JSON via stdin).
- 配置体验升级：支持 `~/.codex-helper/config.toml`（优先）与旧版 `config.json`（兼容）；提供 `codex-helper config init` 一键生成带注释的 TOML 默认模板（含配置示例）；首次自动落盘默认生成 TOML。  
  Better config UX: support `~/.codex-helper/config.toml` (preferred) while remaining compatible with legacy `config.json`; add `codex-helper config init` to generate a commented TOML default template (with examples); default to TOML on first write.

### 新增 / Added
- Codex `notify` 集成：支持系统通知与自定义 Hook（exec，stdin 接收聚合 JSON），并提供默认的合并/限流策略以减少噪音（系统通知默认关闭）。  
  Codex `notify` integration: system notifications and a custom hook (exec, aggregated JSON via stdin), with default merge/rate-limit policy to reduce noise (system notifications disabled by default).
- 双格式配置：支持 `~/.codex-helper/config.toml`（优先）与 `config.json`（兼容），并提供 `codex-helper config init` 生成带注释的 TOML 模板。  
  Dual config formats: support `~/.codex-helper/config.toml` (preferred) and `config.json` (compatible), plus `codex-helper config init` to generate a commented TOML template.

### 变更 / Changed
- 文档更新：说明 notify 的配置方式与 TOML 配置的优先级。  
  Docs update: describe notify configuration and TOML precedence.

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

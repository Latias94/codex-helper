# Changelog
All notable changes to this project will be documented in this file.

> Starting from `0.5.0`, changelog entries are bilingual: **Chinese first, then English**.

## [0.12.0] - Not released
### 新增 / Added
- 新增 `codex-helper session recent`：按会话文件最后更新时间（mtime）筛选最近会话，并支持 `text/tsv/json` 输出；可选 `--open` 通过 Windows Terminal（`wt`）或 WezTerm 打开并执行恢复命令，便于快速 `codex resume`（支持 `--since/--limit/--raw-cwd/--format/--open/--terminal/--shell/--resume-cmd`）。
  Add `codex-helper session recent`: filter recent sessions by session file mtime with `text/tsv/json` output; optionally `--open` via Windows Terminal (`wt`) or WezTerm to run a resume command for fast `codex resume` workflows (supports `--since/--limit/--raw-cwd/--format/--open/--terminal/--shell/--resume-cmd`).
- GUI 新增 `Stats` 页面：展示 requests/tokens KPI、按天 rollup、Top configs/providers，并支持基于 `CODEX_HELPER_PRICE_*` 的成本估算（如配置）。
  GUI adds a `Stats` page: requests/tokens KPIs, per-day rollup, top configs/providers, plus optional cost estimate via `CODEX_HELPER_PRICE_*`.
- 新增 attach 友好的 API v1 `GET /__codex_helper/api/v1/snapshot`：返回 GUI/TUI 所需的 dashboard 快照（含 usage rollup 与窗口统计），减少多次拉取。
  Add attach-friendly API v1 `GET /__codex_helper/api/v1/snapshot`: returns a dashboard snapshot (including usage rollup and window stats) to reduce multi-call polling.

### 改进 / Improved
- 启动监听失败时，提供更友好的提示（端口占用/权限不足等），并在 Windows/Linux/macOS 下尽力显示占用端口的进程 PID/名称，便于快速定位冲突进程。
  Provide friendlier bind/listen failure messages (port in use / permission denied, etc.) and best-effort show the PID/process holding the port on Windows/Linux/macOS for faster troubleshooting.
- `codex-helper session list` 默认完整展示 first prompt，并提供 `--truncate` 以按需截断输出。
  `codex-helper session list` now prints the full first prompt by default and provides `--truncate` for an optional compact view.
- GUI `History` 页面新增“全局最近”范围：按 mtime（默认近 12 小时）列出最近 Codex 会话，并支持一键复制 `root id` 列表与在 Windows Terminal（`wt`）中直接执行 `codex resume`。
  GUI `History` adds a “Global recent” scope: list recent Codex sessions by mtime (default last 12 hours), copy `root id` lists, and open `codex resume` directly in Windows Terminal (`wt`).

## [0.11.0] - 2026-01-09
### 修复 / Fixed
- 修复 `balanced` 等策略下，`never_on_status` 对 `400` 的默认 guardrail 会误伤可重试的错误分类（例如 `cloudflare_challenge`），导致 400 场景无法按 `on_class` 触发重试/切换的问题。
  Fix an issue where the default `never_on_status` guardrail (including `400`) could override retryable error classes (e.g. `cloudflare_challenge`) under profiles like `balanced`, preventing retries/failover for certain 400 responses that should be eligible via `on_class`.

### 变更 / Changed
- 默认 `never_on_status` 移除 `400`，保留 `413/415/422` 作为更“确定是请求侧”的状态码兜底；仍建议使用 `never_on_class=["client_error_non_retryable"]` 阻止明显的参数/校验类错误扩散到多 provider。
  Default `never_on_status` now excludes `400` and keeps `413/415/422` as more clearly client-side guardrails; keep using `never_on_class=["client_error_non_retryable"]` to avoid amplifying obvious request/validation mistakes across providers.

## [0.10.0] - 2026-01-07
### 新增 / Added
- TUI 新增 `7 历史` 页面：展示当前目录相关的 Codex 本地历史会话（`~/.codex/sessions`），可在未活跃会话上直接打开 transcript。
  TUI adds `7 History`: browse Codex local history sessions (`~/.codex/sessions`) for the current directory and open transcripts even for inactive sessions.
- 增强 `aggressive-failover`：在更高尝试次数基础上，对更多非 2xx 错误尝试 failover 到备选 upstream/provider；并引入 `never_on_status` / `never_on_class` 兜底以避免对明显的客户端参数错误进行无意义切换。
  Improve `aggressive-failover`: in addition to more attempts, fail over on a broader set of non-2xx errors; add `never_on_status` / `never_on_class` guardrails to avoid pointless switching on obvious client-side request mistakes.

### 改进 / Improved
- TUI transcript 视图升级为全屏“页面式”展示，并新增 `A`（全量/尾部切换）与 `y`（复制到剪贴板）。
  TUI transcript view is now full-screen, with `A` (toggle all/tail) and `y` (copy to clipboard).
- 重试/切换模型升级为“两层”：先在当前 provider/config 内做 upstream 级重试（默认优先同一 upstream），仍失败再做 provider/config 级 failover；在可用性优先模式下对 4xx（非 429）等路由/认证类错误也会更积极切换，并对失败线路施加冷却惩罚，降低“坏线路反复被选中”的概率。
  Retry/failover model upgraded to two layers: retry within the current provider/config first (upstream-layer, default prefers the same upstream), then fail over across upstreams and configs/providers (provider/config layer). In availability-first mode, it also switches more aggressively on 4xx (except 429) routing/auth failures, and applies cooldown penalties to reduce repeatedly selecting a broken route.
- 默认策略兜底补强：`balanced` 也会对常见上游认证/路由类错误（例如 401/403/404/408）触发 provider/config 级 failover；并默认启用 `never_on_status` / `never_on_class` 兜底，避免将明显的客户端参数错误扩散到多 provider（旧版 `[retry]` 扁平字段仍兼容）。
  Default strategy guardrails: `balanced` now triggers provider/config failover on common auth/routing errors (e.g. 401/403/404/408) and enables `never_on_status` / `never_on_class` by default to avoid amplifying obvious client-side mistakes across providers (legacy flat `[retry]` fields remain compatible).

### 修复 / Fixed
- 修复切换页面时偶发的 UI 残影：页面切换时强制 `terminal.clear()` 后重绘。
  Fix occasional UI artifacts when switching pages: force a `terminal.clear()` on page switch before redraw.
- 修复 TUI 在 Ctrl+C/关停时偶发卡住：为快照刷新与 Settings 页本地 HTTP 拉取增加超时，并监听 Ctrl+C 信号以更快退出。
  Fix occasional TUI hangs on Ctrl+C/shutdown: add timeouts to snapshot refresh and Settings local HTTP fetch, and listen for Ctrl+C for faster exit.

## [0.9.0] - 2026-01-01
### 新增 / Added
- 新增 `codex-helper session transcript <ID>`：按 session id 输出 Codex 会话的历史对话（best-effort 解析 `~/.codex/sessions/*.jsonl`），用于快速辨认 session（支持 `--tail/--all` 以及 `--format text|markdown|json`）。
  Add `codex-helper session transcript <ID>`: print a Codex session transcript by id (best-effort parse of `~/.codex/sessions/*.jsonl`) to quickly identify sessions (supports `--tail/--all` and `--format text|markdown|json`).
- TUI Sessions 页新增 `t`：打开所选 session 的 transcript 弹窗（默认展示最近 80 条消息，支持滚动/翻页）。
  TUI Sessions page adds `t`: open a transcript modal for the selected session (shows last 80 messages by default, scroll/page supported).
### 改进 / Improved
- `config.toml` 中 `ServiceConfig.name` 允许省略：加载/保存时会自动用配置 key 回填，减少重复字段与配置噪音。
  Allow omitting `ServiceConfig.name` in `config.toml`: it is auto-filled from the config key on load/save, reducing redundant fields and config noise.
- 请求性能指标增强：`requests.jsonl` 新增 `ttfb_ms`，Stats 表格新增 `tok/s`（按 `duration_ms - ttfb_ms` 估算生成阶段输出速率），Requests 详情显示 `ttfb` / `usage` / `out_tok/s`。
  Performance metrics: add optional `ttfb_ms` to `requests.jsonl`, add `tok/s` to Stats (output rate over `duration_ms - ttfb_ms`), and show `ttfb` / `usage` / `out_tok/s` in the Requests details panel.

## [0.8.0] - 2025-12-31
### 新增 / Added
- 新增落地的重试/选路追踪日志 `retry_trace.jsonl`（默认在 `~/.codex-helper/logs/`），用于诊断“为何没切 provider / 为何重试没 failover”等问题：`CODEX_HELPER_RETRY_TRACE=1`。
  Add an on-disk retry/routing trace log `retry_trace.jsonl` (default under `~/.codex-helper/logs/`) to diagnose “why provider didn’t switch / why retries didn’t fail over”: `CODEX_HELPER_RETRY_TRACE=1`.
- 新增 `codex-helper config set-retry-profile`：一键写入推荐的 `[retry]` 策略预设（例如 `balanced` / `cost-primary`），让用户只用“选策略 + 配分组”即可落地常见场景。
  Add `codex-helper config set-retry-profile`: apply curated `[retry]` policy presets (e.g. `balanced` / `cost-primary`) so common setups only need “pick a strategy + set routing groups”.
- `[retry]` 新增 `profile` 字段：在配置文件里直接选择策略预设（profile 先应用默认值，再用同段里显式写出的字段做覆盖）。
  Add `[retry].profile`: select a retry policy preset directly in the config file (apply profile defaults first, then override with explicitly set fields).

### 修复 / Fixed
- 修复同 level 多 provider 时，failover 无法跨 config 切换的问题（active 优先，但同级其他 config 也会参与 failover）。
  Fix failover across multiple same-level providers/configs (active is preferred, but other same-level configs now participate in failover).
- 修复 TUI 切换页签时顶部区域偶发残影/脏 UI（清空背景 buffer 字符 + header 内容与边框行分离渲染）。
  Fix occasional TUI header artifacts when switching tabs (clear background buffer symbols + render header content and border on separate rows).
- 修复 TUI Sessions 的 CWD 在 Windows 路径下无法正确取目录名的问题（之前只识别 `/`，导致整条 `C:\...` 被截断显示）。
  Fix TUI Sessions CWD basename on Windows paths (previously only `/` was handled, so full `C:\...` got truncated).
- 修复 usage token 解析兼容性：支持 Chat Completions 风格的 `prompt_tokens`/`completion_tokens`（以及 `completion_tokens_details.reasoning_tokens`），避免 Requests 面板 Tok 长期为 0。
  Fix usage token parsing compatibility: support Chat Completions-style `prompt_tokens`/`completion_tokens` (and `completion_tokens_details.reasoning_tokens`) so Tok no longer stays at 0 in the Requests panel.
- 修复流式（SSE）长响应中 usage 出现较晚时统计丢失的问题（不再因缓冲上限而错过 usage）。
  Fix missing usage in long streaming (SSE) responses when usage arrives late (no longer missed due to buffer limits).

### 改进 / Improved
- TUI 的“全局 provider 选择”改为落盘写入本地配置 `active`（首选但允许 failover），并在 Codex 场景 best-effort 从 `~/.codex/config.toml` + `auth.json` 同步账号 env key（不写入 secrets）。
  TUI “global provider selection” now persists as local config `active` (preferred but failover-enabled), and best-effort syncs Codex auth env keys from `~/.codex/config.toml` + `auth.json` (no secrets are written).
- `codex-helper config init` 生成的 `config.toml` 模板注释默认使用中文（更符合中文用户阅读习惯）。
  `codex-helper config init` now generates a `config.toml` template with Chinese comments by default.
- `codex-helper config init` 在检测到 `~/.codex/config.toml` 时，会 best-effort 自动导入 Codex providers 到生成的 `config.toml`（可用 `--no-import` 关闭）。
  `codex-helper config init` now best-effort auto-imports Codex providers into the generated `config.toml` when `~/.codex/config.toml` is present (disable via `--no-import`).
- 如果你是从旧版本升级且想拿到最新的 `config.toml` 模板/注释/默认项，可以备份后尝试重新初始化：`codex-helper config init --force`（会覆盖现有 `~/.codex-helper/config.toml`；工具会 best-effort 备份为 `config.toml.bak`）。
  If you upgraded from an older version and want the latest `config.toml` template/comments/defaults, consider re-initializing after a backup: `codex-helper config init --force` (overwrites `~/.codex-helper/config.toml`; best-effort backup to `config.toml.bak`).
- failover 模式下，对触发重试的 `5xx` 状态也会施加冷却，并支持“冷却指数退避”（用于“便宜主线路不稳定 → 降级 → 隔一段时间探测回切”的主从成本优化场景）。
  In failover mode, retryable `5xx` responses also trigger cooldown penalties, with optional exponential cooldown backoff (for “cheap primary unstable → degrade → periodically probe back” cost-optimization setups).
- TUI 截断逻辑按终端显示宽度裁剪（更适配中文/emoji 等宽字符），并对 URL/path/base_url 等字段使用中间截断以保留两端关键信息。
  TUI truncation now respects terminal display width (better for CJK/emoji wide chars), and URL/path/base_url fields use middle truncation to preserve both ends.

### 变更 / Changed
- 从 `v0.8.0` 起，重试参数不再支持通过 `CODEX_HELPER_RETRY_*` 环境变量覆盖；统一以 `~/.codex-helper/config.toml`（或 `config.json`）的 `[retry]` 段为准。
  Starting from `v0.8.0`, retry parameters are no longer overridable via `CODEX_HELPER_RETRY_*` environment variables; only the `[retry]` block in `~/.codex-helper/config.toml` (or `config.json`) is used.
- `config init` / 自动导入 Codex providers 时，不再默认补齐 `openai`；仅按 `~/.codex/config.toml`（及其 backup）里的 `[model_providers.*]` 生成对应的 configs。
  When importing Codex providers (e.g. `config init`), we no longer auto-add `openai`; only providers declared under `~/.codex/config.toml` (and its backup) `[model_providers.*]` are converted into configs.

## [0.7.0] - 2025-12-29
### 新增 / Added
- 覆盖导入增加二次确认：`codex-helper config overwrite-from-codex` 需要 `--yes` 才会写盘；TUI Settings 页 `O` 需 3 秒内二次按键确认，避免误操作。  
  Add confirmation for overwrite import: `codex-helper config overwrite-from-codex` requires `--yes` to write; TUI Settings `O` needs a second press within 3s to confirm.
- 运行态配置热加载：覆盖导入或手动修改配置文件后，无需重启，下一次请求会按新的 `active`/配置路由。  
  Runtime config hot reload: after overwrite import or manual edits, no restart needed—next request uses the updated `active`/routing config.
- Settings 页增加运行态配置状态：展示最近一次加载时间与当前 retry 配置，支持 `R` 立即触发重载。  
  Settings now shows runtime config status: last loaded time and current retry config, with `R` to trigger reload.

## [0.6.0] - 2025-12-29
### 亮点 / Highlights
- 重新设计 TUI：使用 `ratatui v0.30` 重写，信息分层更清晰（Header 总览 / 页面主体 / Footer 快捷键），为后续功能扩展预留结构。  
  Redesigned TUI: rewritten with `ratatui v0.30`, clearer hierarchy (header overview / page body / footer shortcuts) and a structure ready for future features.
- 更清晰的导航与指引：`1-6` 切换页面，`?` 查看帮助，`L` 切换中英（首次启动默认跟随系统语言）。  
  Clearer navigation: `1-6` switches pages, `?` opens help, `L` toggles CN/EN (first run follows system language).
- 代理可用性一眼可见：Header 展示 5m/1h 成功率、p95、429/5xx、平均尝试次数，并把 health check “进行中总览”放到顶部。  
  Proxy availability at a glance: header shows 5m/1h success rate, p95, 429/5xx, avg attempts, plus an in-progress health-check overview.
- Configs 可解释 + 可操作：支持 `enabled/level` 热编辑并落盘；`i` 打开 config/provider 详情（auth、模型/映射、LB/health、延迟/错误）。  
  Configs is explainable and actionable: hot-edit `enabled/level` with persistence; `i` opens config/provider details (auth, models/mapping, LB/health, latency/errors).
- Stats 报告：Stats 页支持一键复制/导出报告（例如最近错误 Top 状态码/路径/模型），方便分享与排障。  
  Stats reports: one-key copy/export (e.g. recent top errors by status/path/model) for sharing and debugging.
- Settings 页补齐：从 “coming soon” 变为运行态与配置入口信息面板。  
  Settings page is now real: replaces “coming soon” with a runtime/config entry overview panel.

### 新增 / Added
- Level 分组与跨配置降级：为每个 config 增加 `level` / `enabled`，多 level 时按 `1→10` 自动路由与故障降级。  
  Level-based routing + failover: per-config `level` / `enabled`, routes/fails over from `1→10` when multiple levels exist.
- 从 Codex CLI 覆盖导入账号/配置：新增 `codex-helper config overwrite-from-codex`，清空并重建 codex-helper 的 Codex 配置（默认分组/level）。  
  Overwrite Codex configs from Codex CLI: add `codex-helper config overwrite-from-codex` to reset and rebuild codex-helper Codex configs (default grouping/levels).
- 模型白名单与映射（通配符）：新增 `supported_models` / `model_mapping`（兼容 JSON `supportedModels` / `modelMapping`），在转发前过滤不支持上游并重写 `model`。  
  Model allowlist + mapping (wildcards): `supported_models` / `model_mapping` (JSON `supportedModels` / `modelMapping` compatible), filters incompatible upstreams and rewrites `model` before forwarding.
- `config` 子命令增强：新增 `config set-level` / `config enable` / `config disable`，并在 `config list` 中显示 `level/enabled`。  
  Enhanced `config` subcommands: add `config set-level` / `config enable` / `config disable`, and show `level/enabled` in `config list`.

### Changed
- 默认重试状态码包含 `429`。  
  Default retry status codes include `429`.
- TUI 渲染层重构：按 `src/tui/view/{chrome,widgets,modals,pages}` 拆分，便于持续迭代与新增页面。  
  TUI renderer refactor: split into `src/tui/view/{chrome,widgets,modals,pages}` for easier iteration and new pages.

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
- Non-2xx requests include a small header/body preview in logs by default (disable with `CODEX_HELPER_HTTP_WARN=0`).
- `http_debug.auth_resolution` records where upstream auth headers came from (never includes secrets), to help diagnose auth/config issues.
- `http_debug` is split to `requests_debug.jsonl` by default (disable with `CODEX_HELPER_HTTP_DEBUG_SPLIT=0`).
- `runtime.log` auto-rotates on startup when running with the built-in TUI (size/retention via `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES`).

### Changed
- Streaming responses are only proxied as SSE when upstream is `2xx`; non-2xx responses are buffered to enable classification/logging and optional retry before returning to the client.
- Retry defaults to 2 attempts; set `retry.max_attempts = 1` to disable.

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

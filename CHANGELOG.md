# Changelog
All notable changes to this project will be documented in this file.

> Starting from `0.5.0`, changelog entries are bilingual: **Chinese first, then English**.

## [未发布 / Unreleased]

## [0.13.0] - 2026-05-09

### 重点 / Highlights

- `version = 3` 成为默认配置：先定义 provider，再用 routing 表达“按顺序兜底 / 手动固定 / 包月优先”等策略。旧配置会自动迁移到 `config.toml`，并保留 `.bak` 备份。
  `version = 3` is now the default config model: define providers once, then use routing for ordered fallback, manual pinning, or monthly-first policies. Older configs migrate automatically with a `.bak` backup.
- 新增更好懂的 provider 切换体验：包月中转可以打 `billing=monthly` 标签，已知耗尽后再按策略继续或停止。
  Provider switching is clearer: monthly relays can be tagged with `billing=monthly`, then known exhaustion can either fall through or stop according to policy.
- 余额和套餐更可见：Sub2API、New API 和常见 `/user/balance` 接口会自动探测；查询失败显示为 `unknown`，不会被当作耗尽。
  Balance and plan visibility improved: Sub2API, New API, and common `/user/balance` endpoints are auto-probed. Lookup failures show as `unknown` and do not count as exhaustion.
- TUI/GUI 更适合日常操作：routing 页面会显示 provider 顺序、余额/套餐、tags、启停状态和候选状态；请求视图会显示 token、cache token、耗时、速度、重试和估算成本。
  TUI/GUI are more operator-friendly: routing pages show provider order, balances/plans, tags, enabled state, and candidates; request views show tokens, cache tokens, latency, speed, retries, and estimated cost.
- Codex 配置 patch 更安全：`switch on/off` 只修改 codex-helper 的本地代理片段，不再用旧快照覆盖 Codex 运行期间写入的其它配置。
  Codex config patching is safer: `switch on/off` only changes the codex-helper local proxy section and no longer restores an old whole-file snapshot over Codex edits.
- 长时间运行更稳：上游连接增加连接超时、TCP keepalive、空闲连接回收；运行态日志和 TUI/GUI 刷新路径做了有界化处理。
  Long-running proxy stability improved with connect timeouts, TCP keepalive, idle connection cleanup, rotated runtime logs, and bounded TUI/GUI refresh state.

### 可复制 Routing 示例 / Copyable Routing Examples

先定义 provider，再复制一个 `[codex.routing]` 策略。Claude 配置同理，把 `codex` 换成 `claude`。
Define providers once, then copy one `[codex.routing]` policy. For Claude, replace `codex` with `claude`.

```toml
version = 3

[codex.providers.monthly_a]
base_url = "https://monthly-a.example.com/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example.com/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo" }
```

顺序兜底：最直观的优先级链。
Ordered fallback: the clearest priority chain.

```toml
[codex.routing]
policy = "ordered-failover"
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

手动固定：临时强制使用一个 provider。
Manual sticky: force one provider temporarily.

```toml
[codex.routing]
policy = "manual-sticky"
target = "monthly_a"
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

包月优先并保持可用：先用 `billing=monthly`，已知耗尽后继续兜底。
Monthly first with fallback: prefer `billing=monthly`, then fall back after known exhaustion.

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

包月严格止损：包月都已知耗尽时停止，不走付费兜底。
Strict monthly budget: stop instead of falling back to pay-as-you-go.

```toml
[codex.routing]
policy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
order = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "stop"
```

更多配置示例见 `docs/CONFIGURATION.md`。
More config recipes are available in `docs/CONFIGURATION.md`.

### 新增 / Added

- `provider` / `routing` 命令与 API：新增 provider、调整 fallback 顺序、pin provider、启停 provider、编辑标签、解释当前 routing。
  `provider` / `routing` commands and APIs: add providers, reorder fallback, pin, enable/disable, edit tags, and explain routing.
- 余额适配与自动探测：Sub2API `/v1/usage`、Sub2API dashboard `/api/v1/auth/me`、New API `/api/user/self`、通用 `/user/balance`。
  Balance adapters and auto-probing for Sub2API `/v1/usage`, Sub2API dashboard `/api/v1/auth/me`, New API `/api/user/self`, and generic `/user/balance`.
- 价格目录刷新与请求成本估算：可同步外部价格目录，不需要把模型价格写死在主配置里。
  Pricing catalog refresh and request cost estimates: sync external price catalogs instead of hardcoding model prices in the main config.

### 改进 / Improved

- TUI 的 routing/provider 视图更接近真实用户心智：显示 policy、顺序、余额、套餐、tags、启停状态和候选状态。
  TUI routing/provider views now match the user mental model better: policy, order, balance, plan, tags, enabled state, and candidate status.
- GUI 可以编辑常见单 endpoint provider 和 routing；复杂 provider 保持只读，避免 UI 保存时丢掉高级字段。
  GUI can edit common single-endpoint providers and routing; complex providers remain read-only to avoid dropping advanced fields.
- 请求日志和统计更有用：provider、model、input/output token、cache token、TTFB、总耗时、输出速度、重试链和估算成本会尽量进入 ledger/UI。
  Request logs and stats now include provider, model, input/output tokens, cache tokens, TTFB, total duration, output speed, retry chain, and estimated cost where available.
- README 和配置文档已重写为首页 + recipes + reference 的结构，新用户更容易复制可用配置。
  README and configuration docs now follow a homepage + recipes + reference structure for easier onboarding.

### 修复 / Fixed

- 修复退出 codex-helper 后用旧快照覆盖 `~/.codex/config.toml` 的问题，Codex 自动写入的 project trust 等配置会保留。
  Fixed old snapshot restore overwriting `~/.codex/config.toml`; Codex-written project trust and similar entries are preserved.
- 修复 TUI provider 列表重复行、顶部状态栏和底部快捷键在窄终端下显示不稳的问题。
  Fixed TUI provider-list duplicate rows plus top-status/footer layout issues in narrow terminals.
- 修复 v3 provider/routing 保存后可能丢失 provider tags、endpoint tags、模型支持和 model mapping 的问题。
  Fixed v3 provider/routing saves potentially losing provider tags, endpoint tags, model support, and model mappings.
- 修复余额查询失败残留旧耗尽状态的问题；HTTP 404 等失败现在显示为 `unknown`，不会影响 routing。
  Fixed failed balance lookups leaving stale exhaustion state; failures such as HTTP 404 now show as `unknown` and do not affect routing.

### 升级说明 / Upgrade Notes

- 正常升级不需要手动重写配置。启动 CLI、TUI、GUI 或代理时会自动迁移旧配置。
  Normal upgrades do not require manual config rewrites. CLI, TUI, GUI, and proxy startup migrate old configs automatically.
- 想先查看迁移结果，可以运行：
  To preview migration first, run:

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

- 新版本下 provider 选择统一由 routing 负责。外部脚本如果还在写旧 station/active 字段，建议改用 `provider` / `routing` 命令、API 或 v3 TOML。
  Provider selection now belongs to routing. External scripts that still write old station/active fields should move to `provider` / `routing` commands, APIs, or v3 TOML.
- 余额查询失败不再意味着不可用；只有可信的已耗尽快照才会参与 routing 降级。
  Balance lookup failure no longer means unavailable; only trusted exhausted snapshots can demote routing.

## [0.12.1] - 2026-02-09
### 新增 / Added
- TUI 新增 `8 最近 / 8 Recent` 页面：按时间窗口（30m/1h/3h/8h/12h/24h）筛选本机 Codex 最近会话；列表两行展示（root + 分支 / session_id），并支持一键复制 `root session_id`（选中/全部可见）。
  TUI adds an `8 Recent` page: filter local Codex sessions by time windows (30m/1h/3h/8h/12h/24h); two-line rows (root + branch / session_id), with one-key copy of `root session_id` (selected / all visible).
- GUI `History -> 全局最近 / Global recent` 增强：新增时间窗口预设与分钟级自定义；会话列表两行展示并显示分支名；新增快捷键 `Ctrl+Y` 复制可见 `root id` 列表、`Ctrl+Enter` 复制选中条目。
  GUI `History -> Global recent` enhancements: time-window presets + minute-level customization; two-line rows with branch name; shortcuts `Ctrl+Y` to copy visible `root id` list and `Ctrl+Enter` to copy selected.

### 改进 / Improved
- TUI 会话 ID 展示更友好：详情/弹窗完整显示 `session_id`；短展示改为尾部省略（避免中间隐藏导致难以辨认/复制）。
  Friendlier TUI session id display: show full `session_id` in details/modals; short display now uses end-ellipsis (avoids mid-hidden ids that are harder to recognize/copy).

### 修复 / Fixed
- TUI 在非交互终端（非 TTY）环境下会自动退出；进入 alternate screen 失败时会回滚 raw mode，避免终端状态残留。
  TUI now exits automatically in non-interactive (non-TTY) environments; entering alternate screen failure rolls back raw mode to avoid leaving the terminal in a bad state.

## [0.12.0] - 2026-02-08
### 新增 / Added
- `codex-helper serve` 新增 `--host` 参数：支持绑定到 `0.0.0.0` 等非 loopback 地址（默认仍为 `127.0.0.1`）。
  Add `--host` to `codex-helper serve`: supports binding to non-loopback addresses like `0.0.0.0` (default remains `127.0.0.1`).
- 新增 `codex-helper session recent`：按会话文件最后更新时间（mtime）筛选最近会话，并支持 `text/tsv/json` 输出；可选 `--open` 通过 Windows Terminal（`wt`）或 WezTerm 打开并执行恢复命令，便于快速 `codex resume`（支持 `--since/--limit/--raw-cwd/--format/--open/--terminal/--shell/--resume-cmd`）。
  Add `codex-helper session recent`: filter recent sessions by session file mtime with `text/tsv/json` output; optionally `--open` via Windows Terminal (`wt`) or WezTerm to run a resume command for fast `codex resume` workflows (supports `--since/--limit/--raw-cwd/--format/--open/--terminal/--shell/--resume-cmd`).
- GUI 新增 `Stats` 页面：展示 requests/tokens KPI、按天 rollup、Top configs/providers，并支持基于 `CODEX_HELPER_PRICE_*` 的成本估算（如配置）。
  GUI adds a `Stats` page: requests/tokens KPIs, per-day rollup, top configs/providers, plus optional cost estimate via `CODEX_HELPER_PRICE_*`.
- 新增 attach 友好的 API v1 `GET /__codex_helper/api/v1/snapshot`：返回 GUI/TUI 所需的 dashboard 快照（含 usage rollup 与窗口统计），减少多次拉取。
  Add attach-friendly API v1 `GET /__codex_helper/api/v1/snapshot`: returns a dashboard snapshot (including usage rollup and window stats) to reduce multi-call polling.
- GUI `History` 新增“全部(按日期)”视图：按日期分组浏览全部 Codex 本地会话，并支持对话预览与复制（默认隐藏工具调用）。
  GUI `History` adds an “All (by date)” view: browse all local Codex sessions grouped by day, with transcript preview and copy (tool calls hidden by default).
- GUI 新增托盘快速操作：支持一键显示/隐藏窗口、启动/停止/重载代理，并可通过托盘菜单快速切换 Active / Pinned / Routing Preset（best-effort 立即应用）。
  GUI adds a tray quick-actions menu: show/hide, start/stop/reload proxy, plus quick switching Active / Pinned / Routing Preset (best-effort apply now).

### 改进 / Improved
- 上游连接更稳健：为代理的 HTTP client 增加 `connect_timeout`/`tcp_keepalive`/`pool_idle_timeout`，降低长时间运行后连接“半死不活”导致的请求挂起概率。
  More robust upstream connections: add `connect_timeout`/`tcp_keepalive`/`pool_idle_timeout` to the proxy HTTP client to reduce hung requests caused by stale connections over long runs.
- 诊断信息更清晰：上游 `transport_error` 现在会记录更完整的错误链（caused-by/source）与关键标志（如 timeout/connect），便于区分 DNS/TLS/连接超时等问题。
  Clearer diagnostics: upstream `transport_error` now logs a richer error chain (caused-by/source) plus key flags (timeout/connect) to help distinguish DNS/TLS/connect timeouts, etc.
- 启动监听失败时，提供更友好的提示（端口占用/权限不足等），并在 Windows/Linux/macOS 下尽力显示占用端口的进程 PID/名称，便于快速定位冲突进程。
  Provide friendlier bind/listen failure messages (port in use / permission denied, etc.) and best-effort show the PID/process holding the port on Windows/Linux/macOS for faster troubleshooting.
- `codex-helper session list` 默认完整展示 first prompt，并提供 `--truncate` 以按需截断输出。
  `codex-helper session list` now prints the full first prompt by default and provides `--truncate` for an optional compact view.
- GUI `History` 页面新增“全局最近”范围：按 mtime（默认近 12 小时）列出最近 Codex 会话，并支持一键复制 `root id` 列表与在 Windows Terminal（`wt`）中直接执行 `codex resume`。
  GUI `History` adds a “Global recent” scope: list recent Codex sessions by mtime (default last 12 hours), copy `root id` lists, and open `codex resume` directly in Windows Terminal (`wt`).
- GUI `History` 会话恢复流程增强：按工作目录/项目分组、组内批量打开（默认每会话新窗口）、记住终端/shell/resume 命令/工作目录模式等偏好，并展示“可打开/总数”和跳过原因摘要。
  GUI `History` resume workflow: group by workdir/project, batch open sessions (default new window per session), persist terminal/shell/resume/workdir preferences, and show openable/total counts with skip reasons.
- GUI 启动更安全：默认不自动附着到已有代理（避免 TUI 已运行的代理被误操作）；端口与服务选择以配置为准。
  Safer GUI startup: do not auto-attach to an existing proxy by default (avoids interfering with a proxy started from TUI); service/port follow the config.
- GUI `History`/Transcript 性能优化：对话加载改为后台任务（避免 UI 卡死），并将消息列表改为虚拟滚动渲染，长对话也更流畅。
  GUI `History`/Transcript performance: load transcripts in background (non-blocking UI) and render the message list via virtual scrolling for smoother long sessions.
- GUI `History` 内部重构：拆分组件并减少不必要的会话列表 clone，避免为绕过 borrow-checker 引入的额外内存开销。
  GUI `History` internal refactor: componentized UI and removed unnecessary session list clones to avoid extra allocations used as a borrow-checker workaround.
- GUI `History` 滚动体验更稳定：统一跨布局的 ScrollArea 标识，使窗口大小变化时更容易保留滚动/选择状态。
  GUI `History` scrolling is more stable: unify ScrollArea IDs across layouts to better preserve scroll/selection state while resizing.
- 内部重构：将代码拆分为 workspace 的 `codex-helper-core` / `codex-helper-tui` / `codex-helper-gui` 三个 crate，降低耦合并提升可维护性（对外 CLI/GUI 使用方式不变）。
  Internal refactor: split into a workspace with `codex-helper-core` / `codex-helper-tui` / `codex-helper-gui` crates to reduce coupling and improve maintainability (CLI/GUI usage unchanged).

### 修复 / Fixed
- 修复 GUI 在部分平台/工具链上因 `fontdb` `Source` 变更导致的编译失败，并移除无效的本地 `cfg(feature = "fs"/"memmap")` 条件分支。
  Fix GUI build failures caused by `fontdb` `Source` changes and remove invalid local `cfg(feature = "fs"/"memmap")` guards.
- 修复 Linux release 构建中 GUI 依赖缺失导致的打包失败：通过 `cargo-dist` 声明 `pango/gtk/appindicator/pkg-config` 等原生依赖，确保 CI 自动安装所需包。
  Fix Linux release packaging failures due to missing GUI native deps by declaring required `pango/gtk/appindicator/pkg-config` packages via `cargo-dist` so CI installs them automatically.
- 修复 Linux GUI 链接失败：补齐 `libxdo` 原生依赖，避免 `-lxdo` 找不到导致的构建失败。
  Fix Linux GUI link failures by adding the missing `libxdo` system dependency (avoids `-lxdo` not found at link time).
- macOS 下 `open_in_file_manager(..., select_file=true)` 改为使用 `open -R` 以在 Finder 中定位文件。
  On macOS, use `open -R` for `open_in_file_manager(..., select_file=true)` to reveal the file in Finder.
- 修复 “recent sessions” 边界条件：当 `since=0` 时应返回空结果，避免 mtime 精度导致的偶发误筛选。
  Fix a recent-sessions edge case: `since=0` now returns an empty result to avoid rare mis-filtering due to mtime precision.

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

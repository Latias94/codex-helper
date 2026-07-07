# Changelog
All notable changes to this project will be documented in this file.

> Recent entries use **Chinese first, then an English summary**. Older entries keep the previous inline bilingual style.

## [0.20.0] - 2026-07-07

### 中文

#### 新增

- 新增可选的 TUI `5 状态` 页。启用 `[ui.service_status]` 后，可以按 provider / endpoint 发起轻量探针，或读取只读 status JSON，方便判断线路是否真的可用。
- 新增 Codex hosted image generation 开关：`[codex.client_patch].hosted_image_generation = "auto" | "enabled" | "disabled"`。关闭后会在 Codex patch 和代理转发时移除 hosted image tool；OpenAI Images 兼容入口仍可继续使用。
- 新增 provider signal / policy action 控制链路。限流、传输错误和可信余额耗尽会形成可解释的路由证据，并显示在 request ledger、admin API、TUI 和桌面端里。
- Reasoning Guard 支持更完整的 `518*n-2` 推理 token 边界识别（如 `516/1034/1552/2070`）。开启 guard 后默认匹配到 `n <= 4`，可用 `boundary_sequence_max_n = 0` 关闭序列匹配。
- Reasoning Guard 新增 `on_retry_exhausted = "pass" | "block"`。默认 `pass`：多次命中仍修不掉时放行最后一次上游响应，避免 helper 中断 Codex 任务。

#### 变更

- 可信余额耗尽现在会通过 codex-helper 自己管理的 balance policy action 影响路由；新的可用余额只会清理 helper 自动创建的 action，不会覆盖手动禁用或其它 cooldown。
- 自动余额探针会记住成功的 adapter，暂时跳过刚失败或今日套餐已耗尽的 adapter；路由降级后也会节流复查高优先级线路，减少余额接口滥用。
- Tauri 桌面端仍只作为源码内预览和内部打包验证路径；v0.20.0 公开 release 不发布桌面安装包。
- 依赖栈升级到当前可用版本，并适配 `toml` / `toml_edit` / `rand` / `rusqlite` / `tauri` 等跨版本 API 变化。

#### 破坏性变更

- 移除旧 `codex-helper-gui` egui crate、二进制入口和 `gui` feature。GUI 方向收口到 `apps/desktop` Tauri，公开安装命令只保留 `codex-helper` 和 `ch`。
- 不再接受 `switch on --mode ...` / relay diagnostics `--mode ...` 旧写法，也不再接受 `official-relay-bridge` / `official-imagegen-bridge` 作为新输入。请改用 `--preset official-relay`、`--preset official-imagegen` 或 API 字段 `patch_preset`。
- 正常启动不再隐式迁移旧配置。legacy v2/v3/v4、未标版本 TOML 和 `config.json` 需要先运行 `codex-helper config migrate --dry-run` 或 `--write --yes`。
- 新的 routing explain、usage/balance 和 route attempt DTO 不再输出 legacy station/upstream identity。调用方应使用 `provider_endpoint_key`、`provider_id` 和 `endpoint_id`；旧日志和旧快照仍可读取。

#### 修复

- 修复 provider surface、request ledger 和 CLI 过滤中的 endpoint key 兼容问题，旧记录和旧 dashboard DTO 仍可读取。
- 修复 provider control 证据不一致的问题；route attempt、request detail、provider surface 和 runtime projection 现在对同一 provider endpoint 使用一致的 signal / action 归属。
- 400 请求侧非瞬态错误现在归类为 `client_error_non_retryable`，不再计入 provider health 失败；流式 read error / idle timeout 日志也补充了更完整的 provider endpoint 信息。

### English summary

#### Added

- Added an optional TUI `5 Status` page. When `[ui.service_status]` is enabled, it can run lightweight probes per provider / endpoint or read status JSON URLs to make route availability easier to inspect.
- Added `[codex.client_patch].hosted_image_generation = "auto" | "enabled" | "disabled"`. `disabled` removes hosted image tools during Codex patching and proxied `/responses` / WebSocket forwarding, while the OpenAI Images-compatible endpoints remain available.
- Added the provider signal / policy action control loop. Rate limits, transport failures, and trusted balance exhaustion now produce route-facing evidence visible in the request ledger, admin API, TUI, and desktop client.
- Reasoning Guard now recognizes the broader `518*n-2` reasoning-token boundary pattern, such as `516/1034/1552/2070`. When enabled, it matches up to `n <= 4` by default; set `boundary_sequence_max_n = 0` to disable sequence matching.
- Reasoning Guard added `on_retry_exhausted = "pass" | "block"`. The default `pass` forwards the final upstream response after the guard retry budget is used, so helper does not interrupt the Codex task.

#### Changed

- Trusted balance exhaustion now affects routing through codex-helper-owned balance policy actions. Fresh non-exhausted balances clear only helper-owned balance actions, not manual overrides or unrelated cooldowns.
- Automatic balance probing remembers working adapters and temporarily skips recently failed or daily-exhausted adapters. After fallback, codex-helper throttles reprobes for higher-priority endpoints to avoid balance API abuse.
- The Tauri desktop client remains a source-tree preview and internal packaging validation target; v0.20.0 does not publish desktop installers.
- Dependencies were upgraded to current available releases, including cross-version compatibility work for `toml`, `toml_edit`, `rand`, `rusqlite`, and `tauri`.

#### Breaking changes

- Removed the old `codex-helper-gui` egui crate, binary entrypoint, and `gui` feature. GUI work is now in `apps/desktop` Tauri; public installs expose only `codex-helper` and `ch`.
- Removed `switch on --mode ...` / relay diagnostics `--mode ...` and the `official-relay-bridge` / `official-imagegen-bridge` input aliases. Use `--preset official-relay`, `--preset official-imagegen`, or the API field `patch_preset`.
- Normal startup no longer migrates old config files implicitly. Legacy v2/v3/v4, unversioned TOML, and `config.json` must be migrated with `codex-helper config migrate --dry-run` or `--write --yes`.
- New routing explain, usage/balance, and route attempt DTOs no longer emit legacy station/upstream identity. Use `provider_endpoint_key`, `provider_id`, and `endpoint_id`; old logs and snapshots remain readable.

#### Fixed

- Fixed endpoint-key compatibility in provider surfaces, request ledger reads, and CLI filters so old records and old dashboard DTOs remain readable.
- Fixed provider control evidence consistency: route attempts, request details, provider surfaces, and runtime projections now agree on signal / action ownership for the same provider endpoint.
- Non-transient client-side 400 responses are now `client_error_non_retryable` and stay health-neutral. Stream read errors and idle timeouts now include clearer provider endpoint diagnostics.

## [0.19.0] - 2026-06-29

### 中文

#### 新增

- 新增可配置的 Reasoning Guard，用于拦截 Codex 中转偶发的异常短推理路径。核心配置在 `[retry.reasoning_guard]`：
  `enabled`、`reasoning_equals`、`action`、`stream_mode`、`max_guard_retries`、`paths`、`log_matches`。默认异常桶为 `[516, 1034, 1552]`，配置支持运行时热加载，新请求会自动使用最新设置。
- 新增 Codex 客户端压缩策略配置 `[codex.client_patch].compaction = "auto" | "local" | "remote-v1" | "remote-v2"`，并支持 `codex-helper switch on --compaction ...`。这让 official relay / imagegen 预设可以按中转能力选择本地压缩、remote compact v1 或 remote compact v2。
- OpenAI Images 兼容入口增强：支持 JSON `POST /v1/images/edits` / `/images/edits` 参考图生成，并对 hosted `image_generation_call` 结果做语义校验，避免上游返回 HTTP 200 但没有图片结果时被误判为成功。
- TUI Settings 页现在会显示当前 reasoning guard 规则；Relay 能力诊断也会按实际 compaction 策略计算 expected / mismatch。

#### 修复

- 修复 Image API 模型被直接放进 `/v1/responses` 顶层 `model` 后导致路由池误判的问题；现在会映射到可配置的 Responses wrapper model，并强制调用 hosted `image_generation`。
- 图像入口的路由失败现在会返回更明确的 `failure_hint`、`request_id` 和 `suggested_action`，`ch-imagegen` 也会把失败归因到 provider / 路由池，而不是误导为分辨率问题。
- Codex `remote_compaction_v2` 请求会在中转不支持 v2 时自动降级到 `/responses/compact` 并合成 Codex 期望的 v2 compact SSE；可用 `[codex.compaction].remote_v2_downgrade = false` 关闭。
- 修复 TUI 依赖组合在新版 Rust 上可能触发的 `ratatui-widgets` / `time` 编译冲突。

### English summary

#### Added

- Added configurable Reasoning Guard protection for anomalous Codex relay short-reasoning paths. The new `[retry.reasoning_guard]` options are `enabled`, `reasoning_equals`, `action`, `stream_mode`, `max_guard_retries`, `paths`, and `log_matches`. The default anomaly buckets are `[516, 1034, 1552]`, and runtime config reload applies to new requests.
- Added `[codex.client_patch].compaction = "auto" | "local" | "remote-v1" | "remote-v2"` plus `codex-helper switch on --compaction ...`, so official relay / imagegen presets can match the relay's real local/remote compaction support.
- Improved OpenAI Images-compatible endpoints with JSON image edits support and hosted `image_generation_call` result validation, preventing HTTP 200 responses without image results from being recorded as successful generations.
- The TUI Settings page now shows the active reasoning guard rule, and relay diagnostics evaluate expected capability / mismatch output with the selected compaction strategy.

#### Fixed

- Fixed Image API model intents being routed as top-level `/v1/responses` models. They are now mapped to a configurable Responses wrapper model and forced through hosted `image_generation`.
- Image route failures now include clearer `failure_hint`, `request_id`, and `suggested_action` fields; `ch-imagegen` reports provider / route-pool problems instead of mislabeling them as resolution failures.
- Codex `remote_compaction_v2` requests now downgrade to `/responses/compact` when the relay cannot produce valid v2 compact output. Set `[codex.compaction].remote_v2_downgrade = false` to disable this fallback.
- Fixed a TUI dependency combination that could fail to compile on newer Rust due to `ratatui-widgets` / `time` coherence conflicts.

## [0.18.0] - 2026-05-31

### 中文

#### 新增

- 新增仓库内 `codex-session-diagnostics` skill，可按 Codex session key 只读收集 `~/.codex-helper` 日志/状态/配置和 `~/.codex` 会话 JSONL，辅助定位 waiting、resume、stream、routing affinity 和 relay 连续性问题。
- 新增容器优先的中央 relay runtime：`codex-helper-server`、cargo-chef Dockerfile、Synology-friendly Compose 示例、容器 server 配置和 Docker 部署文档。容器启动 proxy/admin API，不会 patch 宿主机的 `~/.codex/config.toml` 或 `auth.json`。
- 新增本机保存的 relay target 工作流：`ch relay add/list/status/off/use` 和短入口 `ch relay <target>`。`local` 是内置本机 target；命名 target 可指向 NAS、Tailscale 或 LAN 上的 helper runtime，并支持 `--no-tui` switch-only、`--attach-only` observe-only，以及只保存环境变量名的 `admin_token_env`。
- 新增 GHCR Docker 发布 workflow：`v*` tag、GitHub Release 发布和手动 dispatch 可构建/发布 `ghcr.io/<owner>/codex-helper-server`，PR 只做 Docker build/smoke，不推送镜像；稳定 tag 会额外发布 `latest`，预发布 tag 不会覆盖 `latest`。

#### 变更

- 本地桌面 CLI 和容器 server 的职责边界进一步拆开：本地 `ch`、`serve`、`switch` 仍负责本机 client patch 和 TUI 生命周期；容器/server runtime 只暴露 proxy/admin control-plane。远端 attached TUI 现在通过 resolved admin URL 和可选 admin token env 观察目标 proxy，不再假设 admin API 一定在本机 loopback。
- control-plane 的 `station/config` 语义收口到 station-first 口径；GUI/TUI/tray/请求详情不再把默认站点、上次观测站点或旧 route-attempt 投影显示成 `active_station` / `legacy` / `config` 文案，`operator/summary` 回归断言也补强为显式拒绝旧 session-card、link 和 capability key。
- TUI 第 5 页用户可见名称统一为 `Usage / 用量` 口径；Recent/History 都明确标注为 Codex 全局会话，Recent 页 footer 也补齐 `s/f/h` 跳转提示。Usage / Balance 预测现在会显示样本来源来自当前 runtime 还是本地 request ledger，Requests 页在从 Codex 历史会话跳入且当前 runtime 未观测到请求时会给出明确空态说明，避免启动后把历史数据误认成当前会话请求。

#### 修复

- 修复 Codex `/responses` / `/responses/compact` 流式请求在上游已返回 HTTP 200 但后续 SSE body 长时间无字节时会无限 waiting 的问题；现在默认 900 秒 idle watchdog 会用 Codex 可解析的 `response.failed` 结束流，并在日志中记录 `codex_helper_error=upstream_stream_idle_timeout`。可用 `CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS=0` 关闭，或设置秒数覆盖默认值。
- TUI 的 Recent/History/Requests/Sessions 现在先在 `UiState` 里同步选择和表格状态，再交给 render 消费；Usage 预测样本来源也改成显式模型，不再通过 `Vec` 长度推断是否带上本地 request ledger。
- 修复交互式 TUI/runtime 日志只在启动时检查大小的问题；`runtime.log` 现在会在运行过程中按 `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES` 持续轮转，并且升级后会在下次启动时清理超过保留预算的历史 `runtime.log.*`，避免老用户遗留的巨型日志继续占用磁盘。
- 修复轮转日志清理在 Windows 上遇到占用或删除失败时会把文件大小误算为已释放的问题；删除失败的 `runtime.log.*` / `control_trace.*.jsonl` 会继续保留预算压力，清理会尝试后续候选，并在下次 repair 时重试，避免旧用户升级后仍残留超大轮转文件。
- 将 runtime、GUI、request/debug、control trace、retry trace 和 Codex relay evidence 统一到有界本地日志存储；`control_trace.jsonl` 等 JSONL 日志现在会在首次写入时按 `CODEX_HELPER_REQUEST_LOG_MAX_BYTES` / `CODEX_HELPER_REQUEST_LOG_MAX_FILES` 轮转并清理历史轮转文件，`gui.log` 和 relay evidence 也新增独立大小上限，降低老用户日志目录继续膨胀的风险。
- request ledger、control trace 和 Codex relay evidence 的读取入口现在也会先执行同一套有界日志修复；老用户升级后即使先打开 GUI/管理 API/CLI 查看日志、尚未产生新的写入，遗留的超大 active JSONL 也会按保留策略轮转清理，避免读取最近记录时完整扫描巨型文件。
- TUI/管理 API 的 Sessions 列表不再把仅由持久化 route affinity 恢复出来的旧 session 当作当前运行期已观测会话展示；恢复的 affinity 仍保留用于后续 remote compaction 连续性，但只有 session 被当前运行期请求、统计或显式 override 触达后才会显示。

#### 重构

- 收口 route graph 与 legacy routing compat 的 authoring 边界；CLI、GUI 和 admin API 现在通过 `RoutingConfigV4` / `ServiceViewV4` 的语义方法更新 entry route、provider 引用和手动 target，而不是在调用点手动修改字段后同步兼容字段。
- 新增 `RequestLedgerStore` 作为 request ledger 读模型边界；CLI、TUI、GUI 和 admin API 现在通过统一 store 读取 tail、filter 和 summary，最近记录与过滤查询改为流式保留窗口，避免只为读取最近 N 条就加载完整 `requests.jsonl`。
- 拆分 Codex relay live-smoke case registry；case 描述、HTTP spec 和诊断请求体迁移到独立 `codex_relay_live_smoke::cases` 模块，主模块保留 proxy orchestration、transport 和 response classification。

### English summary

#### Added

- Added the repository `codex-session-diagnostics` skill for read-only collection of `~/.codex-helper` logs/state/config and `~/.codex` session JSONL files from a Codex session key, helping diagnose waiting, resume, stream, routing-affinity, and relay-continuity failures.
- Added a container-first central relay runtime: `codex-helper-server`, a cargo-chef Dockerfile, Synology-friendly Compose samples, container server config, and Docker deployment docs. The container starts only the proxy/admin APIs and does not patch the host machine's `~/.codex/config.toml` or `auth.json`.
- Added client-side relay targets: `ch relay add/list/status/off/use` and the shorthand `ch relay <target>`. `local` is the built-in local target; named targets can point at NAS, Tailscale, or LAN helper runtimes with `--no-tui` switch-only, `--attach-only` observe-only, and `admin_token_env` storing only the token environment variable name.
- Added a GHCR Docker publishing workflow. `v*` tags, published GitHub Releases, and manual dispatches can build/publish `ghcr.io/<owner>/codex-helper-server`; PRs perform Docker build/smoke only. Stable tags also publish `latest`, while prerelease tags do not overwrite `latest`.

#### Fixed

- Fixed Codex `/responses` / `/responses/compact` streams waiting forever when an upstream returns HTTP 200 and then stops producing SSE body bytes. A 900-second idle watchdog now finishes the client stream with a Codex-parseable `response.failed` event and logs `codex_helper_error=upstream_stream_idle_timeout`; set `CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS=0` to disable it or set a custom timeout in seconds.
- TUI Recent/History/Requests/Sessions now sync selection and table state inside `UiState` before render consumes them; Usage forecast sample provenance is now explicit instead of inferred from `Vec` length.
- Fixed interactive TUI/runtime log rotation only checking file size at startup. `runtime.log` now rotates while the process is running according to `CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES`, and upgrades clean up historical `runtime.log.*` files that exceed the retention budget on the next startup so oversized legacy logs do not keep consuming disk space.
- Fixed bounded-log pruning accounting when Windows cannot delete a rotated file because it is still open. Failed deletes no longer count as recovered budget, later rotated candidates are still pruned, and the oversized file is retried on the next repair.
- Unified runtime, GUI, request/debug, control trace, retry trace, and Codex relay evidence writes behind a bounded local log store. JSONL logs such as `control_trace.jsonl` now rotate and prune historical rotated files on first write according to `CODEX_HELPER_REQUEST_LOG_MAX_BYTES` / `CODEX_HELPER_REQUEST_LOG_MAX_FILES`, while `gui.log` and relay evidence gained their own size limits to reduce continued log directory growth for existing users.
- Request ledger, control trace, and Codex relay evidence readers now run the same bounded-log repair before scanning. Existing users who open the GUI, admin API, or CLI before a new write is produced will still have oversized active JSONL logs rotated and pruned instead of fully scanned for recent records.
- TUI/admin Sessions no longer display old sessions that were restored only from persisted route affinity. The restored affinity is still kept for later remote-compaction continuity, but a session is shown only after the current runtime observes requests, stats, or explicit overrides for it.

#### Changed

- Split the local desktop CLI responsibilities from the container server runtime. Local `ch`, `serve`, and `switch` continue to own local client patching and TUI lifecycle, while the container/server runtime exposes only proxy/admin control-plane APIs. Remote attached TUI now observes a target proxy through a resolved admin URL and optional admin-token environment variable instead of assuming loopback admin access.
- Closed the control-plane `station/config` semantic tail around station-first wording. GUI/TUI/tray/request-detail surfaces no longer present default stations, last observed stations, or legacy route-attempt projections as `active_station` / `legacy` / `config` labels, and `operator/summary` regressions now explicitly reject old session-card, link, and capability keys.
- Standardized the TUI page-5 user-facing label around `Usage` / `Usage / Balance`. Recent and History now both identify their Codex-global session scope, the Recent footer advertises the `s/f/h` navigation keys, Usage / Balance spend forecasts show whether their sample comes from the current runtime or the local request ledger, and Requests explains when a focused Codex-history session has no requests observed by the current runtime.
- Consolidated the route graph and legacy routing compatibility authoring boundary. CLI, GUI, and admin API callers now update entry routes, provider references, and manual targets through semantic `RoutingConfigV4` / `ServiceViewV4` methods instead of mutating fields and synchronizing compatibility state at each call site.
- Added `RequestLedgerStore` as the request ledger read-model boundary. CLI, TUI, GUI, and admin API consumers now read tail, filter, and summary data through one store, and recent/filter queries use a streaming bounded window instead of loading the full `requests.jsonl` just to return the newest records.
- Split the Codex relay live-smoke case registry. Case descriptors, HTTP specs, and diagnostic request bodies now live in a dedicated `codex_relay_live_smoke::cases` module while the main module keeps proxy orchestration, transport, and response classification.

## [0.17.0] - 2026-05-26

### 中文

#### Codex 请求/响应语义增强

- 本地代理新增 OpenAI Images 兼容入口 `POST /v1/images/generations` / `/images/generations`，会把 `model`、`prompt`、`size`、`output_format`、`quality` 等字段转成 `/v1/responses` hosted `image_generation` 请求，并继续复用既有 provider routing、重试、fallback、auth 注入和请求日志。成功响应会转换为 `data[0].b64_json`，当前仅支持单图 `n=1`。
- Codex `/responses`、`/responses/compact` 和 Responses WebSocket 请求现在会从已有请求证据补齐缺失的 `session_id`、`x-session-id`、官方 `session-id` / `thread-id` 和 `prompt_cache_key`；来源包括 header session、body `session_id`、`prompt_cache_key` 和 `metadata.session_id`。`previous_response_id` 只用于 stale-response 修复，不再作为 session completion 或 session identity 来源。helper 不会凭空生成 session id，也不会覆盖客户端已发送的 session 字段。
- Codex `/responses/compact` 现在会先尝试已存在的 session affinity；在单 provider endpoint 或 `fallback-sticky` route graph 下，state-bound compact 不再仅因为缺少既有 route affinity 被本地 503 拦截，helper 会按配置尝试可用 provider endpoint，并在成功后记录 session affinity。如果 affinity 账号不可用，`fallback-sticky` 可继续按路由策略尝试其它可用账号；`hard` affinity 和 legacy 多 upstream 仍保持 fail-closed，避免盲目跨账号搬迁 compact state。
- Codex `/responses/compact` 现在会保留官方 compaction 输入里的 `service_tier` 和 `prompt_cache_key`，转发体会去掉 `previous_response_id` 以贴近官方 compact 形状；如果请求体里带有 `encrypted_content` 或 `previous_response_id` 这类状态字段，就会把 compact 视为 state-bound 请求，避免跨账号兜底把 502 误变成上游状态错误。
- Session route affinity 现在会以 provider endpoint identity 形式持久化到 helper state。helper 重启后，Codex remote compaction 会继续使用之前已证明的 provider endpoint；如果当前策略要求已知 affinity 但 state-bound compact 缺少可恢复 affinity，会返回明确连续性错误，而不是静默切到另一个 provider。
- Codex remote compaction v2 现在会在普通 `POST /responses` 请求里识别结构化 `compaction_trigger`，并在日志中写入 `codex_bridge.remote_compaction_v2_request`。v2 compact 会按 provider-state-bound 处理，并遵守和 v1 compact 相同的 route affinity policy：`fallback-sticky` 可引导或更新 affinity，`hard` 会限制在 affinity continuity domain 内。helper 不会假设背后 relay 是 sub2api、OpenAI、New API 或其它实现。
- 新增显式 `continuity_domain` 连续性边界，可配置在 provider 或 endpoint 上，并会出现在 relay capability diagnostics 和桌面 provider 编辑里；普通 provider 编辑也会保留已有 `continuity_domain`，不会因为改 base URL 或启停状态而意外清空。只有显式共享同一个 `continuity_domain` 的 endpoints，才允许 state-bound compact 在已有 affinity 后跨 endpoint failover；helper 不会再根据相同 `base_url`、host 或 provider 名推断共享状态。
- Responses WebSocket 的 state-bound compact 选路现在和 HTTP compact 对齐：`fallback-sticky` 可以引导 affinity；`hard` 会限制在 affinity continuity domain 内，并且只在显式共享 `continuity_domain` 的 endpoints 之间故障切换。
- Relay live smoke 新增显式 `remote_compaction_v2` / `--compact-v2` case：会发送真实 `/responses` stream 请求、带 `compaction_trigger` 和 `x-codex-beta-features: remote_compaction_v2`，只有看到一个 compaction output item 和 `response.completed` 才算通过；默认 live smoke 仍只测 `/responses/compact`。
- Control trace 新增 provider-opaque 的连续性诊断字段：continuity class、affinity source、provider failover 是否允许、阻断原因，以及余额信号是否对该连续性决策具有权威性。helper 不会从这些信号推断 relay 背后是 OpenAI、sub2api、New API 或其它实现。
- Codex routing explain 现在会和实际执行路径一致：HTTP、Responses WebSocket 和 legacy 路径都会应用同一套 session route affinity，legacy explain 不再因为遗漏 session affinity 而和真实路由结果不一致。
- 如果上游 400/404 明确表示 `previous_response_id` 对应 response 不存在，helper 会移除 `previous_response_id` 并对同一个上游重试一次，同时在 route attempts 中记录 `codex_stale_previous_response_id`，方便排查 relay 状态不同步。
- Codex 非流式响应新增受控 gzip JSON 修复：当 relay 无视 `Accept-Encoding: identity` 返回 gzip JSON 时，helper 会解压后转发普通 JSON，并继续复用现有响应头过滤去掉过期的 `Content-Encoding` / `Content-Length`。
- 当 Codex stream 请求在选路前失败（例如所有候选都因余额耗尽、cooldown 或无可路由目标被阻断）时，helper 会返回 Codex 可解析的 `response.failed` SSE，而不是裸 HTTP 错误，避免客户端流式解析卡在异常形态上。
- `service_tier` 观测补齐代理级回归测试：请求日志会保留 requested / effective / actual 三段，确认 fast mode 仍只由客户端或显式 override 决定，不由 helper 默认配置偷偷改写。
- OpenAI 风格 `/models` 到 Codex `models` catalog 的翻译改为显式 `translate_models = true` 开关；启用时会补充 image/search/apply_patch、context window 和 fast `service_tier` 等 metadata，并可叠加 Basellm 模型 metadata，但默认不再把合成目录当作权威返回给 Codex。
- OpenAI Images 兼容入口现在会在转发 hosted image generation 请求前去掉客户端 `User-Agent`，避免部分上游把本地脚本或 Codex 客户端 UA 当成能力/风控信号。
- 新增仓库分发的 `ch-imagegen` skill：通过本地 `/v1/images/generations` 入口生成图片，自动计算 `gpt-image-2` 的 2K/4K 尺寸，保存并校验新生成的文件。

#### Codex 客户端与 TUI

- `codex-helper switch on` 不带显式 preset 时现在会读取 `[codex.client_patch]` 配置，正确应用配置中的 `preset`、`responses_websocket` 等选项；显式 `--preset` 仍可覆盖配置。
- TUI transcript 弹窗现在可以按实际换行后的内容滚动，长消息折行后不再出现无法滚到被包裹行的问题。

#### Codex 中转请求字段覆盖

- 修复自动 `default_profile` 会把客户端请求体里的 `model`、`reasoning.effort` 和 `service_tier` 改成 profile 默认值的问题；现在只有显式 session override 或手动 apply 到 session 的 profile binding 才会改写这些请求字段。
- Codex 本地 fast mode 发出的 `service_tier: "priority"` 不会再被中转的默认 profile 覆盖成 `default`。自动 default profile 仍可用于 session binding 和 station 路由，但不再冒充用户请求字段 override。

#### 发布包

- cargo-dist release 包现在只包含 `codex-helper` CLI package，避免把 desktop package 混入命令行发行物。

### English summary

#### Codex request/response semantics

- Added an OpenAI Images-compatible local proxy entrypoint: `POST /v1/images/generations` / `/images/generations`. It maps `model`, `prompt`, `size`, `output_format`, `quality`, and related fields into a `/v1/responses` hosted `image_generation` request while preserving existing provider routing, retry, fallback, auth injection, and request logging. Successful responses are converted to `data[0].b64_json`; the first version supports single-image `n=1` only.
- Codex `/responses`, `/responses/compact`, and Responses WebSocket requests now complete missing `session_id`, `x-session-id`, official `session-id` / `thread-id`, and `prompt_cache_key` fields from existing request evidence: header session ids, body `session_id`, `prompt_cache_key`, or `metadata.session_id`. `previous_response_id` is only used for stale-response repair, not as a session completion or session identity source. Helper does not invent synthetic session ids or overwrite client-provided session fields.
- Codex `/responses/compact` now tries existing session affinity first. With a single provider endpoint or a `fallback-sticky` route graph, state-bound compact is no longer blocked locally with a 503 solely because no prior route affinity exists; helper tries the configured provider endpoint path and records session affinity after success. If the affinity account is unavailable, `fallback-sticky` can continue through other available accounts according to routing policy; `hard` affinity and legacy multi-upstream routing remain fail-closed to avoid blindly moving compact state across accounts.
- Codex `/responses/compact` now preserves the official input fields `service_tier` and `prompt_cache_key`, strips `previous_response_id` from the forwarded payload to match the official compact shape, and treats requests carrying `encrypted_content` or `previous_response_id` as state-bound so compact fallback does not cross accounts when upstream state might be pinned to one relay identity.
- Session route affinity is now persisted by provider endpoint identity under helper state. After a helper restart, Codex remote compaction keeps using the previously proven provider endpoint; when the active policy requires known affinity and a state-bound compact request has no restorable affinity, helper returns an explicit continuity error instead of silently moving to another provider.
- Codex remote compaction v2 is now recognized on ordinary `POST /responses` requests with a structured `compaction_trigger` input item and logged as `codex_bridge.remote_compaction_v2_request`. V2 compact is treated as provider-state-bound and follows the same route affinity policy as v1 compact: `fallback-sticky` can bootstrap or update affinity, while `hard` stays inside the affinity continuity domain. Helper does not infer whether the relay backend is sub2api, OpenAI, New API, or something else.
- Added an explicit `continuity_domain` boundary on providers and endpoints, surfaced in relay capability diagnostics and desktop provider editing; ordinary provider edits also preserve existing `continuity_domain` values instead of clearing them while changing base URLs or enabled state. State-bound compact can fail over across endpoints after known affinity only when those endpoints explicitly share the same `continuity_domain`; helper does not infer shared state from matching `base_url`, host, or provider names.
- Responses WebSocket state-bound compact routing now matches HTTP compact routing: `fallback-sticky` can bootstrap affinity, while `hard` stays inside the affinity continuity domain and only fails over across endpoints with an explicit shared `continuity_domain`.
- Relay live smoke now has an explicit `remote_compaction_v2` / `--compact-v2` case. It sends a real streaming `/responses` request with `compaction_trigger` and `x-codex-beta-features: remote_compaction_v2`, and only passes after one compaction output item plus `response.completed`; the default live smoke still checks only `/responses/compact`.
- Control trace now includes provider-opaque continuity diagnostics: continuity class, affinity source, whether provider failover is allowed, the blocked reason, and whether balance signals are authoritative for that continuity decision. Helper does not infer whether the relay backend is OpenAI, sub2api, New API, or something else.
- Codex routing explain now matches the actual execution path: HTTP, Responses WebSocket, and legacy paths all apply the same session route affinity, so legacy explain no longer diverges from the real route selection.
- If an upstream 400/404 explicitly says the `previous_response_id` response no longer exists, helper removes `previous_response_id` and retries the same upstream once. Route attempts record `codex_stale_previous_response_id` for relay-state debugging.
- Added bounded gzip JSON repair for non-streaming Codex responses. If a relay ignores `Accept-Encoding: identity` and returns gzip JSON, helper decodes it before forwarding plain JSON and keeps filtering stale `Content-Encoding` / `Content-Length`.
- When Codex streaming requests fail before an upstream is selected, such as all candidates being blocked by trusted balance exhaustion, cooldown, or no routable target, helper now returns a Codex-parseable `response.failed` SSE instead of a bare HTTP error.
- Added proxy-level regression coverage for `service_tier` attribution: request logs preserve requested / effective / actual values, confirming fast mode is driven by the client or explicit overrides, not by helper defaults.
- OpenAI-style `/models` to Codex `models` catalog translation is now behind the explicit `translate_models = true` switch. When enabled, helper adds metadata such as image/search/apply_patch support, context windows, and fast `service_tier`, with optional Basellm metadata overlay, but it no longer treats synthesized catalogs as authoritative by default.
- The OpenAI Images-compatible entrypoint now strips the client `User-Agent` before forwarding hosted image generation requests, avoiding relays that treat local script or Codex client UAs as capability or policy signals.
- Added the repository-distributed `ch-imagegen` skill. It calls the local `/v1/images/generations` entrypoint, computes valid `gpt-image-2` 2K/4K sizes, saves the generated file, and validates only the new output.

#### Codex client and TUI

- `codex-helper switch on` without an explicit preset now reads `[codex.client_patch]` and applies configured `preset`, `responses_websocket`, and related options; explicit `--preset` still overrides config.
- TUI transcript modals now scroll by rendered wrapped lines, so long wrapped messages no longer hide unreachable content.

#### Codex relay request-field overrides

- Fixed automatic `default_profile` bindings rewriting client request-body `model`, `reasoning.effort`, and `service_tier`; only explicit session overrides or manually applied session profile bindings now patch those fields.
- Codex local fast mode requests with `service_tier: "priority"` are no longer overwritten to `default` by relay default profiles. Automatic default profiles can still bind sessions and station routing, but no longer act as request-field overrides.

#### Release packaging

- cargo-dist release artifacts now include only the `codex-helper` CLI package, avoiding accidental desktop package inclusion in the command-line distribution.

## [0.16.0] - 2026-05-19

### 中文

#### Codex 中转和 ChatGPT 原生体验

- Codex 本地代理现在会默认归一化请求体 `Content-Encoding`（`zstd`、`gzip` / `x-gzip`、`br`、`deflate`），在路由、model override 和转发前得到普通 JSON，并移除过期的 `Content-Encoding` / `Content-Length`；极少数需要原始压缩体的中转可用 `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` 启动 helper 作为逃生口。
- 当请求缺少 `session_id` / `session-id` / `conversation_id` / `thread-id` 这类强 session header 时，helper 会把已解码 JSON 的 `prompt_cache_key` 作为 session affinity 兜底，让 `/responses` 与 `/responses/compact` 在多中转路由下保持 sub2api 风格粘性。
- 新增 Codex client preset 体系，用来处理“Codex 客户端想要官方/ChatGPT 形态，但模型流量要走中转”的场景。用户侧推荐使用 `[codex.client_patch].preset = "..."` 和 `codex-helper switch on --preset ...`；旧配置 `mode = "..."` 和 CLI `--mode ...` 仍兼容读取，但 helper 保存/生成配置时统一写 `preset`。
- `chatgpt-bridge` 适合已经在官方 Codex 登录 ChatGPT 的用户。Codex 仍看到 ChatGPT 登录态，桌面端和手机端账号能力可以继续按官方路径判断；模型请求由 codex-helper 路由到你的 relay。
- `imagegen-bridge` 适合 relay 不支持 official provider 身份，但你想让 Codex 暴露 hosted `image_generation` 的场景。它会写入 `{}` auth facade，真实上游密钥仍来自 codex-helper 配置。
- `official-relay` 适合能转发 OpenAI Responses 语义的中转，尤其是支持 `/responses/compact` 的 sub2api 或类似服务。它让 Codex 尝试 remote compaction v1。
- `official-imagegen` 适合背后确实是官方订阅账号、同时支持 `/responses/compact` 和 hosted image generation 的 relay。它同时给 Codex official provider 身份和 imagegen facade。
- 旧 preset 名称 `official-relay-bridge` / `official-imagegen-bridge` 仍作为 alias 接受，但不再作为推荐写法；未发布过的旧 `official-ws-*` 组合已删除。
- 新增独立传输开关 `responses_websocket = true` / `--responses-websocket`。它不是新的 preset，只允许搭配 `official-relay` 或 `official-imagegen`，启用后 helper 会写入 `supports_websockets = true` 并代理 Responses WebSocket v2。
- bridge 模式不会把 Codex 的 ChatGPT token 透传给没有 helper 侧密钥的第三方 relay。上游应配置自己的 `auth_token_env`、`auth_token`、`api_key_env` 或 `api_key`。

#### 中转能力诊断

- 新增 Codex relay 能力诊断，可从 TUI Settings、admin API 或 CLI 运行。常用 CLI：`codex-helper codex relay-capabilities --preset official-imagegen --model gpt-5.5`。
- 诊断会检查 relay 的 `/models`、`/responses`、`/responses/compact`，然后给出更适合的 preset。它不会自动切换配置。
- 诊断和 live smoke 支持 provider/endpoint 定向：CLI 可用 `--provider <ID>`、`--endpoint <ID>`，便于直接验证某个中转，而不是只测当前路由首选项。
- 如果 relay 返回 OpenAI 风格的 `/models`（`data: [...]`），codex-helper 会翻译成 Codex 需要的 `models: [...]` catalog，避免 Codex 因模型 metadata 形态不对而看不到 image/search/apply_patch 等能力。
- 新增需要明确确认的 live smoke：`codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5`。默认只测 `/responses/compact`；加 `--image` 只测 hosted image generation，加 `--websocket` 只测 Responses WebSocket v2，同时传多个可选 case 时只跑显式指定的 case。这个命令会打真实上游，可能消耗额度、生成图片或建立 WebSocket 会话。
- 诊断和 live smoke 会写入 `~/.codex-helper/logs/codex_relay_evidence.jsonl`。这是给人看的本地证据，不参与 routing、affinity、health、余额、retry 或自动切换 preset。

#### 多中转路由和余额

- Codex 多中转默认更偏向“稳定粘住当前可用上游”。对 official relay、remote compaction 这类可能带上游账号绑定状态的请求，建议使用 `[codex.routing].affinity_policy = "fallback-sticky"`。
- 当所有候选都因为可信余额耗尽或 cooldown 被挡住时，Codex streaming 请求会收到可重试的 `response.failed`，helper 会排队一次受节流的余额刷新，而不是每秒反复打同一个已耗尽上游。
- 如果某个中转余额接口经常把可用账号报成 0，把对应 usage provider 的 `trust_exhaustion_for_routing` 设为 `false`，让余额只作为提示，不再驱动路由降级。
- 请求触发的余额刷新现在按 provider/endpoint 去重和延迟刷新；手动刷新也能命中自动探测到的 sub2api provider id。

#### TUI/GUI 可见变化

- TUI 的 `Stats` 改为 `Usage`，集中看 provider 用量、余额/配额、刷新结果、endpoint 最近样本和费用估算。
- `Usage` 页面可以按 `g` 刷新余额，按 `a` 只看需要处理的 provider；provider 很多时详情页支持 `PgUp` / `PgDn` 滚动 endpoint。
- 窄终端下 Routing/Usage 的余额显示更稳，不再容易出现 `$0/$` 这类半截金额。Routing 页保留紧凑余额，详细分析放在 Usage/Balance。
- GUI 的余额状态和 core 语义对齐，`unknown`、`stale`、`exhausted`、`error`、`unlimited` 不再由不同 UI 各算一套。
- Usage 的 burn forecast 现在会合并请求 ledger tail 和 recent samples，并在请求日志里补充 model / route decision 信息，避免高频请求场景下 burn rate 和到 0 点预估明显偏低。

#### 升级时关注

- README 现在把 cargo-dist 生成的预编译 shell / PowerShell installer 作为推荐安装入口；`cargo-binstall` 仍保留给 Rust 用户。
- 常用 client preset 放在 `[codex.client_patch] preset = "..."`，也可以用 `codex-helper switch on --preset ...` 显式切换。旧的 `mode` / `--mode` 仍可读，但建议迁移到 `preset`。改完后需要完整重启 Codex App、TUI 或 `codex exec`。
- route graph 的默认 affinity policy 回到 `fallback-sticky`，新配置会倾向同一 session 粘住已成功的 fallback provider。若你更希望故障恢复后尽快回到最高优先级 provider，请显式设置 `[codex.routing].affinity_policy = "preferred-group"`。
- 启动时会先加载并规范化 `~/.codex-helper/config.toml`，再 patch Codex；如果 `~/.codex/config.toml` 还保留旧的本地代理但缺少 switch state，会以 codex-helper 配置里的 `preset` 为准重新 patch。
- 如果想启用 Responses WebSocket，请额外设置 `responses_websocket = true` 或传 `--responses-websocket`；它不是 preset，只建议在已验证支持 WebSocket 的上游上开启。例如实测 `input8` 支持，`ciii` 的 HTTP endpoints 可用但 WebSocket upstream/proxy 失败，应保持关闭。
- relay 要求模型名前缀时，用 `provider add --model-map FROM=TO` 或 provider 级 `model_mapping`，例如 `gpt-5.5` -> `openai/gpt-5.5`。
- 不确定 relay 是否支持 compact/imagegen 时，先跑 `codex-helper codex relay-capabilities`；只有要真实验证上游链路时再跑 live smoke。
- 手机远程控制仍走 `codex-helper switch remote-control ...`，它和 `chatgpt-bridge` 是两条路径。

### English summary

#### Codex relays and native ChatGPT behavior

- The local Codex proxy now normalizes request-body `Content-Encoding` by default (`zstd`, `gzip` / `x-gzip`, `br`, and `deflate`) before routing, model overrides, and forwarding, then removes stale `Content-Encoding` / `Content-Length`; rare relays that require the original compressed body can start helper with `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough`.
- When no stronger session header is present (`session_id`, `session-id`, `conversation_id`, or `thread-id`), helper uses decoded JSON `prompt_cache_key` as the session-affinity fallback so `/responses` and `/responses/compact` keep sub2api-style stickiness across relay routing.
- Added Codex client presets for setups where Codex should keep an official or ChatGPT-like client shape while model traffic goes through codex-helper relays. Use `[codex.client_patch].preset = "..."` and `codex-helper switch on --preset ...`; legacy `mode = "..."` and CLI `--mode ...` are still accepted, but helper writes saved/generated config as `preset`.
- `chatgpt-bridge` is for users already signed in to ChatGPT in official Codex. Codex keeps seeing ChatGPT account state, while model requests are routed through codex-helper.
- `imagegen-bridge` exposes hosted `image_generation` for relays that do not support official provider identity. It writes the empty `{}` auth facade; real upstream credentials still come from helper config.
- `official-relay` is for relays that forward OpenAI Responses semantics, especially `/responses/compact`. It lets Codex try remote compaction v1.
- `official-imagegen` is for official-subscription-backed relays that support both `/responses/compact` and hosted image generation.
- Legacy preset names `official-relay-bridge` / `official-imagegen-bridge` remain accepted as aliases, but are no longer the recommended spelling; unpublished `official-ws-*` combinations were removed.
- Added the separate transport switch `responses_websocket = true` / `--responses-websocket`. It is not a new preset, is only valid with `official-relay` or `official-imagegen`, and makes helper advertise `supports_websockets = true` while proxying Responses WebSocket v2.
- Bridge modes do not forward Codex ChatGPT tokens to third-party relays without helper-side credentials. Configure upstream secrets with `auth_token_env`, `auth_token`, `api_key_env`, or `api_key`.

#### Relay diagnostics

- Added Codex relay capability diagnostics in TUI Settings, admin API, and CLI. Common CLI: `codex-helper codex relay-capabilities --preset official-imagegen --model gpt-5.5`.
- Diagnostics check `/models`, `/responses`, and `/responses/compact`, then recommend a preset. They do not switch presets automatically.
- Diagnostics and live smoke can target a specific provider/endpoint with `--provider <ID>` and `--endpoint <ID>`, making it possible to verify one relay directly instead of only the current routing preference.
- OpenAI-style `/models` responses (`data: [...]`) are translated into the Codex `models: [...]` catalog so Codex can see model metadata such as image/search/apply_patch support.
- Added explicit live smoke: `codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5`. It checks compact by default; `--image` tests only hosted image generation, `--websocket` tests only Responses WebSocket v2, and multiple optional cases run only the explicitly requested cases. This sends real upstream requests and may spend quota, create an image, or open a WebSocket session.
- Diagnostics and live smoke append sanitized records to `~/.codex-helper/logs/codex_relay_evidence.jsonl`. That file is local diagnostic evidence only; it does not affect routing, affinity, health, balance, retry, or automatic preset switching.

#### Multi-relay routing and balance

- Codex multi-relay routing now favors keeping a viable selected upstream stable. For official relay and remote compaction setups, use `[codex.routing].affinity_policy = "fallback-sticky"` when upstream-account continuity matters.
- When every candidate is blocked by trusted balance exhaustion or cooldown, Codex streaming gets a retryable `response.failed` and helper queues a throttled balance refresh instead of hammering the same exhausted upstream.
- If a relay balance API reports false zero balance, set that usage provider's `trust_exhaustion_for_routing` to `false` so balance stays informational and no longer demotes routing.
- Request-triggered balance refreshes are deduplicated by provider/endpoint and delayed; manual refresh can also target auto-discovered sub2api provider ids.

#### TUI and GUI

- TUI `Stats` is now `Usage`, focused on provider usage, balance/quota state, refresh results, recent endpoint samples, and cost estimates.
- On `Usage`, press `g` to refresh balances and `a` to show only providers that need attention. Large endpoint lists can be scrolled with `PgUp` / `PgDn`.
- Narrow Routing/Usage views keep balance amounts readable instead of showing partial values like `$0/$`. Routing keeps compact balance context; detailed inspection lives in Usage/Balance.
- GUI balance state now uses the same core semantics as TUI, keeping `unknown`, `stale`, `exhausted`, `error`, and `unlimited` distinct.
- Usage burn forecasts now merge request ledger tail data with recent samples and include model / route-decision context in request logs, avoiding undercounted burn rate and midnight projections during high-volume Codex sessions.

#### Upgrade notes

- README now recommends the prebuilt shell / PowerShell installers generated by cargo-dist; `cargo-binstall` remains documented for Rust users.
- Set the usual client preset with `[codex.client_patch] preset = "..."` or `codex-helper switch on --preset ...`. Legacy `mode` / `--mode` are still accepted, but `preset` is the recommended spelling. Restart Codex App, TUI, or `codex exec` after changing it.
- The route-graph default affinity policy is back to `fallback-sticky`, so new configs keep a session on a successful fallback provider. If you prefer returning to the highest-priority provider as soon as it recovers, set `[codex.routing].affinity_policy = "preferred-group"` explicitly.
- Startup now loads and normalizes `~/.codex-helper/config.toml` before patching Codex; if `~/.codex/config.toml` still points at the local proxy but the switch state is missing, helper re-patches from the configured `preset` instead of trusting stale inferred state.
- To enable Responses WebSocket, set `responses_websocket = true` or pass `--responses-websocket`; it is not a preset and should only be enabled for upstream relays verified to support WebSocket. For example, `input8` was verified to support it, while `ciii` supports the HTTP endpoints but fails the WebSocket upstream/proxy path and should keep it disabled.
- If a relay requires provider-prefixed model names, use `provider add --model-map FROM=TO` or provider-level `model_mapping`, for example `gpt-5.5` -> `openai/gpt-5.5`.
- If you are unsure whether a relay supports compact or imagegen, run `codex-helper codex relay-capabilities` first. Use live smoke only when you want a real upstream test.
- Mobile remote control still uses `codex-helper switch remote-control ...`; it is separate from `chatgpt-bridge`.

## [0.15.0] - 2026-05-14

### 中文

#### 重要变化

- 配置格式升级到 `version = 5`。旧的 v2/v3/v4 和 legacy JSON 配置会自动迁移；迁移前仍会保留 `.bak` 备份。
- Route graph 现在是真正的运行时路由模型。月包优先、月包池、付费兜底和多 endpoint provider 都按 provider endpoint 选择，不再依赖 legacy station 状态。
- 默认会话粘性改为 `preferred-group`：临时 fallback 后，只要高优先级月包 provider 恢复可用，后续请求会回到月包组。旧的 fallback 粘性需要显式设置 `affinity_policy = "fallback-sticky"`。
- 新路由模型支持 `ordered-failover`、`tag-preferred`、`manual-sticky` 和多 endpoint provider。常见可复制模板见中文配置参考 `docs/CONFIGURATION.zh.md` 和英文参考 `docs/CONFIGURATION.md`。

#### 用户可见改进

- TUI、GUI、`routing explain`、请求详情和日志统一显示 provider endpoint、preference group、跳过原因和兼容 station 信息，排查“为什么走了 fallback”更直接。
- GUI/TUI 的路由和 provider 视图保留嵌套 route graph，不再把复杂配置意外压平成简单顺序。
- 请求 usage 的缓存读数改为单一口径，详情页和统计视图现在展示一致的读缓存/新缓存值。
- 文档更新为 v5 route graph 示例，覆盖单 provider、顺序兜底、月包池、月包止损、手动固定、多 endpoint provider、fallback 恢复、余额未知和 trusted exhaustion 行为。

#### 修复

- 余额刷新失败不会被当作耗尽，也不会中断其他 provider 的刷新；刷新请求现在有超时、复用代理运行态 HTTP client，并在日志中显示探测的 origin 和 adapter kind。
- TUI 按 `q` 退出时仍会优雅关停 proxy/admin server，但现在有短超时保护，避免被后台请求或长连接拖住太久。
- Sub2API 懒刷新零额度、余额查询失败、冷却和真实耗尽在 UI/路由预览中区分更清楚，降低误切到 fallback 的概率。

#### 升级说明

- 正常升级无需手动重写配置；启动 CLI、TUI、GUI 或 proxy 时会自动迁移。
- 如果你只是想按“单 provider / 顺序兜底 / 月包优先 / 月包止损 / 手动固定”来选，优先看 `docs/CONFIGURATION.zh.md` 的常用配置模板。
- 外部脚本如果还在写 legacy station/active 字段，应迁移到 provider、route target、routing 命令/API 或 v5 TOML。
- 当前版本仍使用系统/环境变量形式的 outbound proxy 支持；一等 `config.toml` outbound proxy 配置会在后续版本设计。

### English Summary

- Config format is now `version = 5`. Existing v2/v3/v4 and legacy JSON configs migrate automatically, with `.bak` backups kept before writing.
- Route graph is now the real runtime routing model. Monthly-first pools, paid fallback, and multi-endpoint providers are selected by provider endpoint instead of legacy station state.
- Session affinity now defaults to `preferred-group`: after temporary fallback, sessions return to the preferred monthly group once it is viable again. The old fallback-sticky behavior must be enabled explicitly with `affinity_policy = "fallback-sticky"`.
- TUI, GUI, `routing explain`, request details, and logs now show provider endpoint, preference group, skip reasons, and compatibility station context, making fallback decisions easier to diagnose.
- Balance refresh failures are not treated as exhaustion and do not stop other provider refreshes. Refresh calls now have a timeout, reuse the proxy runtime HTTP client, and log the probed origin plus adapter kind.
- Pressing `q` in the TUI still gracefully shuts down the proxy/admin server, but now has a short timeout guard to avoid long waits behind background requests or long-lived connections.
- Copyable v5 routing recipes for single-provider, ordered fallback, monthly-first, monthly-only, manual pin, and multi-endpoint setups are documented in `docs/CONFIGURATION.md`; the equivalent Chinese reference is `docs/CONFIGURATION.zh.md`.

## [0.13.0] - 2026-05-09

### 重点 / Highlights

- `version = 3` 成为默认配置：provider 只定义一次，routing 负责顺序兜底、手动固定和标签优先。旧配置会自动迁移到 `config.toml`，并在覆盖前保留 `config.toml.bak` / `config.json.bak`。
  `version = 3` is now the default config model: define providers once and let routing handle ordered fallback, manual pinning, and tag-based preference. Older configs migrate automatically to `config.toml`, with `config.toml.bak` / `config.json.bak` kept before overwrite.
- 新增更直观的 provider 切换体验：包月中转可以打 `billing=monthly` 标签，已知耗尽后按策略继续或停止。
  Provider switching is clearer: monthly relays can be tagged with `billing=monthly`, then known exhaustion can either fall through or stop according to policy.
- 余额和套餐更可见：会优先尝试 Sub2API、New API 和常见 `/user/balance` 接口；查询失败显示为 `unknown`，不会被当作耗尽。
  Balance and plan visibility improved: Sub2API, New API, and common `/user/balance` endpoints are probed first. Lookup failures show as `unknown` and do not count as exhaustion.
- TUI/GUI 更适合日常操作：routing 页面显示 provider 顺序、余额/套餐、tags、启停状态和候选状态；请求视图显示 token、cache token、耗时、速度、重试和估算成本。
  TUI/GUI are more operator-friendly: routing pages show provider order, balances/plans, tags, enabled state, and candidates; request views show tokens, cache tokens, latency, speed, retries, and estimated cost.
- Codex 配置 patch 更安全：`switch on/off` 只改本地代理相关片段，不会覆盖 Codex 运行期间写入的其他配置。
  Codex config patching is safer: `switch on/off` only changes the local proxy section and does not overwrite other Codex edits made during a run.
- 长时间运行更稳：上游连接增加连接超时、TCP keepalive、空闲连接回收；运行态日志和 TUI/GUI 刷新路径做了有界化处理。
  Long-running proxy stability improved with connect timeouts, TCP keepalive, idle connection cleanup, and bounded TUI/GUI refresh state.

### 可复制 Routing 示例 / Copyable Routing Examples

先定义 provider，再复制一个 `[codex.routing]` 策略。Claude 配置同理，把 `codex` 换成 `claude`。
Define providers once, then copy one `[codex.routing]` policy. For Claude, replace `codex` with `claude`.

```toml
version = 4

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

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly_a", "monthly_b"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "paygo"]
```

包月优先并保持可用：先用 `billing=monthly`，已知耗尽后继续兜底。
Monthly first with fallback: prefer `billing=monthly`, then fall back after known exhaustion.

```toml
[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"
```

包月严格止损：包月都已知耗尽时停止，不走付费兜底。
Strict monthly budget: stop instead of falling back to pay-as-you-go.

```toml
[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
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
- 修复 GUI 手动切换、重载或探测后，旧的后台刷新结果可能短暂覆盖新状态的问题。
  Fixed stale GUI background refresh results briefly overriding newer state after manual switching, reloads, or probes.
- 修复 GUI 在持久化配置保存或运行态重载后，界面有时还会沿用旧配置快照的问题。
  Fixed GUI cases where the UI could keep showing an old config snapshot after persisted saves or runtime reloads.
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

# 配置指南

English reference: [CONFIGURATION.md](CONFIGURATION.md)

本文档是英文配置参考的中文对应版，说明公开的 `version = 5` route graph 配置格式。

简短版本：先定义 providers，再让 `routing.entry` 指向 `routing.routes` 下的具名 route node。大多数用户只需要 `[codex.providers.*]`、`[codex.routing]`、`[codex.routing.routes.*]` 和 `[retry]`。

## 心智模型

- `providers` 是你的上游目录：base URL、认证、可选 tags、可选 endpoints。
- `routing.entry` 是某个服务的根 route node。
- `routing.routes.*` 是具名 route node。route node 可以引用 providers，也可以引用其他 route nodes。
- `profiles` 是请求默认值，例如 model 和 reasoning effort。它不应该负责选择 provider。
- `retry` 控制代理在返回错误前会做多努力的重试。

公开配置只使用 `provider`、`endpoint` 和 `route graph` 这些概念，运行时路由也直接使用这些 identity。

## 本地代理和出站代理

这里有两层不同的代理：

- 本地代理：Codex 连接到 codex-helper，通常是 `127.0.0.1:3211`。显式执行 `switch on` 让 Codex 指向 helper 后，即使没有配置出站网络代理，这一层仍然存在。
- 出站代理：codex-helper 通过网络代理连接 provider endpoints、relay dashboard 或 balance APIs。

当前出站代理支持来自底层 HTTP client 的系统/环境代理行为。`HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 和 `NO_PROXY` 可能影响 provider 与 balance 请求。目前还没有一等 `config.toml` 出站代理配置段。当前行为和后续设计见 [出站代理](#出站代理)。

## 文件位置

- 主配置：`~/.codex-helper/config.toml`
- 运行时状态：`~/.codex-helper/state/state.sqlite`
- 余额适配：`~/.codex-helper/usage_providers.json`
- 价格覆盖：`~/.codex-helper/pricing_overrides.toml`
- 提交后的请求调试日志：`~/.codex-helper/logs/requests.jsonl`
- 路由/控制面诊断日志：`~/.codex-helper/logs/control_trace.jsonl`
- Codex relay 诊断证据：`~/.codex-helper/logs/codex_relay_evidence.jsonl`

Codex 自己的文件仍由 Codex 维护：

- `~/.codex/auth.json`
- `~/.codex/config.toml`

只有显式执行本地 `switch on/off` 才能 patch `~/.codex/config.toml`，而且范围仅限 helper 自有的 provider selector 和 `model_providers.codex_proxy` stanza。codex-helper 永远不会读写 Codex 的 `auth.json`、模型缓存或 SQLite 文件。

## Relay Targets

Relay target 是本机客户端保存的本地/远端 codex-helper runtime 书签，配置在 `~/.codex-helper/config.toml`，供 `ch relay ...` 使用；真正的 provider/routing 配置仍然属于接收请求的 server runtime。

```toml
[relay_targets.nas]
service = "codex"
proxy_url = "http://nas.local:3211"
admin_url = "https://nas.example.com:4211"
admin_token_env = "CODEX_HELPER_NAS_ADMIN_TOKEN"
```

等价 CLI：

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url https://nas.example.com:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN
```

`local` 是内置 target，会按当前 `default_service` 解析到普通 loopback 端口；`ch relay local` 启动正常的本地前台流程。命名 target 默认是远端：`ch relay nas` 只会启动或附着到目标 runtime，并打开只读 TUI，绝不会修改 Codex 客户端配置。`--no-tui` 表示不打开控制台，`--attach-only` 要求目标 runtime 已经运行。要让 Codex 指向某个 target，必须另行显式执行 `codex-helper switch on --base-url <PROXY_URL>`。

`admin_token_env` 保存的是环境变量名，不是 token 值。远程 admin URL 必须使用 HTTPS；HTTP 只允许 loopback。可信 SSH/Tailscale 隧道可以把远端 admin listener 映射到客户端 loopback URL。远端 target 必须显式设置 `admin_url`；runtime 响应或重定向不能替换已经配置的 authority。带 userinfo、query 凭据、fragment 或 path 的 URL 会被拒绝。

## Fleet 观测注册表

Fleet 页是只读的。它可以观测本地和远端 runtime，但不会对远端节点发送 interrupt、message、approval 或 TTY attach。

Fleet target 配在 `[fleet.nodes.*]` 下，与 `relay_targets` 是两套不同的配置：

```toml
[fleet.nodes.workstation]
label = "Workstation"
admin_url = "https://workstation.example.com:4211"
admin_token_env = "CODEX_HELPER_WORKSTATION_ADMIN_TOKEN"
enabled = true

[fleet.nodes.mini]
label = "Mac mini"
admin_url = "https://mac-mini.tailnet.example.ts.net:4211"
admin_token_env = "CODEX_HELPER_MAC_MINI_ADMIN_TOKEN"
enabled = true
```

`admin_token_env` 只填写环境变量名，不要直接写 token 字符串。非 loopback 节点必须使用 HTTPS 并配置 `admin_token_env`；使用可信加密隧道时，应把它终止到客户端 loopback URL。

`ch tui` 会在 `9` 打开 Fleet 页，`r` 负责刷新，`Tab` 在节点和工作单元之间切换焦点，`t` 在 tree / flat 两种 work unit 视图间切换。

## 显式 Codex 客户端 Switch

客户端切换与启动、选择或诊断 runtime 是彼此独立的本地动作。Server、relay bookmark、TUI 刷新、桌面端动作和 capability 结果都不会隐式修改 Codex 配置。

```bash
codex-helper switch on                         # http://127.0.0.1:3211
codex-helper switch on --port 4321
codex-helper switch on --base-url https://relay.example/v1
codex-helper switch status
codex-helper switch off
```

`switch on` 会记录原 selector 和 helper stanza，然后只写入 helper 自有的 `model_providers.codex_proxy` stanza 并选中它。`switch off` 只恢复记录过的 selector/stanza。恢复 journal 位于 `~/.codex-helper/state/`；如果外部编辑导致当前文件既不匹配原 fingerprint，也不匹配 helper 应用后的 fingerprint，状态会进入 `recovery_required`，配置文件保持不动，等待人工协调。

Switch 永远不会修改 `~/.codex/auth.json`、`models_cache.json`、Codex SQLite、无关 providers、feature flags、compaction 设置、WebSocket 设置或 hosted-tool 设置。Provider capability 来自选中 provider 的契约和实时观测，而不是客户端 preset。

Proxy 生命周期与 switch 独立。`codex-helper serve` 默认在前台运行，`--resident` 会在控制台退出后保持 runtime，`codex-helper tui` 只附着一个只读控制台。这些命令都不会执行 `switch on` 或 `switch off`。Resident runtime 会在 `~/.codex-helper/run/` 写入提示性的 owner marker；使用只读的 `codex-helper daemon status` 检查。已安装的本地 runtime 使用 `codex-helper service start/stop/restart` 管理，不提供远程 HTTP shutdown 命令。

codex-helper 会在检查和转发前规范化 HTTP `Content-Encoding`。支持 `zstd`、`gzip` / `x-gzip`、`br` 和 `deflate`；成功解码后会转发普通 JSON，并移除失效的 `Content-Encoding` / `Content-Length`。只有上游要求收到完全相同的压缩 body 时，才设置 `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough`。

当 Codex 没有发送更强的 session header（`session_id`、`session-id`、`conversation_id` 或 `thread-id`）时，codex-helper 会把解码后 JSON 里的 `prompt_cache_key` 作为 session-affinity key，使普通 Responses 和 compact 请求可以留在同一个选中 provider endpoint。

## OpenAI Images 兼容入口

本地代理也暴露 OpenAI Images 风格入口，方便本地 skill 或脚本使用：

- `POST /v1/images/generations` 和 `/images/generations` 用于文本生图。
- JSON `POST /v1/images/edits` 和 `/images/edits` 用于带参考图生成。

codex-helper 会把这些请求转成非流式 `/v1/responses` + hosted `image_generation` tool 调用，
再把成功响应里的 `image_generation_call.result` 转回 `data[0].b64_json`。

示例：

```bash
curl 'http://127.0.0.1:3211/v1/images/generations' \
  -X POST \
  -H 'Content-Type: application/json' \
  --data-raw '{
    "model": "gpt-image-2",
    "prompt": "一只猫在雨夜的霓虹灯下",
    "size": "3840x2160",
    "output_format": "png",
    "quality": "high"
  }'
```

这个入口刻意复用正常 provider routing、model mapping、retry/fallback、auth 注入和请求日志；
被选中的真实上游仍必须支持 Responses hosted image generation。

参考图 edits 使用 JSON `images` 数组。每个元素可以是带 `image_url` 或 `file_id` 的对象，
也可以直接写图片 URL / data URL 字符串。helper 会把这些引用转成 Responses `input_image` 内容：

```bash
curl 'http://127.0.0.1:3211/v1/images/edits' \
  -X POST \
  -H 'Content-Type: application/json' \
  --data-raw '{
    "model": "gpt-image-2",
    "prompt": "把参考图人物画成一整页凌乱角色速写",
    "images": [
      {"image_url": "data:image/png;base64,..."}
    ],
    "size": "2160x2880",
    "output_format": "png",
    "quality": "high",
    "input_fidelity": "high"
  }'
```

文本生图和 JSON edits 当前都只支持单张输出结果（`n` 不传或为 `1`）。JSON edits 不解析
mask；带 `mask` 的 JSON 请求和 multipart edits 会按普通代理请求直通上游。

CLI capability diagnostic 是显式、人工触发、进程内的 operator 动作。请从 shell 运行：

```bash
codex-helper codex relay-capabilities \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default
```

命令只接受可选的 canonical provider endpoint selector（`--provider`，以及可选的 `--endpoint`）和可选 model；未指定 selector 时使用当前 runtime target。旧 station 名和位置型 upstream index 会被拒绝，`--preset`、`--mode`、`--compaction` 这类客户端假设同样会被拒绝。这个有界诊断会探测选中 endpoint 的 `/models`、`/responses` 和 `/responses/compact`，但不进入普通 retry/failover、request accounting、affinity、passive health 或 policy state。

响应包含：

- 必填的 `provider_id`、`endpoint_id`、`provider_endpoint_key` identity，以及 provider adapter、捕获的 catalog revision、request dialects 和选中 model；
- `expected`：Responses、compact、hosted image generation、WebSocket、ultra mapping、web search、apply patch 和 reasoning summaries 的 provider-owned capability decisions；
- `observed`：validation-only 的 `/models`、`/responses`、`/responses/compact` 结果、置信度和翻译证据；
- `continuity`：选中的 continuity domain、endpoint 数量、affinity policy、warnings 和 recommendations；
- `mismatches`：观测到的 endpoint 行为与捕获的 provider contract 不一致之处。

Capability 结果永远不会修改客户端配置、provider 配置、routing 或 policy state。使用 `--json` 可输出 JSON。

对 sub2api 风格中转来说，原始 OpenAI `/models` 响应（`data: [...]`）本身可以接受，但前提是 codex-helper 在 Codex 看到之前把它翻译成 Codex 的 `models: [...]` catalog。诊断响应会把这类情况标成 `observed.models.translation_required = true`。非 sub2api 中转也按同一套规则处理：它可以直接返回 Codex 形态的模型 metadata，也可以返回 helper 能翻译的 OpenAI model list。如果选中模型缺失或 metadata 不具备权威性，model-scoped capability decisions 会保持 `unknown`。

该诊断不会主动探测 hosted `image_generation`，因为这可能消耗额度或生成实际图片；contract 会保留该决定，但不会伪造 live evidence。Responses WebSocket support 来自捕获的 provider/model catalog。Codex 发送 `compaction_trigger` 时，helper 会识别 remote-compaction-v2 请求形态，并应用 lifecycle 与 route-continuity 保护，但上游仍必须返回有效的 v2 compaction item。

Provider contract 与 continuity model 刻意区分两件事：

- Endpoint capability 可以证明 Responses 和 `/responses/compact` 协议面。
- 协议支持并不证明两个 provider endpoint 能共享上游 encrypted response state。

默认情况下，每个 provider endpoint 都是自己的 continuity domain。对于 sub2api、New API 或其他 OpenAI-compatible gateway 这类中转链路，不要用 host name、base URL、provider 品牌名或“域名一致”来证明 encrypted compact state 可以跨 endpoint 移动。如果两个 endpoint 明确指向同一套上游账号或同一状态存储，才给它们配置相同的 `continuity_domain`：

```toml
[codex.providers.relay_hk]
base_url = "https://hk.relay.example/v1"
auth_token_env = "RELAY_HK_KEY"
continuity_domain = "relay-cluster-a"

[codex.providers.relay_us]
base_url = "https://us.relay.example/v1"
auth_token_env = "RELAY_US_KEY"
continuity_domain = "relay-cluster-a"
```

只有相同显式 `continuity_domain` 的 endpoints，才允许 provider-state-bound compact 在已有 route affinity 后跨 endpoint failover。每个 endpoint 代表不同中转账号、不同上游 OpenAI 账号或不透明 reseller 时，请保持未配置。直连 `https://api.openai.com/v1` 且只有一个认证账号的场景通常不需要这个字段，因为 provider-endpoint affinity 已经是连续性边界。

当 validation-only 诊断还不能解释问题时，可以手动跑更强的 live smoke 检查。它是真实上游请求，不是后台健康检查；可能消耗额度，也可能触发上游生成图片。codex-helper 在发送任何上游请求前，必须先收到固定确认字符串：

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5
```

不传可选 case flag 时，live smoke 只会通过 `/responses/compact` 检查 remote compaction v1。Remote compaction v2、Hosted image generation 和 Responses WebSocket 永远不属于默认 case。要显式测试选中 relay/provider 链路是否真的支持 Codex remote compaction v2，传 `--compact-v2`。这个 smoke 会发送 `POST /responses`，带 `stream: true`、一个 `compaction_trigger` input item，以及 `x-codex-beta-features: remote_compaction_v2`；只有响应流里刚好出现一个 compaction output item，并且出现 `response.completed`，才算通过：

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default \
  --compact-v2
```

要显式测试 hosted tool 请求链路，可以传：

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --image
```

要显式测试选中上游的 Responses WebSocket v2 链路，传 `--websocket`。这个 smoke 会用 WebSocket 打开 `GET /responses`，注入 `OpenAI-Beta: responses_websockets=2026-02-06`，发送一个最小 `response.create` frame；只要中转返回 `response.*` 事件，或 `codex.rate_limits` 这类 Codex WebSocket 协议事件，就说明握手和首帧协议可用：

```bash
codex-helper codex relay-live-smoke \
  --acknowledgement run-live-codex-relay-smoke \
  --model gpt-5.5 \
  --provider ciii \
  --endpoint default \
  --websocket
```

使用 `codex-helper codex relay-evidence --limit 20` 可以查看本地已脱敏摘要。

CLI 里不带可选 case 参数时会跑默认 compact smoke；传 `--compact-v2`、`--image`、`--websocket` 或任意组合时，只跑这些显式可选 case，避免为了测某个可选能力而额外消耗一次 compact 请求。

默认使用当前 runtime target；也可以用 `--provider` 和可选的 `--endpoint` 指定一个 canonical provider endpoint；不再接受 `--station` / `--upstream-index`。

Live smoke 刻意和正常路由隔离。它只选择一个 provider endpoint，每个选中的 case 最多发一次请求/连接，不走 route retry/failover，也不会写 request ledger、route affinity、passive health、runtime health、余额状态或修改客户端/配置。图片响应只做摘要：codex-helper 会报告是否出现 `image_generation_call`，但不会保存原始图片字节或 base64 payload。

Capability diagnostics 和 live smoke 会把已脱敏的摘要追加写入 `~/.codex-helper/logs/codex_relay_evidence.jsonl`。这个 evidence store 是本地人工诊断记忆，不是 routing truth；它不会进入 request ledger 汇总，也不会驱动 load balancing、session affinity、passive health、余额耗尽、retry policy 或客户端切换。需要给中转站对比或 bug report 附机器可读结果时，可以用 `codex-helper codex relay-evidence --json`。

落盘和终端输出仅用 `provider_id`、`endpoint_id`、`provider_endpoint_key` 标识目标，不保存或显示配置的 upstream base URL 与原始上游 payload。可以使用 `relay-evidence --provider` 按 canonical provider ID 过滤 evidence。

要诊断 remote compaction v1 是否生效，可以在 Codex 发生压缩后查看 codex-helper 请求账本：

```bash
codex-helper usage find --path responses/compact --limit 20
codex-helper usage find --path responses --limit 20
```

HTTP compact 请求会显示为 `POST /responses/compact`；remote compaction v2 会通过普通 `/responses` 携带结构化 `compaction_trigger` item。WebSocket turn 使用 `GET /responses` 风格 upgrade。Request ledger 会记录路径和捕获的 provider endpoint，但不会推断客户端 preset。

认证按 origin 隔离。客户端认证只能透传给官方 OpenAI origin；第三方 relay 必须配置 helper 侧的 `auth_token_env`、`auth_token` 或等价 API key，Codex 客户端账号 header 会在转发前被移除。

## 推荐开始方式

尽量使用 CLI 命令：

```bash
codex-helper config init

codex-helper provider add input \
  --base-url https://ai.input.im/v1 \
  --auth-token-env INPUT_API_KEY \
  --tag billing=monthly

codex-helper provider add openai \
  --base-url https://api.openai.com/v1 \
  --auth-token-env OPENAI_API_KEY \
  --tag billing=paygo

codex-helper routing order input openai
codex-helper config set-retry-profile balanced
```

这会生成和手写配置等价的轻量 TOML：

```toml
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["input", "openai"]

[retry]
profile = "balanced"
```

### Reasoning Guard：拦截推理 token 异常桶

如果某些 Codex 中转偶发出现 `reasoning_tokens = 516`、`1034`、`1552` 或同类 `518*n-2` 边界后直接 final、且答案质量明显异常，可以开启 retry reasoning guard。它只基于上游响应里的 usage 元数据做高置信拦截，不会尝试理解或判定答案本身是否正确。

```toml
[retry.reasoning_guard]
# 总开关。默认 false；只有显式开启才会拦截或重试。
enabled = true
# 固定异常桶：精确命中这些 reasoning token 数时触发 guard。
reasoning_equals = [516, 1034, 1552]
# 序列异常桶：额外匹配 reasoning_tokens = 518*n-2。默认 n<=4，设为 0 可关闭。
boundary_sequence_max_n = 4
# 命中后的动作：retry 改判为本地 502 并交给重试策略；block 直接拦截；observe 只记录。
action = "retry"
# 流式响应检查方式：strict-buffer 会先完整缓冲 SSE，避免异常内容先写给客户端。
stream_mode = "strict-buffer"
# 同一个客户端请求最多因 reasoning guard 增加多少轮上游请求。
max_guard_retries = 1
# guard 重试预算耗尽后如何处理仍命中的响应：pass 原样放行；block 继续拦截。
on_retry_exhausted = "pass"
# 只在这些路径上启用，避免影响非 Codex / 非 Responses 请求。
paths = ["/v1/responses", "/responses", "/v1/chat/completions", "/chat/completions"]
# 是否把命中记录为 control-trace event，便于 TUI Requests 和日志排查。
log_matches = true
```

- 默认关闭；不配置时不会改变现有行为。开启后默认匹配 `reasoning_equals = [516, 1034, 1552]`，并额外匹配 `518*n-2` 且 `n <= 4` 的边界序列。可以用 `reasoning_equals` 覆盖固定列表，用 `boundary_sequence_max_n = 0` 关闭序列匹配。
- 推荐先从上面的示例开始：`action = "retry"` + `stream_mode = "strict-buffer"` 可以在内容写给 Codex 前拦截异常响应；如果只想观察命中频率，把 `action` 改成 `"observe"`。
- `action = "retry"` 会把命中的成功响应改判为本地 502，并交给 `[retry]` 的 upstream/provider 重试规则处理。`max_guard_retries = 1` 表示同一个客户端请求最多因为该 guard 多打一轮上游请求；如果重试后仍命中，默认 `on_retry_exhausted = "pass"` 会把最后一次上游响应原样放给 Codex，避免 helper 自己中断任务。需要强拦截时可设为 `"block"`。
- `stream_mode = "strict-buffer"` 会在命中路径的流式请求中先完整缓冲 SSE，再检查末尾 usage。这样可以避免异常答案已经写给客户端后才发现异常 token，代价是这类流式请求不再实时透传。
- 配置支持运行时热加载：每个新请求准备阶段都会检查配置文件变更；已在途请求继续使用它开始时的配置快照。
- TUI Requests 页会在列表的 `RG` 列显示命中标记；详情里的 `Retry / route chain` 会显示 `decision=failed_reasoning_guard`、`class=reasoning_guard_triggered` 和 `reason=reasoning_tokens=<命中值>`。预算耗尽后放行的最后一次响应会按正常完成记录，同时 control-trace event 会有 `action=exhausted-pass`。

## Route Graph 形状

每个服务都可以有自己的 route graph：

```toml
[codex.routing]
entry = "monthly_first"
affinity_policy = "fallback-sticky"
# fallback-sticky affinity 的可选兼容边界。
# fallback_ttl_ms = 120000
# reprobe_preferred_after_ms = 30000

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]
```

规则：

- route node 名不能和 provider 名相同。
- `children` 可以引用 providers 或 route nodes。
- 循环引用会被拒绝。
- 重复的 provider 叶子节点会被拒绝，因为它会让 fallback 行为变得含糊。
- 运行时健康状态、cooldown、余额耗尽和 reprobe 状态不会写入静态配置。
- provider 名字不代表业务类型。如果 route policy 需要关心计费类型，请使用 `billing = "monthly"` 或 `billing = "paygo"` 这样的 tags。

常用策略：

- `ordered-failover`：从左到右尝试 children。children 可以是 providers，也可以是嵌套 route nodes。
- `tag-preferred`：按 `prefer_tags` 把 children 分成优先组，再 fallback 到其余 children。`on_exhausted = "continue"` 允许可信耗尽后继续走付费 fallback；`on_exhausted = "stop"` 防止自动溢出到 fallback。
- `manual-sticky`：使用一个明确的 `target`。target 可以是 route node、provider 或 provider endpoint。

大多数用户应该用 `ordered-failover` 表达固定优先级，用 `tag-preferred` 表达“包月优先”这类业务意图。

## 会话粘性

Route graph 的会话粘性是运行时状态，但为了 Codex 路由连续性，helper 会额外维护一个很小的持久 ledger。TOML 配置选择 affinity policy，并且可以选择性约束 fallback 粘性的边界：

- `fallback-sticky` 是 canonical version 5 配置模板使用的默认值。它会在 fallback provider 仍可用时继续让同一会话使用上次成功的 fallback provider；对于 remote compaction 这类可能携带上游账号绑定 encrypted state 的 official relay 功能更稳。设置 `fallback_ttl_ms` 可以限制低优先级 fallback affinity 的复用时长；设置 `reprobe_preferred_after_ms` 可以在 fallback target 变化后强制 reprobe 高优先级组。
- `preferred-group` 只会在当前最佳可用 preference group 内应用会话粘性，所以一个临时 fallback 到 paygo 的会话，会在月包 provider 再次可用时回到月包组。
- `off` 忽略自动 route affinity。
- `hard` 会把已有 affinity target 当成这个 route graph 的严格目标；如果该目标不可用，不会选择其他候选。

对于带 session id 的每个请求，codex-helper 使用 `session_id + service + route_graph_key` 作为 affinity key。只要 route graph 不变，同一会话就可以按 policy 继续使用之前选中的 provider/endpoint。这能提高一些 relay provider 的上游 prompt-cache 命中率，同时默认不会让自动粘性覆盖用户偏好。

成功的 route affinity 会提交到 helper 自有的运行时数据库：

```text
~/.codex-helper/state/state.sqlite
```

这个运行时存储只保存 helper 自己拥有的 provider endpoint identity，不保存也不推断上游 relay 的实现细节。Affinity 与其他运行时状态共用同一份存储所有权和持久化保证，不能再重定向到独立 JSON ledger。

对 Codex remote compaction，helper 会把带有 `encrypted_content`、`previous_response_id` 或 `compaction_summary` 这类字段的 v1 compact，以及带结构化 `compaction_trigger` 的 v2 compact，视为 provider-state-bound。在默认 `fallback-sticky` route affinity policy 下，如果这类请求还没有已有 route affinity，仍然可以尝试：helper 会按配置的 route graph 选择 provider endpoint，在成功后把它记录成该 session 的 affinity，并让上游判断 compact state 是否有效。在 `hard` affinity 下，缺失 affinity 仍会 fail-closed，并返回明确的连续性错误。如果已知 affinity endpoint 自身失败，`fallback-sticky` 可以继续沿 route graph 尝试并更新 affinity；`hard` 会阻止跨 endpoint 移动，除非显式共享的 `continuity_domain` 允许。不带这类状态字段的 v1 compact 仍可按 route policy 走普通 provider fallback。

Affinity 不是硬 pin：

- request retry、provider health、capability mismatch、cooldown 和可信余额耗尽仍然生效；
- 如果 sticky provider 失败，普通请求和非 state-bound 请求会继续沿当前 route graph 尝试，然后粘到下一个成功的 provider；
- provider-state-bound compact 会遵守 route affinity policy：`fallback-sticky` 保持可尝试，并在 fallback 成功后更新 affinity；`hard` 会留在 affinity continuity domain 内，除非显式共享的 `continuity_domain` 允许移动；
- 如果 provider tags、route node strategy、children、entry 或 provider endpoint identity 改变，route graph key 会改变，旧 affinity 不再匹配；
- route graph 决策使用 route/provider/endpoint controls，不存在第二套 station-shaped override 路径。

这意味着 `monthly_pool -> paygo` 这样的月包池通常会让一个会话持续使用同一个月包 provider，直到该 provider 不再可用，而不是每个请求轮询 provider、降低上游缓存命中率。

## 配置模板

先选一个模板开始，后续再细化字段。Claude 配置同理，把 `codex` 换成 `claude`。

| 用户目标 | 从哪个模板开始 | 原因 |
| --- | --- | --- |
| 只有一个上游，只想要 dashboard/logs | [单 Provider](#单-provider) | 最小配置，不会意外 fallback |
| 有几个 relays，希望第一个可用的生效 | [顺序 Fallback](#顺序-fallback) | 简单的从左到右 fallback |
| 有几个包月 relays 和一个按量备用 | [月包池加 Paygo Fallback](#月包池加-paygo-fallback) | 把月包池保留为一个优先组 |
| 有几个包月 relays 和几个付费 relay 备用 | [月包池加 Relay Fallback 池](#月包池加-relay-fallback-池) | 明确分隔月包池和付费 fallback 池 |
| 希望所有带 monthly tag 的 provider 都优先 | [按 Tag 包月优先](#按-tag-包月优先) | 使用 metadata，不硬编码某个池 |
| 宁愿失败也不要花 pay-as-you-go | [仅包月](#仅包月) | 可信月包耗尽后停止 |
| 需要临时强制某个 provider | [手动固定](#手动固定) | 明确且容易撤销 |
| 一个 provider 账号有多个 upstream endpoints | [单 Provider 多 Endpoints](#单-provider-多-endpoints) | 保留一个 provider identity，同时做 endpoint 级路由 |

路由决策使用运行时 provider endpoints。诊断和余额 DTO 会直接暴露 `provider_endpoint_key`、`provider_id` 和 `endpoint_id`。

### 单 Provider

适合只想把 codex-helper 作为本地代理和 dashboard 的场景。

```toml
version = 5

[codex.providers.main]
base_url = "https://api.example.com/v1"
auth_token_env = "MAIN_API_KEY"

[codex.routing]
entry = "main_route"

[codex.routing.routes.main_route]
strategy = "manual-sticky"
target = "main"

[retry]
profile = "balanced"
```

### 顺序 Fallback

这是多个 relays 的默认建议：第一个可用 provider 获胜，然后按顺序 fallback。

```toml
version = 5

[codex.providers.monthly]
base_url = "https://monthly.example/v1"
auth_token_env = "MONTHLY_API_KEY"
tags = { billing = "monthly" }

[codex.providers.backup]
base_url = "https://backup.example/v1"
auth_token_env = "BACKUP_API_KEY"
tags = { billing = "paygo" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "official" }

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["monthly", "backup", "openai"]

[retry]
profile = "balanced"
```

这是旧 priority 或 level-based 配置最直接的替代。

### 月包池加 Paygo Fallback

适合多个 monthly providers 组成一个优先组，而 paygo provider 只是最后备用的场景。

```toml
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.input1]
base_url = "https://ai.input1.im/v1"
auth_token_env = "INPUT1_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.input2]
base_url = "https://ai.input2.im/v1"
auth_token_env = "INPUT2_API_KEY"
tags = { billing = "monthly", pool = "input" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "codex_for"]

[retry]
profile = "balanced"
```

这样会把 monthly pool 保留为一等 route node。临时 502/429 类故障会通过 cooldown 和后续 reprobe 恢复。`unknown` balance 不会被当作耗尽。只有确认耗尽的 balance 信号才可能降级 monthly candidate。

### 月包池加 Relay Fallback 池

适合希望先消耗 monthly providers，再按固定顺序尝试几个 relay fallback 的场景。

```toml
version = 5

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_c]
base_url = "https://monthly-c.example/v1"
auth_token_env = "MONTHLY_C_API_KEY"
tags = { billing = "monthly" }

[codex.providers.right]
base_url = "https://right.example/v1"
auth_token_env = "RIGHT_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.providers.cch]
base_url = "https://cch.example/v1"
auth_token_env = "CCH_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.providers.codex_for]
base_url = "https://codex-for.example/v1"
auth_token_env = "CODEX_FOR_API_KEY"
tags = { billing = "paygo", kind = "relay" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly_a", "monthly_b", "monthly_c"]

[codex.routing.routes.fallback_pool]
strategy = "ordered-failover"
children = ["right", "cch", "codex_for"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "fallback_pool"]

[retry]
profile = "balanced"
```

这是表达“monthly first, several relays as backup”最清楚的形状。会话粘性仍然生效：只要 route graph 不变，一个对话会继续使用上次成功的 provider；只有当该 provider 失败、cooldown、不再支持请求、或被确认耗尽时才继续往后走。

### 按 Tag 包月优先

适合业务意图来自 metadata 的场景：优先所有带 `billing=monthly` 的 provider，然后继续到剩余 provider。

```toml
version = 5

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly", region = "hk" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly", region = "jp" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "continue"

[retry]
profile = "balanced"
```

只有已知完全耗尽的 monthly candidates 才会降级。balance 查询失败会显示为 `unknown`，不代表耗尽。

### 仅包月

适合宁愿失败也不要溢出到付费 fallback 的场景。

```toml
version = 5

[codex.providers.monthly_a]
base_url = "https://monthly-a.example/v1"
auth_token_env = "MONTHLY_A_API_KEY"
tags = { billing = "monthly" }

[codex.providers.monthly_b]
base_url = "https://monthly-b.example/v1"
auth_token_env = "MONTHLY_B_API_KEY"
tags = { billing = "monthly" }

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["monthly_a", "monthly_b"]

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_pool", "paygo"]
on_exhausted = "stop"

[retry]
profile = "balanced"
```

`paygo` 可以留在文件里以后使用，但 stop 规则会防止 preferred set 耗尽后自动溢出。

### 手动固定

适合调试、严格供应商选择或临时 steering。

```toml
version = 5

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"

[codex.routing]
entry = "debug_pin"

[codex.routing.routes.debug_pin]
strategy = "manual-sticky"
target = "input"
children = ["input", "openai"]

[retry]
profile = "balanced"
```

pinned target 是显式目标。它可以命名 route node、provider，或 `relay.hk` 这样的 provider endpoint。如果目标被禁用，codex-helper 会拒绝该 route，而不是静默选择其他 provider。

### 单 Provider 多 Endpoints

只有当一个账号确实有多个 upstream targets 时，才使用显式 endpoints。

```toml
version = 5

[codex.providers.relay]
alias = "Relay account"
auth_token_env = "RELAY_API_KEY"
tags = { billing = "paygo", vendor = "relay" }

[codex.providers.relay.endpoints.hk]
base_url = "https://hk.relay.example/v1"
priority = 0
tags = { region = "hk" }

[codex.providers.relay.endpoints.us]
base_url = "https://us.relay.example/v1"
priority = 1
tags = { region = "us" }

[codex.routing]
entry = "relay_route"

[codex.routing.routes.relay_route]
strategy = "ordered-failover"
children = ["relay.hk", "relay.us"]

[retry]
profile = "balanced"
```

不要用 endpoints 来模拟互不相关的 providers。互不相关的账号应该放在不同 provider 名下。

### Provider 并发上限

当某个 relay 账号只允许很少的同时请求数时，可以配置 `limits.max_concurrent_requests`。这是本进程本地限制：一个正在运行的 codex-helper 进程会统计活跃请求，并在路由时跳过已饱和候选。它不是多个 codex-helper 进程之间共享的分布式配额。

```toml
[codex.providers.relay.limits]
max_concurrent_requests = 5
limit_group = "relay-account"
```

`limit_group` 可选。不配置时，上限按 provider endpoint 单独生效。多个 provider endpoints 共用同一个上游账号额度时，可以给它们配置相同的 `limit_group`。endpoint 级 `limits` 会覆盖 provider 级 `limits`：

```toml
[codex.providers.relay]
alias = "Relay account"
auth_token_env = "RELAY_API_KEY"

[codex.providers.relay.limits]
max_concurrent_requests = 5
limit_group = "relay-account"

[codex.providers.relay.endpoints.hk]
base_url = "https://hk.relay.example/v1"

[codex.providers.relay.endpoints.us]
base_url = "https://us.relay.example/v1"

[codex.providers.relay.endpoints.us.limits]
max_concurrent_requests = 2
limit_group = "relay-us"
```

候选饱和时，routing 会把它当作临时不可用并继续走下一个 fallback。饱和不会记为 provider 失败，不会打开 cooldown，也不会污染 session affinity。`routing explain` 会用 `concurrency_saturated` 展示当前活跃数和上限。

如果只剩一两个候选，failover 仍然按照配置的 route 顺序走。饱和的候选会先被跳过；如果剩余候选全部饱和或不可用，请求会走正常的 route-unavailable 路径，而不是凭空造一个新的 provider。对于共用同一上游账号的多个 endpoint，请给它们设置相同的 `limit_group`，让 runtime 把它们当成一个并发池。

## Route 策略

| Strategy | 最适合 | UI 心智模型 |
| --- | --- | --- |
| `ordered-failover` | 简单 fallback 链和具名池 | 调整 child routes/providers 顺序 |
| `tag-preferred` | 包月优先、区域优先、厂商类型优先 | 选择 preferred tags，然后 fallback |
| `manual-sticky` | 调试或严格手动选择 | 选择一个 target |

`manual-sticky` 仍然会检查被 pin 的 target 自己是否饱和或不可用。它不会改变其它请求的 route graph fallback 规则，也不应该拿来当队列策略。

`on_exhausted` 当前由 `tag-preferred` 使用：

| Value | 行为 |
| --- | --- |
| `continue` | 继续进入剩余 fallback 顺序。适合优先保障可用性。 |
| `stop` | preferred providers 耗尽后停止。适合预算隔离。 |

codex-helper 不会从名字推断计费类型。如果 provider 是包月，请显式打 tag：

```toml
tags = { billing = "monthly" }
```

## Provider 字段

常见 provider 字段：

| Field | 含义 | 建议 |
| --- | --- | --- |
| `alias` | 适合人看的显示名 | 可选 |
| `base_url` | OpenAI-compatible endpoint | 单 endpoint provider 使用 |
| `auth_token_env` | bearer auth 的环境变量 | secrets 首选方式 |
| `auth_token` | 内联 bearer token | 支持，但避免提交 |
| `api_key_env` | `X-API-Key` auth 的环境变量 | 仅在需要时使用 |
| `api_key` | 内联 `X-API-Key` 值 | 支持，但避免提交 |
| `tags` | 自由 metadata | 使用稳定 tags，例如 `billing`、`vendor`、`region` |
| `enabled` | provider 是否可路由 | 临时变更优先用 `provider disable/enable` |
| `supported_models` | 可选 model allowlist | 高级 |
| `model_mapping` | 可选 model alias map | 高级 |

认证字段先按 provider 要求的 HTTP header 来选：

- **OpenAI 和大多数 OpenAI-compatible 中转** 使用 bearer auth：`Authorization: Bearer <key>`。
  日常使用配置 `auth_token_env`，只在本地临时测试时才用 `auth_token`。
  即使中转后台把密钥叫做 “API key”，这里通常也应该填 `auth_token_env`，不是 `api_key_env`。
- 只有 provider 文档明确要求 `X-API-Key` header 时，才使用 `api_key_env` / `api_key`。
- 优先使用 `*_env` 字段，避免 secret 写入 `~/.codex-helper/config.toml`。
  config 里的值是环境变量名，不是密钥本身；运行 codex-helper 的进程里必须真的设置了这个环境变量。
- 同一 header 类型里，如果同时配置 inline 值和 env 引用，inline 值优先。
  如果同时配置 bearer 和 `X-API-Key` 两类凭据，codex-helper 会同时发送两个 header；除非中转明确要求，否则不要这样配。

`model_mapping` 用于“Codex 请求的模型名”和“某个 relay 实际要求的模型名”不一致的场景。它是 provider 级别配置，路由选中该 provider 后才会改写请求体里的 `model` 字段；没有选中该 provider 时不会影响其它 provider。

```toml
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
supported_models = { "gpt-5.5" = true }
model_mapping = { "gpt-5.5" = "openai/gpt-5.5" }
```

OpenAI 官方同样用 bearer 形式：

```toml
[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
```

PowerShell 示例：

```powershell
$env:OPENAI_API_KEY = "sk-..."
codex-helper
```

也支持一个 `*` 通配符，适合一整类模型都要加 provider 前缀：

```toml
[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"
supported_models = { "gpt-*" = true }
model_mapping = { "gpt-*" = "openai/gpt-*" }
```

CLI 添加 provider 时也可以直接写：

```bash
codex-helper provider add relay \
  --base-url https://relay.example/v1 \
  --auth-token-env RELAY_API_KEY \
  --supported-model gpt-5.5 \
  --model-map gpt-5.5=openai/gpt-5.5
```

内联 secret 示例：

```toml
[codex.providers.local_test]
base_url = "https://test.example/v1"
auth_token = "sk-..."
```

内联 secrets 适合本地临时配置。正式使用时更推荐环境变量。

## Profiles

Profiles 是可选请求默认值，不应该决定 provider routing。

```toml
[codex]
default_profile = "daily"

[codex.profiles.daily]
model = "gpt-5"
reasoning_effort = "medium"
service_tier = "auto"

[codex.profiles.deep]
extends = "daily"
reasoning_effort = "high"
```

Profiles 只定义请求默认值；provider selection 属于 `[codex.routing]`。

## 余额适配

大多数 relay 用户不需要为了显示余额手写 `usage_providers.json`。这个文件是可选且由 operator 管理的输入：文件缺失时，codex-helper 只使用内存中的内置 adapters，不会创建文件；文件不可读或内容无效时会返回明确的加载错误，也绝不会替换或重写原文件。如果没有显式 adapter 匹配某个 upstream，codex-helper 会尝试常见 relay 探测：

1. `sub2api_usage`：使用 model API key 请求规范化 provider origin 下的 `GET /v1/usage`。
2. `new_api_token_usage`：使用 model API key 请求规范化 provider origin 下的 `GET /api/usage/token/`。
3. `new_api_user_self`：使用 dashboard-style auth 请求规范化 provider origin 下的 `GET /api/user/self`。
4. `openai_balance_http_json`：使用 model API key 请求规范化 provider origin 下的 `GET /user/balance`。

RightCode hosts（`www.right.codes` / `right.codes`）会在通用 relay 探测前特殊处理。内置 `rightcode_account_summary` adapter 会请求 `GET https://www.right.codes/account/summary`，使用 bearer auth，读取钱包 `balance`，并按 upstream path prefix（例如 `/codex`）匹配订阅日额度。

如果 relay 需要独立的 dashboard credentials、provider-kind 专用字段、自定义 endpoint 或更安全的 exhaustion 处理，显式 adapters 仍然有用。

请求触发的 balance observations 默认会先延迟 60 秒合并，同一 provider 默认至少间隔 600 秒才会再次自动查询；显式配置的 `poll_interval_secs` 小于 120 秒时会被抬到 120 秒。Operator clients 读取最后一次已提交 observation，不会触发远程 refresh。

对于 `api.openai.com`，codex-helper 会跳过 relay-style `/user/balance` 探测。如果设置了 `OPENAI_ADMIN_KEY`，它可以自动读取 `openai_organization_costs`；否则官方 OpenAI provider 会保持 unknown，而不会被当作 exhausted。

OpenAI 的公开平台接口不是钱包余额 API。它提供组织级 costs/usage 视图，适合显示当前花费，但不适合按钱包余额或订阅剩余量来 routing。要接入官方 OpenAI billing 视图，可以使用：

```json
{
  "providers": [
    {
      "id": "openai-official-costs",
      "kind": "openai_organization_costs",
      "domains": ["api.openai.com"],
      "token_env": "OPENAI_ADMIN_KEY",
      "require_token_env": true,
      "endpoint": "https://api.openai.com/v1/organization/costs",
      "poll_interval_secs": 600,
      "refresh_on_request": false,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

`OPENAI_ADMIN_KEY` 必须是组织级 admin key；普通 model API key 不是稳定替代。`openai_organization_costs` adapter 会自行添加滚动 30 天的 `start_time` 和 `limit=30` 查询参数。

`endpoint` 可以是绝对 URL，也可以是相对路径。相对路径会基于去掉结尾 `/v1` 后的 provider base URL 解析。配置不支持 endpoint templates、credential interpolation、environment interpolation、任意 headers 或任意 variables；每个封闭 adapter kind 自己构造认证 header 和必需的查询参数。

Sub2API API-key telemetry：

```json
{
  "providers": [
    {
      "id": "input-monthly",
      "kind": "sub2api_usage",
      "domains": ["ai.input.im"],
      "poll_interval_secs": 600,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

RightCode account summary：

```json
{
  "providers": [
    {
      "id": "rightcode",
      "kind": "rightcode_account_summary",
      "domains": ["www.right.codes", "right.codes"],
      "endpoint": "https://www.right.codes/account/summary",
      "token_env": "RIGHTCODE_API_KEY",
      "poll_interval_secs": 600,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

普通场景可以省略这段配置：默认 adapter 已内置，会按 upstream URL 匹配 RightCode，并使用该 upstream 配置里的 model API key。只有当你希望使用独立的余额 key（例如 `RIGHTCODE_API_KEY`）、自定义 endpoint，或调整 routing trust policy 时才需要显式添加。默认情况下，RightCode 的 daily package quota 只作为 routing 的展示信号，因为账户 `balance` 可能仍然可用，而且 daily subscription windows 可能是 lazy reset。

New API dashboard-style quota：

```json
{
  "providers": [
    {
      "id": "right-newapi",
      "kind": "new_api_user_self",
      "domains": ["www.right.codes"],
      "endpoint": "/api/user/self",
      "token_env": "RIGHTCODE_NEWAPI_ACCESS_TOKEN",
      "require_token_env": true,
      "new_api_user_id_env": "RIGHTCODE_NEWAPI_USER_ID",
      "poll_interval_secs": 600,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

重要余额行为：

- 查询失败显示为 `unknown`，不是 exhausted，也不会改变 route graph 配置。
- 已知 exhausted snapshot 只有在 `trust_exhaustion_for_routing = true` 时才会降级自动路由。
- 账号停用、key 无效、余额/额度不足等终态错误会临时禁用对应 provider target，并抑制后续余额请求 6 小时，避免持续打已不可用账号。
- Sub2API lazy subscription-window zeros 在真实请求刷新周期前会显示为 lazy reset 状态；不要把它和稳定套餐设计混淆。
- Sub2API subscription-mode `remaining` 是周期限制容量信号，不是钱包余额。`remaining` 为零表示至少一个配置的订阅窗口当前耗尽；当前日包/今日窗口耗尽时，codex-helper 会抑制后续余额请求并临时跳过该 target，即使这是展示型套餐信号。
- New API quota values 会按 `QuotaPerUnit = 500000` 转换；带 `unlimited_quota = true` 的 token usage snapshots 永远不会被当作 exhausted。
- RightCode `balance` 会显示为钱包余额。匹配到的 `subscriptions[*].total_quota` 和 `remaining_quota` 会显示为 daily quota；`reset_today = false` 表示 codex-helper 会把今天新发放的日额度计入剩余额度后再展示。
- 如果 provider 对可用订阅返回误导性的零余额，请设置 `trust_exhaustion_for_routing = false`。
- UI 只展示最后一次已提交的 balance observation 及 freshness，不会修改或刷新 provider state。
- Balance HTTP 调用有边界，并且复用和 proxy runtime calls 相同的 outbound client。查询失败时，日志应该显示被探测的 origin 和 adapter kind，例如 `sub2api_usage` 或 `openai_balance_http_json` 返回了非 JSON。

## Usage 页面

TUI 第 5 页显示为 `Usage`。它是本地日用量面板，不是持久多日分析仓库。
Tauri 桌面端 `Usage` 页读取同一份本地日读模型；最近请求行只是 drilldown 样本，不再作为今日总量的来源。

如何阅读：

- 顶部汇总显示今日请求数、token、估算成本、成功率、token 结构和全局 retry gate 数量。
- 24 小时活跃度从已提交的 request events 展示本地日请求分布。
- Provider 和 station 行展示今日请求量、错误数、token、估算成本和平均耗时。
- Model、session、project 面板展示今日主要用量来源。
- Coverage status 区分 committed read model 中完整、部分和未知的经济事实。
- `unknown` 表示没有可信余额数据或查询失败，不能当作健康余额。
- `stale` 表示 snapshot 已过期；它和 `exhausted`、`error`、`unlimited` 是不同状态。
- `unlimited` 是已知不限量/无限 quota，不是 unknown。
- Provider observation 失败会显示为 `unknown` 或 `stale`，但不会打断 TUI redraw 或 read-model refresh。
- 桌面端 Usage 表格里的 `Chain` 操作会按需读取脱敏 request chain。先用总量面板发现异常，再用单行 Chain 排查某一次请求为什么这样路由。
- `Routing` 页面保留紧凑的只读余额上下文。判断“今天用了什么”看 TUI `Usage`；检查余额和路由 eligibility 看 Routing/Stations。

## 运行时保护

Codex `/responses` 和 `/responses/compact` SSE 流带有 idle watchdog，避免上游已经返回 HTTP 200、但之后长时间不再输出字节时让 Codex 一直 waiting。

- `CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS` 控制 Codex Responses SSE 流的逐 chunk idle timeout。
- 默认值：`900` 秒。
- 设置为 `0` 会关闭 watchdog。
- 超过 `86400` 秒的值会被限制为 24 小时。
- 超时后，codex-helper 会用合成的 `response.failed` SSE event 结束客户端流，并记录 `codex_helper_error=upstream_stream_idle_timeout`。

## 出站代理

codex-helper 本身是一个本地代理，但它可能仍然需要出站代理才能访问某些 relays 或 dashboard balance APIs。

当前行为：

- 底层 HTTP client 使用 reqwest 默认的系统/环境代理支持。标准 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 和 `NO_PROXY` 环境变量可能影响出站请求。
- 目前还没有一等 `config.toml` 出站代理配置段。

未来配置版本的推荐模型：

- 为所有 provider 和 balance traffic 增加全局 outbound proxy profile。
- 当某个 relay 需要不同 egress path 时，允许 provider endpoint 覆盖。
- 优先使用 provider/endpoint-scoped proxy selection，而不是 route-scoped proxy selection。Route policy 应该决定使用哪个 provider endpoint；endpoint 应该拥有“如何访问它”的配置。
- 只有当 dashboard/balance API 和 model endpoint 处于不同网络路径时，才允许 balance adapters 覆盖 proxy 行为。

常见 adapter kinds：

- `sub2api_usage`
- `sub2api_auth_me`
- `new_api_token_usage`
- `new_api_user_self`
- `rightcode_account_summary`
- `openai_organization_costs`
- `openai_balance_http_json`
- `relay_balance_http_json`
- `yescode_profile`
- `budget_http_json`

有用 adapter 字段：

| Field | 含义 |
| --- | --- |
| `domains` | 此 adapter 适用的 relay hosts |
| `endpoint` | 绝对 balance URL，或相对于规范化 provider base URL 的路径 |
| `token_env` | adapter auth 使用的环境变量 |
| `require_token_env` | 要求使用 `token_env`，而不是 fallback 到 model API key |
| `new_api_user_id_env` | 仅用于 `new_api_user_self`：为固定 `New-Api-User` header 提供值的环境变量 |
| `poll_interval_secs` | refresh throttle / cache window |
| `refresh_on_request` | routed requests 是否可以触发 balance refresh |
| `trust_exhaustion_for_routing` | exhausted snapshots 是否可以降级 routing |
| `extract` | 自定义 balance 字段的 JSON path 提取规则 |

## 价格

价格配置和 relay 配置分离：

- 本地覆盖：`~/.codex-helper/pricing_overrides.toml`
- 内置和同步 catalog：由 TUI/桌面端渲染，并用于估算成本
- 同步命令：

```bash
codex-helper pricing sync <URL> --dry-run
codex-helper pricing sync-basellm --model gpt-5 --dry-run
```

本地修正或 relay-specific multipliers 使用 pricing overrides。不要把价格表复制到 provider config 里。

## CLI 编辑

初始化 canonical 配置：

正常启动，包括默认打开 TUI 的路径，都要求 `~/.codex-helper/config.toml` 使用 `version = 5`。`config init` 会创建当前模板；`--force` 只会在写入 `config.toml.bak` 后替换已有 canonical 文件。历史 schema 和 `config.json` 都是不支持的输入，不会被自动导入、迁移、重写或删除。

```bash
codex-helper config init
codex-helper config init --force
```

管理 providers：

```bash
codex-helper provider add input --base-url https://ai.input.im/v1 --auth-token-env INPUT_API_KEY --tag billing=monthly
codex-helper provider add openai --base-url https://api.openai.com/v1 --auth-token-env OPENAI_API_KEY --tag billing=paygo
codex-helper provider list
codex-helper provider show input
codex-helper provider disable input
codex-helper provider enable input
```

用 CLI 管理 entry route：

```bash
codex-helper routing order input openai
codex-helper routing pin input
codex-helper routing prefer-tag --tag billing=monthly --order input,openai --on-exhausted continue
codex-helper routing set --policy ordered-failover --order input,openai
codex-helper routing clear-target
codex-helper routing show
codex-helper routing explain
```

当 CLI 只编辑 entry node 时，会保留现有 route graph 结构。高级嵌套图编写在专用 route-node 命令加入前，仍然更适合用 TOML。

编辑 Claude 服务而不是 Codex 服务时，在 provider/routing 命令上使用 `--claude`。

`routing show` 读取持久化配置。`routing list` 和 `routing explain` 读取编译后的运行时候选视图。
使用 `routing explain --model <MODEL> --json` 可以检查和运行时 admin explain API 相同的 selected route、candidate order、route paths 和结构化 skip reasons。
在该响应里，`provider_endpoint_key`、`provider_id`、`endpoint_id`、`route_path` 和 `preference_group` 是 canonical routing identity。

## 检查 Routing 和日志

手动编辑 TOML 前，先使用这些命令：

```bash
codex-helper routing show
codex-helper routing explain --json
codex-helper routing explain --model <MODEL> --json
```

`routing show` 回答“配置里保存了什么”。`routing explain` 回答“运行时现在会尝试什么”，包括 candidate order、route paths，以及 disabled provider、unsupported model、cooldown 或 trusted balance exhaustion 等 skip reasons。

Provider eligibility 只从已提交的 provider observation 派生：

- 封闭的 provider adapter 按 endpoint origin、route scope、account fingerprint、config revision、incarnation 和 generation 规范化 observation。
- 只有身份匹配且权威的 exhausted observation 才能创建自动 block。HTTP 错误、transport failure、parse failure 和被动请求健康状态都不会创建或清除 quota eligibility。
- Observation history、自动 action 与 eligibility projection 会先原子提交到 `~/.codex-helper/state/state.sqlite`，随后新的 policy revision 才会出现在 routing 与 `GET /__codex_helper/api/v1/operator/read-model` 中。
- 手动 eligibility 的优先级始终高于自动 block 或 recovery。
- codex-helper 不会因为自动额度处理去修改 Codex auth 文件、ChatGPT 登录状态、中转账号文件或 provider dashboard。

权威的 request/attempt lifecycle 会提交到：

```text
~/.codex-helper/state/state.sqlite
```

请求重试或切换 provider 时，committed attempts 会保留 `provider_id`、`endpoint_id`、`route_path`、`decision`、`status_code` 和 `error_class`。Request-ledger 读取与 usage rollups 都查询这些已提交事实。`logs/requests.jsonl` 只是可选的 post-commit 调试输出；写入失败或 rotation 不会影响 accounting，生产 reader 也不会 replay 它。

排查 compact 时，按请求路径过滤：

```bash
codex-helper usage find --path responses/compact --limit 20
```

只读 operator bundle 会在 `data.recent_requests` 中发布最近提交的请求。本地过滤检索请使用 `codex-helper usage find`；远程控制面不提供通用 ledger 查询端点。

排查某一次请求或一个 session 的路由控制时间线时，使用 request-chain export：

```bash
codex-helper usage chain --trace-id <TRACE_ID> --json
codex-helper usage chain --request-id <REQUEST_ID>
codex-helper usage chain --session <SESSION_ID> --limit 20 --json
```

同一读模型也可以通过本地 admin API 获取：

```text
GET /__codex_helper/api/v1/request-ledger/chain?trace_id=<TRACE_ID>
GET /__codex_helper/api/v1/request-ledger/chain?request_id=<REQUEST_ID>
GET /__codex_helper/api/v1/request-ledger/chain?session=<SESSION_ID>&limit=20
```

request-chain export 是 allowlist 诊断视图。它包含 request identity、status、脱敏 route attempts、稳定 provider signal / policy action code 和 timeline events；刻意不包含 client address、cwd、upstream base URL、provider trace 内部字段或原始上游 payload 细节。较大的 session export 会被上限截断，并用 `truncated` 标记，而不是把整个本地日志直接输出。

Control trace 默认启用，写入：

```text
~/.codex-helper/logs/control_trace.jsonl
```

它记录 routing selection events，例如 compiled route plan、provider endpoint、preference group、skipped higher-priority groups、pinned-route decisions、retry options 和 failover reasons。当选中低优先级 preference group 时，`route_graph_selection_explain` event 会列出每个被跳过的高优先级 provider endpoint，以及 `unsupported_model`、`cooldown`、`usage_exhausted`、`runtime_disabled` 或 `attempt_avoided` 这样的结构化原因。设置 `CODEX_HELPER_CONTROL_TRACE=0` 可以关闭；设置 `CODEX_HELPER_CONTROL_TRACE_PATH` 可以写到其他路径。

request/debug 日志和 `control_trace.jsonl` 共用有界 JSONL 保留策略，由 `CODEX_HELPER_REQUEST_LOG_MAX_BYTES` 和 `CODEX_HELPER_REQUEST_LOG_MAX_FILES` 控制（默认：active file 50 MiB，保留 10 个轮转文件）。过大的 active JSONL 文件会在首次写入时轮转，轮转文件会按数量和总预算清理。

其它 helper 本地日志使用同一套有界存储实现，但有独立开关：

- `runtime.log`：`CODEX_HELPER_RUNTIME_LOG_MAX_BYTES` / `CODEX_HELPER_RUNTIME_LOG_MAX_FILES`（默认 20 MiB、10 个文件）。
- `codex_relay_evidence.jsonl`：`CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_BYTES` / `CODEX_HELPER_RELAY_EVIDENCE_LOG_MAX_FILES`（默认 20 MiB、10 个文件）。

## 排查包月优先 Routing

如果一个本应优先 monthly providers 的 route fallback 到 paygo，先检查运行时状态，再修改配置：

```bash
codex-helper routing explain --model <MODEL> --json
```

优先检查这些字段：

- `selected_route.provider_endpoint_key` 和 `selected_route.preference_group` 显示运行时现在会尝试什么。Group `0` 是最高优先级组。
- `candidates[].skip_reasons` 解释 preferred candidate 为什么被跳过，例如 `unsupported_model`、`cooldown`、`usage_exhausted`、`runtime_disabled` 或 `attempt_avoided`。
- `affinity.policy` / `affinity_policy` 显示自动 affinity 是 `preferred-group`、`off`、`fallback-sticky` 还是 `hard`。
- route graph 决策使用 `provider_endpoint_key`、`provider_id`、`endpoint_id` 和 `route_path` 作为 canonical identity。

对于 monthly-first setup，生成配置默认使用 `affinity_policy = "fallback-sticky"`，因为中转 provider 往往会把缓存和 encrypted response state 绑定到上游账号。如果你更希望故障恢复后自动回到最佳 monthly group，可以显式设置 `affinity_policy = "preferred-group"`。如果 route 意外一直使用 paygo，请检查这些原因：

- monthly provider 被禁用或缺少 auth；
- 请求的 model 不被 monthly provider 支持；
- monthly endpoint 在 retryable failures 后处于 cooldown；
- 可信 balance data 把 endpoint 标记为 `usage_exhausted`；
- 配置使用 `affinity_policy = "fallback-sticky"` 或 `hard`。

可信余额耗尽是 provider-endpoint 运行时信号。它会为 canonical provider endpoint 创建一个归 codex-helper 所有的 balance policy action，并可以在当前请求/刷新窗口内降级 monthly endpoint，但不是永久 session preference。新的非耗尽 balance snapshot 只会清除 codex-helper 自己拥有的 balance action，不会清手动 eligibility，也不会清其它基于响应的 cooldown。如果所有 candidate 当前都被可信耗尽或 cooldown 阻断，Codex streaming turn 会收到带有限延迟的可重试 `response.failed` SSE，而不是反复打已耗尽 upstream；helper 也会排队一个受节流的 balance refresh，让恢复后的中转重新进入路由。如果某个 provider 对可用订阅返回误导性的零余额，请为该 usage provider 设置 `trust_exhaustion_for_routing = false`，或修复 balance extractor。

当选中低优先级组时，使用 control trace：

```text
~/.codex-helper/logs/control_trace.jsonl
```

查找 `route_graph_selection_explain`。它记录 selected provider endpoint、selected preference group、skipped higher-priority groups 和 per-candidate skip reasons。Route/provider/endpoint identifiers 是唯一 routing control vocabulary。

诊断 route continuity 时，control trace 字段刻意保持 provider-opaque：

- `continuity.class` / `continuity_class`：`stateless_or_session_preferred` 或 `provider_state_bound`。
- `affinity.source`：`session_route_affinity` 表示已知 affinity 约束了选择；`none` 表示没有 affinity。
- `provider_failover_allowed`：本次请求是否允许 helper 切换到另一个 provider endpoint。
- `provider_failover_blocked_reason`：provider failover 被阻止的原因，例如 `provider_state_bound` 或 `state_bound_compact_missing_affinity`。
- `balance_signal_authoritative`：compact 连续性阻断里目前是 `false`。余额探测可以解释 routing 降级，但不能证明 state-bound compact 可以安全换到另一个 provider endpoint。

如果 state-bound compact 没有恢复到 route affinity 且请求返回本地连续性错误，查找 `route_continuity_blocked` 事件和 `reason = "state_bound_compact_missing_affinity"`。这表示当前 policy 拒绝通过选择某个 provider endpoint 来引导 affinity；它不代表 helper 判断出了 relay 背后是 sub2api、New API、OpenAI 或任何其它实现。在 `fallback-sticky` 下，无 affinity compact 请求通常会沿配置的 route graph 发出，而不是产生这个本地阻断。

## Operator UI

TUI 和桌面端消费同一份 typed、redacted `OperatorReadModel`，对远程 runtime control plane 只使用 `GET` / `HEAD`：

- Provider 视图展示 names、aliases、enabled state、tags、已提交的 balance/eligibility facts、expanded fallback order、canonical endpoint keys 和 policy provenance。
- Routing 视图展示 compiled entry、candidate order、route paths、skip reasons、continuity 和捕获的 revisions。
- Requests 与 sessions 展示 provider choice、route affinity、retry chain、token/cache evidence 和 committed economics。
- `ready`、`stale`、`disconnected`、`auth_required` 状态保持显式；客户端不会伪造本地 fallback view。

这些 operator clients 只提供查询。持久 provider/routing intent 应通过本地 CLI 命令或 `config.toml` 编辑。在 TUI 中，`n` / `o` 是唯一的局部例外，只 patch 本机 Codex 配置里的 helper provider selector/stanza，绝不会修改远程 runtime。本指南中唯一的客户端配置 mutation 是独立且显式的本地 `switch on/off`。

## 配置兼容性

`~/.codex-helper/config.toml` 中的 `version = 5` 是唯一公开 helper 配置契约。更早的有版本或无版本 TOML，以及 `config.json`，都是不支持的输入。启动时会报告不支持的文件/schema，但不会导入、转换、删除，也不会把它当作 generated state；项目不再提供 migrate 命令或兼容 runtime reader。

使用 `config init` 创建独立的 version 5 文件，然后直接通过 `[service.providers]`、`[service.routing]` 和 route nodes 表达 provider/routing intent。历史文件如需人工参考，应保留在 canonical path 之外。

## 设计边界

codex-helper 刻意避免：

- 每个 provider 复制一份完整 Codex config；
- 从 provider 名字推断 billing class；
- 在没有真实测量前假装 speed-first 或 cost-first routing 可靠；
- 保留 `level` 作为主要用户可见 priority control；
- 把 balance lookup failure 当作 provider exhaustion；
- 从 TUI 或桌面表单静默写出另一套 station-shaped schema；
- 在 nested route nodes 已经能更清楚表达同一意图时，继续使用特殊 `pool-fallback` syntax。

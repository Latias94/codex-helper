# 配置指南

English reference: [CONFIGURATION.md](CONFIGURATION.md)

本文面向日常使用者，重点说明 `version = 5` 配置怎么写、常见路由策略怎么复制使用，以及代理支持的边界。完整字段参考见英文文档。

## 先看结论

- 主配置文件是 `~/.codex-helper/config.toml`。
- 新配置格式是 `version = 5`。旧的 v2/v3/v4 TOML 和 legacy JSON 会在启动 CLI、TUI、GUI 或 proxy 时自动迁移，并保留 `.bak` 备份。
- 先定义 provider，再用 routing 决定怎么选 provider。不要再把 station 当成新的配置模型。
- 包月、按量、区域、厂商这类业务含义要写到 `tags`，不要依赖 provider 名字推断。
- `codex-helper` 本身是本地代理；是否走外部网络代理是另一件事，见 [代理支持](#代理支持)。

## 配置位置

常用文件：

| 文件 | 用途 |
| --- | --- |
| `~/.codex-helper/config.toml` | 主配置：provider、routing、profile、retry |
| `~/.codex-helper/usage_providers.json` | 余额/套餐适配 |
| `~/.codex-helper/pricing_overrides.toml` | 价格覆盖 |
| `~/.codex-helper/logs/requests.jsonl` | 请求日志 |
| `~/.codex-helper/logs/control_trace.jsonl` | 路由和控制面诊断日志 |

Codex 自己的文件仍由 Codex 维护：

| 文件 | 说明 |
| --- | --- |
| `~/.codex/auth.json` | Codex 登录态 |
| `~/.codex/config.toml` | Codex 配置 |

`codex-helper switch on/off` 和默认启动只会局部修改 Codex 的本地代理片段，不会覆盖 Codex 运行中写入的其他配置。

## 推荐开始方式

优先用 CLI 生成配置：

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

对应的 TOML 很薄：

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

## 路由模型

v5 的心智模型是：

| 概念 | 含义 |
| --- | --- |
| `providers` | 上游目录：base URL、认证、标签、可选 endpoints |
| `routing.entry` | 当前服务的入口 route node |
| `routing.routes.*` | 具名 route node，可以引用 provider 或其他 route node |
| `profiles` | 请求默认值，比如 model、reasoning effort，不负责选 provider |
| `retry` | 请求失败后重试和 failover 的强度 |

常用路由策略：

| 策略 | 适合场景 |
| --- | --- |
| `ordered-failover` | 按顺序兜底，最直观 |
| `tag-preferred` | 包月优先、区域优先、厂商优先 |
| `manual-sticky` | 临时固定或调试 |

route node 的 `children` 可以引用 provider，也可以引用其他 route node。这样就能表达“月包池先内部兜底，再整体兜底到按量池”。

## 常用配置模板

先选一个最接近你需求的模板复制，再按你的 provider 名字和 key 环境变量修改。Claude 配置同理，把 `codex` 换成 `claude`。

| 目标 | 推荐模板 |
| --- | --- |
| 只有一个上游，只想用本地代理和日志 | [单 provider](#单-provider) |
| 多个中转，按顺序找第一个可用 | [顺序兜底](#顺序兜底) |
| 多个包月中转，最后才走按量 | [月包池加按量兜底](#月包池加按量兜底) |
| 多个包月中转，再兜底到多个付费中转 | [月包池加付费 fallback 池](#月包池加付费-fallback-池) |
| 所有 `billing=monthly` 都优先 | [按标签包月优先](#按标签包月优先) |
| 宁愿失败也不要自动走按量 | [包月止损](#包月止损) |
| 临时固定某个 provider | [手动固定](#手动固定) |
| 一个 provider 账号有多个 endpoint | [多 endpoint provider](#多-endpoint-provider) |

### 单 provider

适合只想让 codex-helper 提供本地代理、日志、TUI/GUI 的用户。

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

### 顺序兜底

适合大多数多中转用户：优先走第一个 provider，失败、冷却、不支持模型或可信余额耗尽后再往后走。

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

### 月包池加按量兜底

适合“多个包月都可以先用，全部不可用或可信耗尽后再走按量”的场景。

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

[codex.providers.paygo]
base_url = "https://paygo.example/v1"
auth_token_env = "PAYGO_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
entry = "monthly_first"

[codex.routing.routes.monthly_pool]
strategy = "ordered-failover"
children = ["input", "input1", "input2"]

[codex.routing.routes.monthly_first]
strategy = "ordered-failover"
children = ["monthly_pool", "paygo"]

[retry]
profile = "balanced"
```

这比把所有 provider 平铺在一个列表里更清楚：`monthly_pool` 是一个优先组，`paygo` 是最后兜底。

### 月包池加付费 fallback 池

适合“先用所有包月中转，再按固定顺序尝试多个付费中转”的场景。

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

这个写法比一个长列表更适合维护：月包池和付费 fallback 池的边界很清楚。

### 按标签包月优先

适合你希望“所有打了 `billing=monthly` 的 provider 都自动优先”的场景。

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

`on_exhausted = "continue"` 表示包月组被可信判断为耗尽后，可以继续走后面的 fallback。

### 包月止损

适合你明确不希望自动切到按量 provider 的场景。

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

[codex.routing.routes.monthly_first]
strategy = "tag-preferred"
prefer_tags = [{ billing = "monthly" }]
children = ["monthly_a", "monthly_b", "paygo"]
on_exhausted = "stop"

[retry]
profile = "balanced"
```

`paygo` 可以留在配置里以后手动切换，但 `stop` 会阻止包月耗尽后自动溢出到按量。

### 手动固定

适合调试、临时指定厂商、或排查某个 provider 的问题。

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

`target` 可以是 route node、provider，也可以是 provider endpoint，比如 `relay.hk`。

### 多 endpoint provider

只有在“同一个 provider 账号确实有多个上游 endpoint”时才使用 endpoints。互不相关的账号应该写成多个 provider。

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

## 会话粘性

默认 `affinity_policy = "preferred-group"`。这表示同一个 Codex 会话会尽量继续使用上次成功的 provider，但粘性只在当前最高优先级可用组内生效。

这个默认值适合包月优先：

- 如果月包 provider 暂时失败，当前请求可以 fallback。
- 当月包 provider 恢复可用，后续请求会回到月包组。
- 不会因为一次 fallback 到 paygo，就长期粘在 paygo 上。

如果你需要兼容旧行为，可以显式设置：

```toml
[codex.routing]
entry = "monthly_first"
affinity_policy = "fallback-sticky"
fallback_ttl_ms = 120000
reprobe_preferred_after_ms = 30000
```

如果你完全不想要自动 route affinity：

```toml
[codex.routing]
entry = "main"
affinity_policy = "off"
```

## 余额和套餐

余额刷新失败不会被当作耗尽，也不会中断其他 provider 的刷新。UI 里看到 `unknown` 时，含义是“没有可信余额快照”，不是“余额为零”。

只有在对应余额适配明确允许时，可信耗尽才会影响 routing：

```json
{
  "providers": [
    {
      "name": "input-subscription",
      "kind": "sub2api_usage",
      "domains": ["ai.input.im"],
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

如果某个中转站经常把仍可用的包月返回为 0，应该设为：

```json
{
  "trust_exhaustion_for_routing": false
}
```

这样余额仍会展示，但不会把 provider 自动降级。

## 代理支持

这里要区分两层代理。

### 本地代理

`codex-helper` 本身就是本地代理。默认情况下：

- Codex 请求先发到 `127.0.0.1:3211`。
- codex-helper 再按 routing 选择 provider。
- 如果使用 TUI/GUI，界面和控制 API 也是在这个本地进程周围工作。

所以即使你没有配置任何“外部网络代理”，只要启用了 codex-helper 的 Codex patch，Codex 请求仍会先经过本地 proxy server。这是 codex-helper 的核心工作方式。

### 出站网络代理

出站网络代理指的是：

```text
codex-helper -> provider / relay / balance API
```

当前版本还没有一等 `config.toml` 出站代理配置段。运行时 HTTP client 使用 reqwest 的默认系统/环境代理支持，因此这些环境变量可能影响出站请求：

| 环境变量 | 用途 |
| --- | --- |
| `HTTP_PROXY` | HTTP 出站代理 |
| `HTTPS_PROXY` | HTTPS 出站代理 |
| `ALL_PROXY` | 通用出站代理 |
| `NO_PROXY` | 不走代理的 host 列表 |

如果你没有设置系统代理或这些环境变量，codex-helper 对上游 provider 和余额 API 的访问就是直连。

建议使用时注意：

- 不要把“本地代理端口”理解成“出站网络代理”。`127.0.0.1:3211` 是给 Codex 连 codex-helper 的。
- 如果你设置了全局 `HTTP_PROXY` / `HTTPS_PROXY`，确认 `NO_PROXY` 覆盖了你不希望走代理的内网地址。
- 后续更合理的配置方向是全局 outbound proxy profile，加 provider/endpoint 级覆盖。route policy 应负责“选谁”，provider endpoint 应负责“怎么连过去”。

## 常用诊断

查看保存的 routing：

```bash
codex-helper routing show
```

查看运行时会怎么选：

```bash
codex-helper routing explain
codex-helper routing explain --model gpt-5 --json
```

查看最近请求：

```bash
codex-helper usage tail --limit 20
codex-helper usage find --errors --limit 10
```

查看整体状态：

```bash
codex-helper status
codex-helper doctor
```

排查“为什么没有走包月优先”时，优先看：

```bash
codex-helper routing explain --model <MODEL> --json
```

候选项里的 `provider_endpoint_key`、`provider_id`、`endpoint_id`、`route_path`、`preference_group` 是 v5 路由身份。`compatibility.station` 只用于迁移诊断，不是新的运行时身份。

## 迁移说明

当前写出的配置版本是 `version = 5`。旧的 `version = 4`、`version = 3`、`version = 2`、未标版本 TOML 和 legacy `config.json` 都会迁移到 `config.toml`。

迁移前会保留备份：

```text
~/.codex-helper/config.toml.bak
~/.codex-helper/config.json.bak
```

你也可以先预览迁移：

```bash
codex-helper config migrate --dry-run
```

确认后写入：

```bash
codex-helper config migrate --write --yes
```

迁移完成后，手写配置应以 provider、provider endpoint 和 route graph 为准。station 形状只作为兼容读取和迁移诊断，不再作为新的配置写入面。

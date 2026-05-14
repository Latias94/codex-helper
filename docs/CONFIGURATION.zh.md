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

Legacy `station` 数据只是迁移输入。手写配置时应该围绕 `provider`、`endpoint` 和 `route graph` 思考。

## 本地代理和出站代理

这里有两层不同的代理：

- 本地代理：Codex 连接到 codex-helper，通常是 `127.0.0.1:3211`。即使你没有配置出站网络代理，只要启用了 codex-helper 的 Codex patch，这一层仍然存在。
- 出站代理：codex-helper 通过网络代理连接 provider endpoints、relay dashboard 或 balance APIs。

当前出站代理支持来自底层 HTTP client 的系统/环境代理行为。`HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 和 `NO_PROXY` 可能影响 provider 与 balance 请求。目前还没有一等 `config.toml` 出站代理配置段。当前行为和后续设计见 [出站代理](#出站代理)。

## 文件位置

- 主配置：`~/.codex-helper/config.toml`
- 余额适配：`~/.codex-helper/usage_providers.json`
- 价格覆盖：`~/.codex-helper/pricing_overrides.toml`
- 请求日志：`~/.codex-helper/logs/requests.jsonl`
- 路由/控制面诊断日志：`~/.codex-helper/logs/control_trace.jsonl`

Codex 自己的文件仍由 Codex 维护：

- `~/.codex/auth.json`
- `~/.codex/config.toml`

`switch on/off` 和一键启动只会 patch Codex 配置中的本地代理片段。它们不会覆盖无关的 Codex 配置改动。

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

## Route Graph 形状

每个服务都可以有自己的 route graph：

```toml
[codex.routing]
entry = "monthly_first"
affinity_policy = "preferred-group"
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

Route graph 的会话粘性是运行时状态。TOML 配置选择 affinity policy，并且可以选择性约束 fallback 粘性的边界：

- `preferred-group` 是默认值。会话粘性只会在当前最佳可用 preference group 内生效，所以一个临时 fallback 到 paygo 的会话，会在月包 provider 再次可用时回到月包组。
- `off` 忽略自动 route affinity。
- `fallback-sticky` 保留旧 fallback 粘性行为，作为显式兼容模式。设置 `fallback_ttl_ms` 可以限制低优先级 fallback affinity 的复用时长；设置 `reprobe_preferred_after_ms` 可以在 fallback target 变化后强制 reprobe 高优先级组。
- `hard` 会把已有 affinity target 当成这个 route graph 的严格目标；如果该目标不可用，不会选择其他候选。

对于带 session id 的每个请求，codex-helper 使用 `session_id + service + route_graph_key` 作为 affinity key。只要 route graph 不变，同一会话就可以按 policy 继续使用之前选中的 provider/endpoint。这能提高一些 relay provider 的上游 prompt-cache 命中率，同时默认不会让自动粘性覆盖用户偏好。

Affinity 不是硬 pin：

- request retry、provider health、capability mismatch、cooldown 和可信余额耗尽仍然生效；
- 如果 sticky provider 失败，请求会继续沿当前 route graph 尝试，然后粘到下一个成功的 provider；
- 如果 provider tags、route node strategy、children、entry 或 provider endpoint identity 改变，route graph key 会改变，旧 affinity 不再匹配；
- route graph 配置下 legacy station overrides 会被禁用；请使用 route/provider/endpoint 控制。

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

路由决策使用运行时 provider endpoints。诊断里的 `compatibility` station/upstream 字段只是迁移上下文，不是新的 identity。

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

## Route 策略

| Strategy | 最适合 | UI 心智模型 |
| --- | --- | --- |
| `ordered-failover` | 简单 fallback 链和具名池 | 调整 child routes/providers 顺序 |
| `tag-preferred` | 包月优先、区域优先、厂商类型优先 | 选择 preferred tags，然后 fallback |
| `manual-sticky` | 调试或严格手动选择 | 选择一个 target |

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

Legacy profile station bindings 只是迁移用途。新的 v5 配置应该使用 `[codex.routing]`。

## 余额适配

大多数 relay 用户不需要为了显示余额手写 `usage_providers.json`。如果没有显式 adapter 匹配某个 upstream，codex-helper 会尝试常见 relay 探测：

1. `sub2api_usage`：使用 model API key 请求 `GET {{base_url}}/v1/usage`。
2. `new_api_token_usage`：使用 model API key 请求 `GET {{base_url}}/api/usage/token/`。
3. `new_api_user_self`：使用 dashboard-style auth 请求 `GET {{base_url}}/api/user/self`。
4. `openai_balance_http_json`：使用 model API key 请求 `GET {{base_url}}/user/balance`。

如果 relay 需要 dashboard credentials、自定义 headers、自定义 endpoint 或更安全的 exhaustion 处理，显式 adapters 仍然有用。

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
      "endpoint": "https://api.openai.com/v1/organization/costs?start_time={{unix_days_ago:30}}&limit=30",
      "poll_interval_secs": 60,
      "refresh_on_request": false,
      "trust_exhaustion_for_routing": false
    }
  ]
}
```

`OPENAI_ADMIN_KEY` 必须是组织级 admin key；普通 model API key 不是稳定替代。

在 balance adapter templates 中，`{{base_url}}` 会被规范化为不带结尾 `/v1`。只有当 balance endpoint 确实位于和模型请求相同的 `/v1` 前缀下时，才使用 `{{upstream_base_url}}`。官方 usage/cost APIs 需要查询窗口时，可以使用 `{{unix_now}}`、`{{unix_now_ms}}` 和 `{{unix_days_ago:30}}` 这类时间 helpers。

Sub2API API-key telemetry：

```json
{
  "providers": [
    {
      "id": "input-monthly",
      "kind": "sub2api_usage",
      "domains": ["ai.input.im"],
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

New API dashboard-style quota：

```json
{
  "providers": [
    {
      "id": "right-newapi",
      "kind": "new_api_user_self",
      "domains": ["www.right.codes"],
      "endpoint": "{{base_url}}/api/user/self",
      "token_env": "RIGHTCODE_NEWAPI_ACCESS_TOKEN",
      "headers": {
        "New-Api-User": "{{env:RIGHTCODE_NEWAPI_USER_ID}}"
      },
      "poll_interval_secs": 60,
      "refresh_on_request": true,
      "trust_exhaustion_for_routing": true
    }
  ]
}
```

重要余额行为：

- 查询失败显示为 `unknown`，不是 exhausted，也不会改变 route graph 配置。
- 已知 exhausted snapshot 只有在 `trust_exhaustion_for_routing = true` 时才会降级自动路由。
- Sub2API lazy subscription-window zeros 在真实请求刷新周期前会显示为 lazy reset 状态；不要把它和稳定套餐设计混淆。
- Sub2API subscription-mode `remaining` 是周期限制容量信号，不是钱包余额。`remaining` 为零表示至少一个配置的订阅窗口当前耗尽，并且在可信后可能降级 routing。
- New API quota values 会按 `QuotaPerUnit = 500000` 转换；带 `unlimited_quota = true` 的 token usage snapshots 永远不会被当作 exhausted。
- 如果 provider 对可用订阅返回误导性的零余额，请设置 `trust_exhaustion_for_routing = false`。
- UI 展示的是 cached balance snapshots；手动刷新使用 `POST /__codex_helper/api/v1/providers/balances/refresh`。
- Balance HTTP 调用有边界，并且复用和 proxy runtime calls 相同的 outbound client。查询失败时，日志应该显示被探测的 origin 和 adapter kind，例如 `sub2api_usage` 或 `openai_balance_http_json` 返回了非 JSON。

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
- `openai_organization_costs`
- `openai_balance_http_json`
- `relay_balance_http_json`
- `yescode_profile`
- `budget_http_json`

有用 adapter 字段：

| Field | 含义 |
| --- | --- |
| `domains` | 此 adapter 适用的 relay hosts |
| `endpoint` | Balance endpoint URL，支持可选 `{{base_url}}` templating |
| `token_env` | adapter auth 使用的环境变量 |
| `require_token_env` | 要求使用 `token_env`，而不是 fallback 到 model API key |
| `headers` / `variables` | 请求 templating |
| `poll_interval_secs` | refresh throttle / cache window |
| `refresh_on_request` | routed requests 是否可以触发 balance refresh |
| `trust_exhaustion_for_routing` | exhausted snapshots 是否可以降级 routing |
| `extract` | 自定义 balance 字段的 JSON path 提取规则 |

## 价格

价格配置和 relay 配置分离：

- 本地覆盖：`~/.codex-helper/pricing_overrides.toml`
- 内置和同步 catalog：由 TUI/GUI 渲染，并用于估算成本
- 同步命令：

```bash
codex-helper pricing sync <URL> --dry-run
codex-helper pricing sync-basellm --model gpt-5 --dry-run
```

本地修正或 relay-specific multipliers 使用 pricing overrides。不要把价格表复制到 provider config 里。

## CLI 编辑

初始化或检查迁移：

正常启动，包括默认打开 TUI 的路径，会自动完成配置迁移。只有在你想显式预览或诊断迁移时，才需要使用迁移命令。

```bash
codex-helper config init
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
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
在该响应里，`provider_endpoint_key`、`provider_id`、`endpoint_id`、`route_path` 和 `preference_group` 是 v5 routing identity。Legacy station/upstream identity 会在每个 candidate 的 `compatibility` 对象下报告，用于迁移诊断。

## 检查 Routing 和日志

手动编辑 TOML 前，先使用这些命令：

```bash
codex-helper routing show
codex-helper routing explain --json
codex-helper routing explain --model <MODEL> --json
```

`routing show` 回答“配置里保存了什么”。`routing explain` 回答“运行时现在会尝试什么”，包括 candidate order、route paths，以及 disabled provider、unsupported model、cooldown 或 trusted balance exhaustion 等 skip reasons。

每个完成的请求都会写入：

```text
~/.codex-helper/logs/requests.jsonl
```

当请求重试或切换 provider 时，请求日志会保存 `retry.route_attempts[]`。最有用的字段是 `provider_id`、`endpoint_id`、`route_path`、`decision`、`status_code` 和 `error_class`。

Control trace 默认启用，写入：

```text
~/.codex-helper/logs/control_trace.jsonl
```

它记录 routing selection events，例如 compiled route plan、provider endpoint、preference group、skipped higher-priority groups、pinned-route decisions、retry options 和 failover reasons。当选中低优先级 preference group 时，`route_graph_selection_explain` event 会列出每个被跳过的高优先级 provider endpoint，以及 `unsupported_model`、`cooldown`、`usage_exhausted`、`runtime_disabled` 或 `attempt_avoided` 这样的结构化原因。设置 `CODEX_HELPER_CONTROL_TRACE=0` 可以关闭；设置 `CODEX_HELPER_CONTROL_TRACE_PATH` 可以写到其他路径。旧的 `retry_trace.jsonl` 只有在 `CODEX_HELPER_RETRY_TRACE=1` 时才写入。

## 排查包月优先 Routing

如果一个本应优先 monthly providers 的 route fallback 到 paygo，先检查运行时状态，再修改配置：

```bash
codex-helper routing explain --model <MODEL> --json
```

优先检查这些字段：

- `selected_route.provider_endpoint_key` 和 `selected_route.preference_group` 显示运行时现在会尝试什么。Group `0` 是最高优先级组。
- `candidates[].skip_reasons` 解释 preferred candidate 为什么被跳过，例如 `unsupported_model`、`cooldown`、`usage_exhausted`、`runtime_disabled` 或 `attempt_avoided`。
- `affinity.policy` / `affinity_policy` 显示自动 affinity 是 `preferred-group`、`off`、`fallback-sticky` 还是 `hard`。
- `compatibility` 只是 legacy station/upstream 上下文。route graph 决策优先看 `provider_endpoint_key`、`provider_id`、`endpoint_id` 和 `route_path`。

对于 monthly-first setup，默认通常是 `affinity_policy = "preferred-group"`。使用该 policy 时，会话可能在临时故障期间使用 fallback provider，但只要 monthly provider 重新可用，下一次请求会回到最佳可用 monthly group。如果 route 一直使用 paygo，请检查这些原因：

- 显式 session/global route target override 已设置；
- monthly provider 被禁用或缺少 auth；
- 请求的 model 不被 monthly provider 支持；
- monthly endpoint 在 retryable failures 后处于 cooldown；
- 可信 balance data 把 endpoint 标记为 `usage_exhausted`；
- 配置显式使用 `affinity_policy = "fallback-sticky"` 或 `hard`。

可信余额耗尽是 provider-endpoint 运行时信号。它可以在当前请求/刷新窗口内降级 monthly endpoint，但不是永久 session preference。如果某个 provider 对可用订阅返回误导性的零余额，请为该 usage provider 设置 `trust_exhaustion_for_routing = false`，或修复 balance extractor。

当选中低优先级组时，使用 control trace：

```text
~/.codex-helper/logs/control_trace.jsonl
```

查找 `route_graph_selection_explain`。它记录 selected provider endpoint、selected preference group、skipped higher-priority groups 和 per-candidate skip reasons。临时 steering 请使用 route/provider/endpoint controls；route graph configs 会拒绝 legacy station overrides。

## UI 编辑

TUI 和 GUI 应该和配置文件保持相同心智模型：

- Provider list：names、aliases、enabled state、tags、balance 和 expanded fallback order。
- Routing editor：entry strategy、target、children/order、preferred tags、exhaustion behavior 和 route graph tree preview。
- GUI route node editor：常见 graph 编辑所需的 create、rename、delete 和 save nested route nodes。
- Requests and sessions：provider choice、route affinity、retry chain、token/cache token usage、cache hit rate 和 estimated cost。
- Runtime steering：适合临时选择；持久 provider intent 应属于 `[service.providers]` 和 `[service.routing]`。

TUI routing editor 快捷键：

- `Enter`：用 `manual-sticky` pin 选中的 provider。
- `a`：把 entry route 切到使用可见顺序的 `ordered-failover`。
- `[` / `]` 或 `u` / `d`：在 entry route 的 expanded order 中移动选中 provider。
- `f`：启用 `prefer_tags = [{ billing = "monthly" }]` 的 monthly-first tag preference。
- `e`：启用或禁用选中 provider。
- `s`：在 `continue` 和 `stop` 之间切换 `on_exhausted`。
- `1` / `2` / `0`：设置 `billing=monthly`、设置 `billing=paygo` 或清除 `billing`。

高级 multi-endpoint providers、model mappings、自定义 balance extraction rules 和深层 nested graphs 仍然更适合用 CLI 或 raw TOML/JSON 编辑。

## 迁移

当前 route graph schema 写出 `version = 5`。现有 `version = 4` route graph 配置仍会作为迁移输入加载。

正常用户通常不需要手动运行迁移命令。启动 codex-helper，包括默认打开 TUI 的启动路径，会加载 legacy `version = 4`、`version = 3`、`version = 2`、未标版本 TOML 和 legacy `config.json`，然后迁移到带 `version = 5` 的 `config.toml`。写入新文件前，旧文件会复制为 `config.toml.bak` 或 `config.json.bak`。

迁移期间，如果结果 route graph 会使用新的 `preferred-group` 默认值，而不是旧 fallback stickiness，codex-helper 会发出警告。如果你想恢复旧行为，请在迁移前或迁移后显式设置 `affinity_policy = "fallback-sticky"`。

手动迁移命令主要用于在不走正常 TUI/proxy 启动路径的情况下预览或诊断迁移：

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

迁移规则：

- 旧 `active_station` 会成为初始 route entry 的一部分；
- 旧 `level` 只作为排序输入；
- 旧 station/group members 会展开成 provider entries 和 entry route 的 `children`；
- legacy v3 `policy/order/target/prefer_tags` 会变成 v5 entry route node；
- legacy v3 `pool-fallback` 会变成 nested route nodes；
- 现有 provider tags 会保留；
- `billing=monthly` 这类 business tags 永远不会被猜测；
- endpoint-scoped station groups 可能警告，因为 provider routing 默认是 provider-level。

迁移完成后，请把 provider 和 routing graph 当成公开写入面。station-shaped inputs 是兼容读取和迁移诊断，不是运行时 routing identity。

## 设计边界

codex-helper 刻意避免：

- 每个 provider 复制一份完整 Codex config；
- 从 provider 名字推断 billing class；
- 在没有真实测量前假装 speed-first 或 cost-first routing 可靠；
- 保留 `level` 作为主要用户可见 priority control；
- 把 balance lookup failure 当作 provider exhaustion；
- 从 GUI 或 TUI 静默写出 legacy station schema；
- 在 nested route nodes 已经能更清楚表达同一意图时，继续使用特殊 `pool-fallback` syntax。

# codex-helper（Codex CLI 本地助手 / 本地代理）

> 让 Codex CLI 走一层本地“保险杠”：  
> 集中管理所有中转站 / key / 配额，在额度用完或上游挂掉时自动切换，并提供会话与脱敏辅助工具。

当前版本：`v0.13.0`

> English version: `README_EN.md`

---

## 截图

![内置 TUI 面板](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## 为什么需要 codex-helper？

如果你有下面这些情况，codex-helper 会很合适：

- **不想手改 `~/.codex/config.toml`**  
  手工改 `model_provider` / `base_url` 容易写坏，也不好恢复。

- **有多个中转 / 多个 key，要经常切换**  
  想把 OpenAI 官方、Packy 中转、自建中转都集中管理，并一条命令切换“当前在用”的那一个。

- **经常到 401/429 才发现额度用完**  
  希望上游额度用尽时能自动切到备用线路，而不是人工盯着报错。

- **命令行里希望“一键找回 Codex 会话”**  
  例如“给我当前项目最近一次会话，并告诉我怎么 resume”。

- **想给 Codex 加一层本地脱敏和统一日志**  
  请求先本地过滤敏感信息，再发到上游；所有请求写进一个 JSONL 文件，方便排查和统计。

---

## 一分钟上手（TL;DR）

### 1. 安装（推荐：cargo-binstall）

```bash
cargo install cargo-binstall
cargo binstall codex-helper   # 安装 codex-helper，可得到 codex-helper / ch 两个命令
```

安装成功后，`codex-helper` / `ch` 会被放到 Cargo 的 bin 目录（通常是 `~/.cargo/bin`），只要该目录在你的 `PATH` 里，就可以在任意目录直接运行。

> 如果你更习惯从源码构建：  
> `cargo build --release` → 使用 `target/release/codex-helper` / `ch` 即可。

### 2. 一条命令启动 Codex 助手（最推荐）

```bash
codex-helper
# 或更短的：
ch
```

它会自动帮你：

- 启动 Codex 本地代理，监听 `127.0.0.1:3211`；
- 如果在交互终端运行，会默认显示一个内置 TUI 面板（可用 `--no-tui` 关闭；按 `q` 退出；`1-7` 切页；`7` 查看历史会话；在 Sessions/History 页按 `t` 查看对话记录）；
- 对 429/5xx/网络抖动等瞬态错误，以及常见上游认证/路由类错误（例如 401/403/404/408）在**未开始向客户端输出响应**前进行有限次数的自动重试/切换（可配置）；
- 在修改前检查 `~/.codex/config.toml`，如已指向本地代理且存在备份，会询问是否先恢复原始配置；
- 必要时修改 `model_provider` 与 `model_providers.codex_proxy`，让 Codex 走本地代理；启用前会写入备份，恢复后会清理旧备份，确保下次重新获取最新原始配置；
- 写入 `model_providers.codex_proxy` 时，默认设置 `request_max_retries = 0` 以避免“Codex 重试 + codex-helper 重试”叠加（你也可以在 `~/.codex/config.toml` 中手动覆盖）；
- 如果 `~/.codex-helper/config.toml` / `config.json` 还没初始化，会尝试根据 `~/.codex/config.toml` + `auth.json` 推导一个默认上游（首次自动落盘默认生成 TOML）；
- 用 Ctrl+C 或在 TUI 中按 `q` 退出时，尝试从备份恢复原始 Codex 配置。

从此之后，你继续用原来的 `codex` 命令即可，所有请求会自动经过 codex-helper。

---

## 当前产品定位

从当前版本开始，`codex-helper` 不再只是“本地代理 + 多上游切换工具”，而是一个 **Codex-first 本地控制平面**：

- 用 `provider` / `routing` 管理上游与兜底，而不是只靠零散 `base_url` 记忆配置；
- 用 `profile` 表达常用意图，例如 `daily` / `fast` / `deep`；
- 用 **session identity card** 回答“这个 Codex 会话现在到底走哪个 provider / upstream / model / fast mode / reasoning”；
- 支持 **session 级覆盖**：`model`、`reasoning_effort`、`service_tier`、路由目标；
- 内置 runtime health、breaker、同路由候选内 failover，可在失败时先耗尽当前候选内其他 eligible upstream，再考虑下一候选；
- 支持局域网 / Tailscale 下的 **central relay** 形态，但不会假装远程设备天然拥有宿主机的 transcript/session 文件访问能力。

如果你之前把它理解成“一个帮我切换 config.toml 的本地代理”，现在可以把它理解成：

> `Codex CLI -> codex-helper data plane -> provider/routing/profile/session control plane`

---

## 三个核心概念

### 1. Provider / Routing

`provider` 是一个中转或上游账号的目录项；`routing` 决定是固定一个 provider、按顺序兜底，还是优先匹配某些标签后再降级。

兼容说明：

- 代码和部分运行时视图里仍然会看到 `station` / `config` 这些名字；
- 在当前公开配置面里，新增上游用 `provider`，选择策略用 `routing`。

### 2. Profile

`profile` 是可复用的控制模板，用来表达“我想怎么跑这类会话”，例如：

- 目标 provider / routing
- 目标 model
- `service_tier` / fast mode
- `reasoning_effort`

适合理解为“意图模板”，而不是单纯的 provider 预设。

### 3. Session Binding / Override

`session` 是控制平面的核心对象。你现在可以：

- 看单会话 identity card；
- 对单会话应用 profile；
- 对单会话覆盖 `model / reasoning_effort / service_tier / routing target`；
- 区分值到底来自：
  - session override
  - profile default
  - request payload
  - provider mapping
  - runtime fallback

这也是为什么“先知道当前 Codex 会话对应哪个对象”现在已经变成一等能力，而不是靠猜。

---

## 控制平面快速入口

如果你只是想直接用功能，不想先翻完整文档，可以先记住这几个入口：

- TUI / GUI
  - `Stations`：看 station、能力、健康、breaker、快速切换
  - `Sessions`：看 session identity、effective route、session override
  - `Profiles` / Config：管理 profile、provider/routing 结构
- 只读 API
  - `GET /__codex_helper/api/v1/capabilities`
  - `GET /__codex_helper/api/v1/snapshot`
  - `GET /__codex_helper/api/v1/sessions`
  - `GET /__codex_helper/api/v1/sessions/{session_id}`
- 控制 API
  - `GET/POST /__codex_helper/api/v1/overrides/session`
  - `POST /__codex_helper/api/v1/overrides/session/profile`
  - `GET /__codex_helper/api/v1/profiles`
  - `GET /__codex_helper/api/v1/stations`
  - `GET/POST /__codex_helper/api/v1/retry/config`

如果你正在做重构对照或想看设计边界，建议直接读：

- `docs/workstreams/codex-control-plane-refactor/README.md`
- `docs/workstreams/codex-control-plane-refactor/CENTRAL_RELAY.md`
- `docs/workstreams/codex-routing-config-refactor/CONFIGURATION.md`（routing-first 配置指南）

---

## LAN / Tailscale 中央中转模式

推荐的共享形态不是“远程附着桌面”，而是：

1. 一台常开机器运行 `codex-helper`，同时提供 proxy 和 admin/control-plane；
2. 其他设备把 Codex 请求发到这台机器的 proxy 端口；
3. GUI 或未来 WebUI 连接 admin 端口做控制。

当前边界很明确：

- 可以共享：
  - routing/profile 管理
  - session identity
  - observed request history
  - session override
  - health / breaker / probe
- 不能天然共享：
  - 宿主机 `~/.codex/sessions`
  - transcript 文件浏览
  - 本地路径打开

远程 admin 的安全边界：

- loopback 默认不需要 token；
- 非 loopback 访问需要在宿主机设置 `CODEX_HELPER_ADMIN_TOKEN`；
- 客户端通过请求头 `x-codex-helper-admin-token` 发送同一个 token。

如果你要把它放到局域网 / Tailscale 里，这一段比下面的“多上游切换”更值得先读。

---

## 常见配置：provider + routing

当前推荐的配置模型是 `version = 3`：`provider` 只描述账号、认证、endpoint 和标签；`routing` 只描述顺序、首选和兜底策略。旧的 `active/level/station` 仍可读取和迁移，但不再是公共写入面。

最常见的流程：

```bash
codex-helper config init
codex-helper provider add input --base-url https://ai.input.im/v1 --auth-token-env INPUT_API_KEY --tag billing=monthly
codex-helper provider add openai --base-url https://api.openai.com/v1 --auth-token-env OPENAI_API_KEY --tag billing=paygo
codex-helper routing order input openai
codex-helper config set-retry-profile balanced
```

常用策略：

| 场景目标 | 推荐配置 | 说明 |
| --- | --- | --- |
| 固定只用一个供应商 | `codex-helper routing pin <provider>` | 手动粘住；如果该 provider 不可用，请手动切换 |
| 按顺序兜底 | `codex-helper routing order a b c` | 最直观，适合“这个中转不能用就换下一个” |
| 包月优先、按量兜底 | `codex-helper routing prefer-tag --tag billing=monthly --order paygo --on-exhausted continue` | provider 用标签表达业务含义；包月全耗尽后继续兜底 |
| 包月全耗尽即停止 | 同上但 `--on-exhausted stop` | 防止误走按量线路 |

对应的 TOML 很薄：

```toml
version = 3

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.openai]
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo" }

[codex.routing]
policy = "ordered-failover"
order = ["input", "openai"]
on_exhausted = "continue"

[retry]
profile = "balanced"
```

迁移旧配置：

```bash
codex-helper config migrate --dry-run
codex-helper config migrate --write --yes
```

`routing list` / `routing explain` 用于查看编译后的运行时视图；新增 provider、调整顺序、启用禁用都使用 `provider` 和 `routing` 命令。

---

## 常用命令速查表

### 日常使用

- 启动 Codex 助手（推荐）：
  - `codex-helper` / `ch`
- 显式启动 Codex 代理：
  - `codex-helper serve`（默认端口 3211）
  - `codex-helper serve --no-tui`（关闭内置 TUI 面板）
  - `codex-helper serve --host 0.0.0.0`（监听所有网卡；注意安全风险）

### 开关 Codex

- 一次性让 Codex 指向本地代理：

  ```bash
  codex-helper switch on
  ```

- 从备份恢复原始配置：

  ```bash
  codex-helper switch off
  ```

- 查看当前开关状态：

  ```bash
  codex-helper switch status
  ```

### 配置管理（provider / routing）

- 初始化或迁移配置：

  ```bash
  codex-helper config init
  codex-helper config migrate --dry-run
  codex-helper config migrate --write --yes
  ```

- 从 Codex CLI 导入账号/配置：

  ```bash
  codex-helper config import-from-codex --force
  codex-helper config overwrite-from-codex --dry-run
  codex-helper config overwrite-from-codex --yes
  ```

- 添加和查看 provider：

  ```bash
  codex-helper provider add openai-main \
    --base-url https://api.openai.com/v1 \
    --auth-token-env OPENAI_API_KEY \
    --alias "OpenAI 主额度"
  codex-helper provider list
  codex-helper provider show openai-main
  ```

- 调整 routing：

  ```bash
  codex-helper routing order openai-main packy-main
  codex-helper routing pin openai-main
  codex-helper routing prefer-tag --tag billing=monthly --order openai-main --on-exhausted continue
  codex-helper routing show
  ```

- 启用 / 禁用 provider：

  ```bash
  codex-helper provider disable packy-main
  codex-helper provider enable packy-main
  ```

- 设置重试策略预设（写入 `[retry]` 段，适合“只选策略，不想调一堆参数”的用法）：

  ```bash
  codex-helper config set-retry-profile balanced
  codex-helper config set-retry-profile cost-primary
  ```

- 查看编译后的运行时视图：

  ```bash
  codex-helper routing list
  codex-helper routing explain
  ```

### TUI 设置页（运行态）

- `R`：立即重载运行态配置（用于确认手动修改已生效；下一次请求将使用新配置）

### 会话、用量与诊断

- 会话助手（Codex）：

  ```bash
  codex-helper session list
  codex-helper session recent
  codex-helper session last
  codex-helper session transcript <ID> --tail 40
  ```

- 请求用量 / 日志：

  ```bash
  codex-helper usage summary
  codex-helper usage tail --limit 20
  codex-helper usage tail --limit 20 --raw
  codex-helper usage find --errors --model gpt-5 --retried --limit 10
  codex-helper usage find --session <SESSION_ID> --raw
  ```

  普通文本输出会展示 station/provider/model、service_tier/fast、input/output/cache/reasoning token、耗时、TTFB、输出速度和可估算成本；`usage find` 可按 session/model/station/provider/status/fast/retry 过滤，`--raw` 仍输出原始 JSONL。

- 状态与诊断：

  ```bash
  codex-helper status
  codex-helper doctor

  # JSON 输出，方便脚本 / UI 集成
  codex-helper status --json | jq .
  codex-helper doctor --json | jq '.checks[] | select(.status != "ok")'
  ```

---

## 典型场景示例

### 场景 1：多中转 / 多 key 集中管理 + 快速切换

```bash
# 1. 为不同供应商添加 provider
codex-helper provider add openai-main \
  --base-url https://api.openai.com/v1 \
  --auth-token-env OPENAI_API_KEY \
  --alias "OpenAI 主额度"

codex-helper provider add packy-main \
  --base-url https://codex-api.packycode.com/v1 \
  --auth-token-env PACKYCODE_API_KEY \
  --alias "Packy 中转" \
  --tag billing=monthly

codex-helper provider list

# 2. 选择路由方式
codex-helper routing pin openai-main          # 固定使用 OpenAI
codex-helper routing order packy-main openai-main  # Packy 优先，OpenAI 兜底

# 3. 一次性让 Codex 使用本地代理（只需执行一次）
codex-helper switch on

# 4. 按当前 routing 启动代理
codex-helper
```

### 场景 2：按项目快速恢复 Codex 会话

```bash
cd ~/code/my-app

codex-helper session list   # 列出与当前项目相关的最近会话
codex-helper session recent # 跨项目列出最近会话（每行：project_root + session_id）
codex-helper session last   # 给出最近一次会话 + 对应 resume 命令
codex-helper session transcript <ID> --tail 40   # 查看最近对话，用于辨认某个 session
```

`session list` 会额外展示每个会话的轮数（rounds）与最后更新时间（last_update，优先取最后一次 assistant 响应时间）。

小技巧：`session list` 默认会完整输出 first prompt；如果你想让列表更紧凑，可以手动截断：

```bash
codex-helper session list --truncate 120
```

`session recent` 用于你在多个仓库之间频繁切换时快速 `codex resume`：默认筛选最近 12 小时内有更新（基于 session 文件 mtime）的会话，并按新到旧输出：

```bash
codex-helper session recent --since 12h --limit 50
# <project_root> <session_id>
```

脚本集成建议优先使用 TSV/JSON 输出，避免解析歧义：

```bash
codex-helper session recent --format tsv
codex-helper session recent --format json
```

Windows 下也可以直接打开每个会话（best-effort）：

```bash
codex-helper session recent --open --terminal wt --shell pwsh --resume-cmd "codex resume {id}"
```

你也可以从任意目录查询指定项目的会话：

```bash
codex-helper session list --path ~/code/my-app
codex-helper session last --path ~/code/my-app
```

这在你有多个 side project 时尤其方便：不需要记忆 session ID，只要告诉 codex-helper 你关心的目录，它会优先匹配该目录及其父/子目录下的会话，并给出 `codex resume <ID>` 命令。

---

## 进阶配置（可选）

大部分用户只需要前面的命令即可。如果你想做更细粒度的定制，可以关注这几个文件：

- 主配置：`~/.codex-helper/config.toml`（优先）或 `~/.codex-helper/config.json`（兼容）
- 请求过滤：`~/.codex-helper/filter.json`
- 用量提供商：`~/.codex-helper/usage_providers.json`
- 价格覆盖：`~/.codex-helper/pricing_overrides.toml`
- 请求日志：`~/.codex-helper/logs/requests.jsonl`
- 详细调试日志（可选）：`~/.codex-helper/logs/requests_debug.jsonl`（仅在启用 `http_debug` 拆分时生成）
- 会话统计缓存（自动生成）：`~/.codex-helper/cache/session_stats.json`（用于加速 `session list/search` 的轮数/时间统计；以 session 文件 `mtime+size` 作为失效条件，如怀疑不准可直接删除该文件强制重建）

如果你希望快速生成一个带注释的 TOML 默认模板：

```bash
codex-helper config init
```

> 说明：
> - 模板注释默认是中文；
> - 如果检测到 `~/.codex/config.toml`，会 best-effort 自动把 Codex providers 导入到生成的 `config.toml`；
> - 只想生成纯模板（不导入）可用：`codex-helper config init --no-import`。

Codex 官方文件：

- `~/.codex/auth.json`：由 `codex login` 维护，codex-helper 只读取，不写入；
- `~/.codex/config.toml`：由 Codex CLI 维护，codex-helper 仅在 `switch on/off` 时有限修改。

### 配置文件简要结构（推荐 TOML）

codex-helper 支持 `config.toml` 与 `config.json`；如同时存在，以 `config.toml` 为准。新配置推荐使用 `version = 3` 的 routing-first 结构：

```toml
version = 3

[codex.providers.openai-main]
alias = "主 OpenAI 额度"
base_url = "https://api.openai.com/v1"
auth_token_env = "OPENAI_API_KEY"
tags = { billing = "paygo", vendor = "openai" }

[codex.providers.packy-main]
alias = "Packy 中转"
base_url = "https://codex-api.packycode.com/v1"
auth_token_env = "PACKYCODE_API_KEY"
tags = { billing = "monthly", vendor = "packy" }

[codex.routing]
policy = "ordered-failover"
order = ["packy-main", "openai-main"]
on_exhausted = "continue"
```

关键点：

- `providers`：按名称索引的供应商 / 中转配置，单 endpoint 直接写 `base_url`；
- `tags`：业务标签，例如 `billing=monthly`，用于 `routing prefer-tag`；
- `routing.order`：明确的兜底顺序，越靠前越优先；
- `routing.policy`：`manual-sticky`、`ordered-failover` 或 `tag-preferred`；
- `station` 只保留为运行时视图，不再作为新增/切换 provider 的写入入口。

### 价格覆盖（Pricing Overrides）

内置价格目录覆盖常见 Codex/OpenAI 模型；如果你使用的中转商模型别名、价格或倍率不同，可以添加 `~/.codex-helper/pricing_overrides.toml`。该文件会覆盖内置同名模型，也可以新增模型，成本计算和 GUI/TUI 价格目录都会使用合并后的结果。

```toml
[models.gpt-5]
display_name = "GPT-5 via relay"
aliases = ["relay-gpt5"]
input_per_1m_usd = "1.10"
output_per_1m_usd = "8.80"
cache_read_input_per_1m_usd = "0.11"
cache_creation_input_per_1m_usd = "0"
confidence = "estimated"

[models.custom-codex]
input_per_1m_usd = "0.50"
output_per_1m_usd = "1.50"
```

也可以在本机运行 GUI 代理时，从 `Stats -> Pricing catalog` 把目录行保存为本地覆盖，或从 `Stats -> Local pricing overrides` 直接编辑本地覆盖；附着远端代理时该区域保持只读，避免误写当前机器但不影响远端代理。

也可以用 CLI 管理该文件，避免手动拼 TOML：

```bash
codex-helper pricing path
codex-helper pricing list
codex-helper pricing list --local --model gpt-5
codex-helper pricing set custom-codex --input-per-1m-usd 0.50 --output-per-1m-usd 1.50 --confidence estimated
codex-helper pricing sync http://127.0.0.1:4322/__codex_helper/api/v1/pricing/catalog --model relay-gpt5 --dry-run
codex-helper pricing sync http://127.0.0.1:4322/__codex_helper/api/v1/pricing/catalog --model relay-gpt5
codex-helper pricing sync-basellm --model gpt-5 --dry-run
codex-helper pricing sync-basellm --model gpt-5
codex-helper pricing remove custom-codex
```

`pricing sync` 拉取的是 `ModelPriceCatalogSnapshot` JSON（也就是本项目 admin API 暴露的价格目录格式），默认合并到本地覆盖；加 `--replace` 会用远端匹配结果替换本地覆盖文件。

`pricing sync-basellm` 拉取 `https://basellm.github.io/llm-metadata/api/all.json`，把其中的 per-million 模型价格转换为本项目的本地覆盖格式。这个命令适合定期刷新模型价格，避免长期依赖内置种子价格；本地覆盖仍然优先于内置表。

### 用量提供商（Usage Providers）

路径：`~/.codex-helper/usage_providers.json`，示例：

```jsonc
{
  "providers": [
    {
      "id": "packycode",
      "kind": "budget_http_json",
      "domains": ["packycode.com"],
      "endpoint": "https://www.packycode.com/api/backend/users/info",
      "token_env": null,
      "poll_interval_secs": 60
    },
    {
      "id": "my-sub2api",
      "kind": "openai_balance_http_json",
      "domains": ["relay.example.com"],
      "endpoint": "{{base_url}}/user/balance",
      "poll_interval_secs": 60
    },
    {
      "id": "my-new-api",
      "kind": "new_api_user_self",
      "domains": ["newapi.example.com"],
      "endpoint": "{{base_url}}/api/user/self",
      "token_env": "NEW_API_ACCESS_TOKEN",
      "headers": {
        "New-Api-User": "{{userId}}"
      },
      "variables": {
        "userId": "{{env:NEW_API_USER_ID}}"
      },
      "poll_interval_secs": 60
    }
  ]
}
```

行为简述：

- upstream 的 `base_url` host 匹配 `domains` 中任一项，即视为该 provider 的管理对象；
- 调用 `endpoint` 的认证 token 优先来自 `token_env`，否则尝试使用绑定 upstream 的 `auth.auth_token` / `auth.auth_token_env`（运行时从环境变量解析）；
- `endpoint` 支持 `{{base_url}}`、`{{upstream_base_url}}`、`{{token}}` / `{{apiKey}}` / `{{accessToken}}`、`{{env:NAME}}` 和 `variables` 模板；`{{base_url}}` 会自动去掉常见的尾部 `/v1`；
- `openai_balance_http_json` 适配 cc-switch 通用模板 / 常见 sub2api：默认请求 `{{base_url}}/user/balance`，解析 `balance`、`remaining`、`credit`、`subscription_balance`、`pay_as_you_go_balance` 等常见字段；
- `new_api_user_self` 适配 New API：默认请求 `{{base_url}}/api/user/self`，按 cc-switch 模板解析 `data.quota` / `data.used_quota`，默认除以 `500000` 转成 USD；
- 自研接口可以通过 `extract.remaining_balance_paths`、`extract.monthly_spent_paths`、`extract.monthly_budget_paths`、`extract.exhausted_paths` 和 divisor 字段扩展，不需要改 Rust 代码；
- `refresh_on_request` 控制请求结束后是否自动查询额度，默认 `true`；设为 `false` 可关闭该 provider 的请求后自动刷新；
- `poll_interval_secs` 控制该 provider 两次额度查询之间的最小间隔，省略时默认 `60`；当前触发点是请求结束后的按需轮询，不跟随 TUI/GUI 的界面刷新频率，低于 20 会按 20 秒处理，设为 `0` 可禁用请求后自动刷新；
- `POST /__codex_helper/api/v1/providers/balances/refresh` 可手动触发同一套 core adapter 余额刷新；可用 query 参数 `station_name` / `provider_id` 定向刷新，手动刷新不受 `refresh_on_request` 和请求后节流限制；
- 请求结束后，codex-helper 按需调用 `endpoint` 查询额度，记录 `ok` / `exhausted` / `stale` / `error` / `unknown` 余额快照；
- 当额度用尽时，对应 upstream 在 LB 中被标记为 `usage_exhausted = true`，优先避开该线路。

### 请求过滤与日志

- 过滤规则：`~/.codex-helper/filter.json`，例如：

  ```jsonc
  [
    { "op": "replace", "source": "your-company.com", "target": "[REDACTED_DOMAIN]" },
    { "op": "remove",  "source": "super-secret-token" }
  ]
  ```

  请求 body 在发出前会按规则进行字节级替换 / 删除，规则根据文件 mtime 约 1 秒内自动刷新。

- 请求日志：`~/.codex-helper/logs/requests.jsonl`，每行一个 JSON，字段包括：
  - `service`（目前为 `codex`）、`method`、`path`、`status_code`、`duration_ms`；
  - `config_name`、`upstream_base_url`；
  - `usage`（input/output/total_tokens 等）。
  - （可选）`retry`：发生重试/切换上游时记录重试次数与尝试链路（便于回溯问题）。
  - （可选）`http_debug`：用于排查 4xx/5xx 时记录更完整的请求/响应信息（请求头、请求体预览、上游响应头/响应体预览等）。
  - （可选）`http_debug_ref`：当启用拆分写入时，主日志只保存引用，详细内容写入 `requests_debug.jsonl`。

  你可以通过环境变量启用该调试日志（默认关闭）：

  - `CODEX_HELPER_HTTP_DEBUG=1`：仅当上游返回非 2xx 时写入 `http_debug`；
  - `CODEX_HELPER_HTTP_DEBUG_ALL=1`：对所有请求都写入 `http_debug`（更容易产生日志膨胀）；
  - `CODEX_HELPER_HTTP_DEBUG_BODY_MAX=65536`：请求/响应 body 预览的最大字节数（会截断）。
  - `CODEX_HELPER_HTTP_DEBUG_SPLIT=1`：将 `http_debug` 大对象拆分写入 `requests_debug.jsonl`，主 `requests.jsonl` 仅保留 `http_debug_ref`（推荐在 `*_ALL=1` 时开启）。

  另外，你也可以让代理在终端直接输出更完整的非 2xx 调试信息（同样默认关闭）：

  - `CODEX_HELPER_HTTP_WARN=1`：当上游返回非 2xx 时，以 `warn` 级别输出一段裁剪后的 `http_debug` JSON；
  - `CODEX_HELPER_HTTP_WARN_ALL=1`：对所有请求都输出（不建议，容易泄露/刷屏）；
  - `CODEX_HELPER_HTTP_WARN_BODY_MAX=65536`：终端输出里 body 预览的最大字节数（会截断）。

  注意：敏感请求头会自动脱敏（例如 `Authorization`/`Cookie` 等）；如需进一步控制请求体中的敏感信息，建议配合 `~/.codex-helper/filter.json` 使用。

### 两层重试与切换（默认：每个 upstream 2 次尝试；最多尝试 2 个 config/provider；同一 config 内会在多个 upstream 间切换）

有些上游错误（例如网络抖动、429 限流、5xx/524、或看起来像 Cloudflare/WAF 的拦截页）可能是瞬态的；codex-helper 在**未开始向客户端输出响应**前按“两层模型”执行：先在当前 provider/config 内做 upstream 级重试，仍失败再做 provider/config 级 failover（例如 401/403/404/408 等路由/认证类错误也会触发切换）。

- 强烈建议将 Codex 侧 `model_providers.codex_proxy.request_max_retries = 0`，让“重试与切换”主要由 codex-helper 负责，避免 Codex 默认 5 次重试把同一个 502 反复打满（`switch on` 会在该字段不存在时写入 0；如你手动改过，则不会覆盖）。
- 主配置（`~/.codex-helper/config.toml` / `config.json`）的 `[retry]` 段用于设置全局默认值（从 `v0.8.0` 起不再支持通过环境变量覆盖 retry 参数）。

配置示例（TOML，两层可分别覆盖；profile 默认 `balanced`）：

```toml
[retry]
profile = "balanced"

[retry.upstream]
max_attempts = 2
strategy = "same_upstream"
backoff_ms = 200
backoff_max_ms = 2000
jitter_ms = 100
on_status = "429,500-599,524"
on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge"]

[retry.provider]
max_attempts = 2
strategy = "failover"
on_status = "401,403,404,408,429,500-599,524"
on_class = ["upstream_transport_error"]

never_on_status = "413,415,422"
never_on_class = ["client_error_non_retryable"]
cloudflare_challenge_cooldown_secs = 300
cloudflare_timeout_cooldown_secs = 60
transport_cooldown_secs = 30
cooldown_backoff_factor = 1
cooldown_backoff_max_secs = 600
```

注意：重试可能导致 **POST 请求重放**（例如重复计费/重复写入）。建议仅在你明确接受这一风险、且错误大多是瞬态的场景下开启，并将尝试次数控制在较小范围内。

### 日志文件大小控制（推荐）

`requests.jsonl` 默认会持续追加，为避免长期运行导致文件过大，codex-helper 支持自动轮转（默认开启）：

- `CODEX_HELPER_REQUEST_LOG_MAX_BYTES=52428800`：单个日志文件最大字节数，超过会自动轮转（`requests.jsonl` → `requests.<timestamp_ms>.jsonl`；`requests_debug.jsonl` → `requests_debug.<timestamp_ms>.jsonl`）（默认 50MB）；
- `CODEX_HELPER_REQUEST_LOG_MAX_FILES=10`：最多保留多少个历史轮转文件（默认 10）；
- `CODEX_HELPER_REQUEST_LOG_ONLY_ERRORS=1`：只记录非 2xx 请求（可显著减少日志量，默认关闭）。

这些字段是稳定契约，后续版本只会在此基础上追加字段，不会删除或改名，方便脚本长期依赖。

---

## 与 cli_proxy / cc-switch 的关系

- [cli_proxy](https://github.com/guojinpeng/cli_proxy)：多服务守护进程 + Web UI，看板 + 管理功能很全面；
- [cc-switch](https://github.com/farion1231/cc-switch)：桌面 GUI 级供应商 / MCP 管理器，主打“一处管理、按需应用到各客户端”。

codex-helper 借鉴了它们的设计思路，但定位更轻量：

- 专注 Codex CLI；
- 单一二进制，无守护进程、无 Web UI；
- 更适合作为你日常使用的“命令行小助手”，或者集成进你自己的脚本 / 工具链中。

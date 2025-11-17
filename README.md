# codex-helper（Codex CLI 本地助手 / 本地代理）

> 基于 Rust 的 Codex 本地助手 / 本地代理，专门为 Codex CLI 设计。  
> 帮你在本地统一管理 **多供应商 / 多 key / 多端点**，在额度用完或失败时自动切换，并提供便捷的会话工具。

> English version: [README_EN.md](README_EN.md)

## 你可以用 codex-helper 做什么？

- **一键让 Codex 走本地代理**
  - `codex-helper switch-on` 一次切换，Codex CLI 所有请求都经过本地代理；
  - 自动备份 `~/.codex/config.toml`，用 `switch-off` 随时恢复。

- **集中管理多个 key / 供应商 / 中转站**
  - 在 `~/.codex-proxy/config.json` 里维护多套 Codex 配置（openai / packy / 自建中转等）；
  - 每套配置下可以挂多个 upstream（号池），按顺序作为 primary/backup。

- **用量“用完自动切换”**
  - 内置 usage provider 机制（默认适配 packy）：
    - 定期或按需查询额度；
    - 当检测到某个供应商额度用尽时，自动把对应 upstream 标记为“用尽”，优先走其他线路；
    - 所有线路都用尽时仍然会兜底选一个，不会彻底断流。

- **命令行下快速找回会话**
  - `codex-helper session list`：按“当前项目（cwd/父目录/子目录）”智能列出最近的 Codex 会话；
  - `codex-helper session last`：直接给出“当前项目最后一次会话”以及对应的 `codex resume <ID>` 命令。

- **统一的请求过滤与日志**
  - 在 `filter.json` 配好敏感信息替换 / 删除规则，所有 Codex 请求发出前统一脱敏；
  - 请求日志统一写到 `~/.codex-proxy/logs/requests.jsonl`，便于用 `jq` 等工具做用量分析和排障。

- **（实验性）Claude Code 支持**
  - 基于 `~/.claude/settings.json` 自动引导 Claude 上游配置；
  - 支持 `codex-helper switch-on --claude` / `serve --claude` 将 Claude Code 指向本地代理，并带备份与 Guard；
  - 由于 Claude 自身更新节奏较快，这部分行为暂时视为实验特性。

## 快速开始

### 1. 构建与安装

```bash
cargo build --release
```

生成的可执行文件：

```bash
target/release/codex-helper
```

将其加入 `PATH`，即可直接运行 `codex-helper`。

### 2. 一次性让 Codex 使用本地代理

```bash
codex-helper switch-on
```

- 读取 `~/.codex/config.toml`；
- 如尚未备份，则复制为 `~/.codex/config.toml.codex-proxy-backup`；
- 写入/覆盖下列配置，并将 `model_provider` 指向它：

  ```toml
  [model_providers.codex_proxy]
  name = "codex-helper"
  base_url = "http://127.0.0.1:3211"
  wire_api = "responses"

  model_provider = "codex_proxy"
  ```

- 自定义端口：

  ```bash
  codex-helper switch-on --port 4000
  ```

恢复原始 Codex 配置：

```bash
codex-helper switch-off
```

### 3. 启动代理服务

Codex 代理（默认 3211）：

```bash
codex-helper serve
codex-helper serve --port 3211
```

服务启动后：

- 监听 `127.0.0.1:<port>`；
- Codex 模式下，首次运行会尝试从 `~/.codex/config.toml` 与 `~/.codex/auth.json` 推导默认上游：
  - 使用当前 `model_provider`；
  - 使用其 `env_key` / `auth.json` 中的 token 作为上游 `auth_token`；
- 如无法解析到有效 token，会直接报错退出（fail-fast），避免“默默每次 401/403”的情况。

### 4. 智能 session 辅助（Codex）

查看当前项目相关的最近会话：

```bash
codex-helper session list
codex-helper session list --limit 20
```

- 会从 `~/.codex/sessions/**/rollout-*.jsonl` 中读取会话；
- 优先匹配当前目录 / 父目录 / 子目录的 `cwd`；
- 每条会话展示：
  - `id`：完整会话 ID，单独一行方便复制；
  - `updated`：最后更新时间；
  - `cwd`：会话所属工作目录；
  - `prompt`：首条用户消息的简短预览（截断到 80 字符）。

快速定位“当前项目最近一次会话”并给出 resume 命令：

```bash
codex-helper session last
```

示例输出：

```text
Last Codex session for current project:
  id: 1234-...-abcd
  updated_at: 2025-04-01T10:30:00Z
  cwd: /Users/you/project
  first_prompt: 你的第一条消息...

Resume with:
  codex resume 1234-...-abcd
```

你也可以针对任意目录查询会话（而不必 cd 进去）：

```bash
codex-helper session list --path ~/code/my-app
codex-helper session last --path ~/code/my-app
```

## 典型工作流示例

### 1. 多供应商 / 多 key 集中管理 + 快速切换

假设你有多家供应商/代理（OpenAI 官方、Packy 中转、自建代理等），希望在本地统一管理并在需要时一条命令切换：

```bash
# 1. 为不同供应商添加配置
codex-helper config add openai-main \
  --base-url https://api.openai.com/v1 \
  --auth-token sk-openai-xxx \
  --alias "OpenAI 主额度"

codex-helper config add packy-main \
  --base-url https://codex-api.packycode.com/v1 \
  --auth-token sk-packy-yyy \
  --alias "Packy 中转"

codex-helper config list

# 2. 全局选择当前使用的供应商（active 配置）
codex-helper config set-active openai-main   # 使用 OpenAI
# 或者
codex-helper config set-active packy-main    # 使用 Packy

# 3. 一次性让 Codex 使用本地代理（只需执行一次）
codex-helper switch-on

# 4. 在当前 active 配置下启动代理
codex-helper serve
```

对于大部分“有很多 key / 代理”的用户，这样就可以在一个 JSON + 少量命令中集中管理所有上游，并按需快速切换。

### 2. 按项目快速恢复 Codex 会话

当你回到某个项目目录，希望快速恢复之前的 Codex 会话，可以这样使用：

```bash
cd ~/code/my-app

# 列出当前项目相关的最近会话
codex-helper session list

# 找到“当前项目”最近一次会话并给出 resume 命令
codex-helper session last
```

你也可以从任意位置查询指定项目的会话：

```bash
codex-helper session list --path ~/code/my-app
codex-helper session last --path ~/code/my-app
```

这在你有多个 side project 时尤其方便：不需要记忆 session ID，只要告诉 codex-helper 你关心的目录，它就会优先匹配该目录及其父/子目录下的会话，并给出 `codex resume <ID>` 命令。

## 配置文件与命令

### 配置文件位置

- 主配置：`~/.codex-proxy/config.json`
  - `codex`：Codex 上游配置。
- 请求过滤：`~/.codex-proxy/filter.json`
- 用量提供商：`~/.codex-proxy/usage_providers.json`
- 请求日志：`~/.codex-proxy/logs/requests.jsonl`

### `config.json` 示例

```jsonc
{
  "codex": {
    "active": "openai-main",
    "configs": {
      "openai-main": {
        "name": "openai-main",
        "alias": "主 OpenAI 额度",
        "upstreams": [
          {
            "base_url": "https://api.openai.com/v1",
            "auth": {
              "auth_token": "sk-...",
              "api_key": null
            },
            "tags": {
              "source": "codex-config",
              "provider_id": "openai"
            }
          }
        ]
      }
    }
  }
}
```

- `name`：配置 ID（也是 `configs` 的 key）。
- `alias`：可选展示名称。
- `upstreams`：上游池（顺序 = 优先级）：
  - `base_url`：上游 API 地址；
  - `auth.auth_token`：用于 `Authorization: Bearer <token>`；
  - `auth.api_key`：可选，用于某些 `x-api-key` 风格鉴权；
  - `tags`：任意键值对，便于标记来源等元信息。

### 配置管理命令

列出配置：

```bash
codex-helper config list
```

新增配置：

```bash
# Codex
codex-helper config add openai-main \
  --base-url https://api.openai.com/v1 \
  --auth-token sk-xxx \
  --alias "主 OpenAI 额度"
```

切换激活配置：

```bash
codex-helper config set-active openai-main
```

## 用量提供商（Usage Providers）

配置文件：`~/.codex-proxy/usage_providers.json`，示例：

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
    }
  ]
}
```

- `domains`：只要 upstream 的 `base_url` host 匹配这些域名，即视为属于该 provider；
- `endpoint`：用量查询 API 地址；
- `token_env`：如设置且环境变量非空，则优先使用该值作为 Bearer token；
- 否则，从关联的 upstream 中取第一个非空的 `auth.auth_token`。

对于 `budget_http_json`：

- 用量接口返回的 JSON 中：
  - 读取 `monthly_budget_usd` 与 `monthly_spent_usd`；
  - 若 `monthly_budget_usd > 0` 且 `monthly_spent_usd >= monthly_budget_usd`，视为额度用尽；
- 对应 upstream 在 LB 状态中被标记为 `usage_exhausted = true`；
- LB 会尽量避开这些 upstream；若所有 upstream 都用尽，则忽略“用尽”标记，仅按失败熔断规则兜底。

## 请求过滤与日志

### 请求过滤：`~/.codex-proxy/filter.json`

```jsonc
[
  { "op": "replace", "source": "your-company.com", "target": "[REDACTED_DOMAIN]" },
  { "op": "remove",  "source": "super-secret-token" }
]
```

- 在转发到上游前，对请求 body 做字节级替换 / 删除；
- 过滤规则缓存在内存中，但会根据文件 mtime 约 1 秒内自动刷新。

### 请求日志：`~/.codex-proxy/logs/requests.jsonl`

每行是一个 JSON 对象，例如：

```jsonc
{
  "timestamp_ms": 1730000000000,
  "service": "codex",
  "method": "POST",
  "path": "/v1/responses",
  "status_code": 200,
  "duration_ms": 1234,
  "config_name": "openai-main",
  "upstream_base_url": "https://api.openai.com/v1",
  "usage": {
    "input_tokens": 123,
    "output_tokens": 456,
    "reasoning_tokens": 0,
    "total_tokens": 579
  }
}
```

你可以用 `jq` 等工具按配置名 / 上游 / 时间窗口做用量分析或问题排查。

## 与 cli_proxy / cc-switch 的关系

codex-helper 借鉴了 [cli_proxy](https://github.com/guojinpeng/cli_proxy) 和 [cc-switch](https://github.com/farion1231/cc-switch) 的设计思路，在此基础上提供了一个更轻量、面向 Codex CLI 的 Rust 本地代理与配置管理工具。

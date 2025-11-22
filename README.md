# codex-helper（Codex CLI 本地助手 / 本地代理）

> 让 Codex / Claude Code 走一层本地“保险杠”：  
> 集中管理所有中转站 / key / 配额，在额度用完或上游挂掉时自动切换，并提供会话与脱敏辅助工具。

> English version: `README_EN.md`

---

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

- **想给 Codex/Claude 加一层本地脱敏和统一日志**  
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
- 在修改前检查 `~/.codex/config.toml`，如已指向本地代理且存在备份，会询问是否先恢复原始配置；
- 必要时修改 `model_provider` 与 `model_providers.codex_proxy`，让 Codex 走本地代理，并只在首次写入备份；
- 如果 `~/.codex-helper/config.json` 还没初始化，会尝试根据 `~/.codex/config.toml` + `auth.json` 推导一个默认上游；
- 用 Ctrl+C 优雅退出时，尝试从备份恢复原始 Codex 配置。

从此之后，你继续用原来的 `codex` 命令即可，所有请求会自动经过 codex-helper。

### 3. 可选：把“默认目标”切成 Claude（实验）

默认所有命令都以 Codex 为主。如果你主要用 Claude Code，可以这样调整：

```bash
codex-helper default --claude   # 将默认目标服务改为 Claude（实验）
```

之后：

- `codex-helper serve`（不加参数）会默认启动 Claude 代理（端口 3210）；
- `codex-helper config list/add/set-active`（不加 `--codex/--claude`）默认操作 Claude 的配置。

随时可以用 `codex-helper default` 查看当前默认目标服务。

---

## 常见配置：多上游自动切换

最常见、也是最“物有所值”的用法，是让 codex-helper 在多个上游之间自动切换：

- 某条线路频繁失败（例如 5xx / 连接失败）；
- 或被用量提供商标记为“额度用尽”（`usage_exhausted = true`）；
- 在这种情况下，LB 会优先选择同一配置下的其他 upstream 作为备份。

**关键点：主线路 + 备份线路要放在同一个配置的 `upstreams` 里，而不是拆成两个平级配置。**

示例（`~/.codex-helper/config.json`）：

```jsonc
{
  "version": 1,
  "codex": {
    "active": "codex-main",
    "configs": {
      "codex-main": {
        "name": "codex-main",
        "alias": null,
        "upstreams": [
          {
            "base_url": "https://codex-api.packycode.com/v1",
            "auth": { "auth_token": "sk-packy-..." },
            "tags": { "provider_id": "packycode", "source": "codex-config" }
          },
          {
            "base_url": "https://co.yes.vg/v1",
            "auth": { "auth_token": "cr-..." },
            "tags": { "provider_id": "yes", "source": "codex-config" }
          }
        ]
      }
    }
  },
  "claude": { "active": null, "configs": {} },
  "default_service": null
}
```

在这份配置下：

- `active = "codex-main"` → 负载均衡器会在 `upstreams[0]`（Packy）和 `upstreams[1]`（Yes）之间选择；
- 当某个 upstream：
  - 连续失败达到阈值（`FAILURE_THRESHOLD`，见 `src/lb.rs`），或
  - 被 `usage_providers` 标记为 `usage_exhausted = true`  
  时，LB 会优先避开它，尽量选择列表中的其他 upstream；
- 所有 upstream 都被视为“不可用”时，仍会兜底返回第一个，避免完全断流。

---

## 常用命令速查表

### 日常使用

- 启动 Codex 助手（推荐）：
  - `codex-helper` / `ch`
- 显式启动 Codex / Claude 代理：
  - `codex-helper serve`（Codex，默认端口 3211）
  - `codex-helper serve --codex`
  - `codex-helper serve --claude`（Claude，默认端口 3210）

### 开关 Codex / Claude

- 一次性让 Codex / Claude 指向本地代理：

  ```bash
  codex-helper switch on          # Codex
  codex-helper switch on --claude # Claude（实验）
  ```

- 从备份恢复原始配置：

  ```bash
  codex-helper switch off
  codex-helper switch off --claude
  ```

- 查看当前开关状态：

  ```bash
  codex-helper switch status
  codex-helper switch status --codex
  codex-helper switch status --claude
  ```

### 配置管理（上游 / 中转）

- 列出配置（默认 Codex，可显式指定 Claude）：

  ```bash
  codex-helper config list
  codex-helper config list --claude
  ```

- 添加新配置：

  ```bash
  # Codex
  codex-helper config add openai-main \
    --base-url https://api.openai.com/v1 \
    --auth-token sk-openai-xxx \
    --alias "OpenAI 主额度"

  # Claude（实验）
  codex-helper config add claude-main \
    --base-url https://api.anthropic.com/v1 \
    --auth-token sk-claude-yyy \
    --alias "Claude 主额度" \
    --claude
  ```

- 切换当前 active 配置：

  ```bash
  codex-helper config set-active openai-main
  codex-helper config set-active claude-main --claude
  ```

### 会话、用量与诊断

- 会话助手（Codex）：

  ```bash
  codex-helper session list
  codex-helper session last
  ```

- 请求用量 / 日志：

  ```bash
  codex-helper usage summary
  codex-helper usage tail --limit 20 --raw
  ```

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
codex-helper config set-active packy-main    # 使用 Packy

# 3. 一次性让 Codex 使用本地代理（只需执行一次）
codex-helper switch on

# 4. 在当前 active 配置下启动代理
codex-helper
```

### 场景 2：按项目快速恢复 Codex 会话

```bash
cd ~/code/my-app

codex-helper session list   # 列出与当前项目相关的最近会话
codex-helper session last   # 给出最近一次会话 + 对应 resume 命令
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

- 主配置：`~/.codex-helper/config.json`
- 请求过滤：`~/.codex-helper/filter.json`
- 用量提供商：`~/.codex-helper/usage_providers.json`
- 请求日志：`~/.codex-helper/logs/requests.jsonl`

Codex 官方文件：

- `~/.codex/auth.json`：由 `codex login` 维护，codex-helper 只读取，不写入；
- `~/.codex/config.toml`：由 Codex CLI 维护，codex-helper 仅在 `switch on/off` 时有限修改。

### `config.json` 简要结构

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

关键点：

- `active`：当前生效的配置名；
- `configs`：按名称索引的配置集合；
- 每个 `upstream` 表示一个上游 endpoint，顺序 = 优先级（primary → backup...）。

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
    }
  ]
}
```

行为简述：

- upstream 的 `base_url` host 匹配 `domains` 中任一项，即视为该 provider 的管理对象；
- 请求结束后，codex-helper 按需调用 `endpoint` 查询额度，解析 `monthly_budget_usd` / `monthly_spent_usd`；
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
  - `service`（codex/claude）、`method`、`path`、`status_code`、`duration_ms`；
  - `config_name`、`upstream_base_url`；
  - `usage`（input/output/total_tokens 等）。

这些字段是稳定契约，后续版本只会在此基础上追加字段，不会删除或改名，方便脚本长期依赖。

---

## 与 cli_proxy / cc-switch 的关系

- [cli_proxy](https://github.com/guojinpeng/cli_proxy)：多服务守护进程 + Web UI，看板 + 管理功能很全面；
- [cc-switch](https://github.com/farion1231/cc-switch)：桌面 GUI 级供应商 / MCP 管理器，主打“一处管理、按需应用到各客户端”。

codex-helper 借鉴了它们的设计思路，但定位更轻量：

- 专注 Codex CLI（附带实验性的 Claude 支持）；
- 单一二进制，无守护进程、无 Web UI；
- 更适合作为你日常使用的“命令行小助手”，或者集成进你自己的脚本 / 工具链中。

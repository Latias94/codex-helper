# codex-helper

Codex CLI 的本地中转代理与控制台。

它把 Codex 请求先送到本机代理，再按你配置的 provider / routing 转发到 OpenAI 官方或各类中转站。这样你可以在不中断 Codex 使用体验的情况下集中管理多个中转、多个 key、余额/套餐、请求日志、成本估算和 fallback 策略。

当前发布版本：`v0.15.0`

English: [README_EN.md](README_EN.md)

![内置 TUI 面板](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## 支持项目开发

如果 codex-helper 对你有帮助，可以通过我当前自用的 Codex 包月服务支持项目持续开发：

- AI.INPUT.IM 官网：https://ai.input.im
- AI.INPUT.IM 充值商城：https://shop.input.im
- 我的推广链接：https://shop.input.im/?code=4394517f

可用折扣码：

- Air 套餐八折：`NEWAIR`
- Max 套餐七折：`HELLOMAX`

## 适合谁

- 你有多个 Codex/OpenAI 兼容中转站，不想反复手改 `~/.codex/config.toml`。
- 你希望“包月中转优先，用完或失败后再兜底到备用线路”。
- 你想在 TUI/GUI 里看到当前 provider、余额/套餐、请求 token、cache token、耗时、重试和成本估算。
- 你需要长期运行的本地代理，并希望日志、状态、session 绑定和 dashboard 刷新保持可控。
- 你想快速查看和恢复本机 Codex 会话。

不适合的场景：你只使用一个官方账号、完全不需要切换 provider，也不关心请求可观测性。

## 核心能力

- **本地代理**：默认监听 `127.0.0.1:3211`，Codex 继续按原方式使用。
- **安全 Codex 局部修改**：只改本地代理片段，不影响 Codex 运行中写入的其他配置。
- **provider / routing 配置**：`version = 5` route graph 格式，新增 provider 后用 routing entry/routes 决定顺序、固定、分组或标签优先。
- **会话粘性与自动兜底**：同一 Codex 会话会尽量粘住已选 provider，请求失败、上游不可用或可信余额显示耗尽时再按策略切换候选 provider/upstream。
- **余额/套餐**：支持 Sub2API、New API 和常见 `/user/balance` 探测；失败不计为耗尽。
- **出站代理兼容**：本地代理和出站网络代理是两层概念；当前出站请求受系统/环境代理变量影响，还没有 `config.toml` 专用代理段。
- **请求可观测**：记录 provider、model、token、cache token、缓存命中率、TTFB、总耗时、输出速度、重试链和估算成本。
- **TUI/GUI**：TUI 内置在命令行里；GUI 可作为本地控制台或 attached 控制台使用。

## 快速开始

### 安装

```bash
cargo install cargo-binstall
cargo binstall codex-helper
```

安装后会得到两个命令：`codex-helper` 和短别名 `ch`。

从源码构建：

```bash
cargo build --release
```

### 启动

```bash
codex-helper
# 或
ch
```

默认行为：

- 启动本地代理；
- 初始化或迁移 `~/.codex-helper/config.toml`，旧文件会自动备份为 `.bak`；
- 必要时把 Codex 的 `model_provider` 局部 patch 到 `codex_proxy`；
- 交互终端中打开 TUI；
- 退出时撤销 codex-helper 的本地代理 patch。

只启动代理、不打开 TUI：

```bash
codex-helper serve --no-tui
```

显式开关 Codex 代理 patch：

```bash
codex-helper switch on
codex-helper switch status
codex-helper switch off
```

## 最小配置

最推荐用 CLI 生成和修改配置：

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

对应的 `~/.codex-helper/config.toml` 很薄：

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

常见 routing 策略：

| 目标 | 配置方式 | 说明 |
| --- | --- | --- |
| 固定一个 provider | `codex-helper routing pin input` | 临时强制走某个 provider |
| 按顺序兜底 | `codex-helper routing order input openai` | 最直观，适合大多数用户 |
| 包月优先 | `codex-helper routing prefer-tag --tag billing=monthly --order input,openai --on-exhausted continue` | 包月都已知耗尽后继续兜底 |
| 包月止损 | 同上但 `--on-exhausted stop` | 不自动切到按量 provider |
| 月包池 + paygo 兜底 | 在 TOML 中用嵌套 route nodes | `monthly_pool -> paygo` 保留清晰分组 |

新用户建议先看 [中文配置指南](docs/CONFIGURATION.zh.md)，里面按使用场景给了可复制模板。需要完整字段、迁移细节和高级说明时，再查 [English configuration reference](docs/CONFIGURATION.md)。

## 代理说明

codex-helper 有两层“代理”：

- **本地代理**：Codex 连接 `127.0.0.1:3211`，请求先进入 codex-helper，再由 routing 选择 provider。只要启用了 codex-helper 的 Codex patch，即使没有配置外部网络代理，请求也会经过这个本地 proxy server。
- **出站网络代理**：codex-helper 访问 provider、relay 或 balance API 时是否经过网络代理。当前版本还没有 `config.toml` 专用配置段，但底层 HTTP client 会受 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY` 等系统/环境变量影响。

更详细的边界和未来配置方向见 [配置指南的代理支持章节](docs/CONFIGURATION.zh.md#代理支持)。

## 常用命令

```bash
# provider / routing
codex-helper provider list
codex-helper provider show input
codex-helper provider disable input
codex-helper provider enable input
codex-helper routing show
codex-helper routing explain
codex-helper routing explain --model gpt-5 --json

# 会话
codex-helper session list
codex-helper session recent
codex-helper session last
codex-helper session transcript <SESSION_ID> --tail 40

# 请求日志与统计
codex-helper usage summary
codex-helper usage tail --limit 20
codex-helper usage find --errors --limit 10

# 价格
codex-helper pricing list
codex-helper pricing sync-basellm --model gpt-5 --dry-run

# 诊断
codex-helper status
codex-helper doctor
codex-helper --version
```

## UI 入口

### TUI

`codex-helper` 默认在交互终端打开 TUI。

常用页面：

- `Overview`：代理状态、当前会话和最近请求。
- `Routing` / `Stations`：route graph、provider 顺序、余额/套餐、tags、健康状态和 routing 预览。
- `Sessions`：session identity、effective route、route affinity、单会话覆盖。
- `Stats` / `Requests`：token、cache token、缓存命中率、耗时、重试、成本和请求日志。

常用快捷键会显示在底部。TUI 的持久化 provider/routing 编辑优先使用 routing 页面，手动改配置后可用 `R` 重新加载运行态配置。

### GUI

如果构建启用了 GUI feature，可以运行：

```bash
codex-helper-gui
# 或源码运行：
cargo run --release --features gui --bin codex-helper-gui
```

GUI 可以启动/附着本地代理，编辑常见单 endpoint provider、route node 和 routing，查看请求、余额、价格目录、session、health、breaker 和控制面板状态。复杂多 endpoint provider、模型映射和高级字段仍建议用 CLI 或 raw TOML。

## 配置文件位置

- 主配置：`~/.codex-helper/config.toml`
- 余额适配：`~/.codex-helper/usage_providers.json`
- 价格覆盖：`~/.codex-helper/pricing_overrides.toml`
- 请求过滤：`~/.codex-helper/filter.json`
- 请求日志：`~/.codex-helper/logs/requests.jsonl`
- GUI 配置：`~/.codex-helper/gui.toml`

Codex 自己的文件仍由 Codex 维护：

- `~/.codex/auth.json`
- `~/.codex/config.toml`

codex-helper 只会局部修改 `~/.codex/config.toml` 里的本地代理片段。

## 设计边界

codex-helper 刻意避免这些做法：

- 每个 provider 复制一份完整 Codex 配置。
- 根据 provider 名字猜测包月/按量。
- 在没有可靠测量前做“智能速度优先”或“成本优先”幻觉策略。
- 把余额查询失败当作 provider 不可用或已耗尽。
- 让 UI 保存复杂 provider 时悄悄丢掉高级字段。

## 更多文档

- [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md)：中文配置指南，包含常用 routing 模板和代理说明。
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md)：English configuration reference, routing, balance adapters, pricing, migration.
- [CHANGELOG.md](CHANGELOG.md)：版本变更和升级注意事项。
- [docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md](docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md)：与 cc-switch、aio-coding-hub、all-api-hub 的差距分析。
- [docs/workstreams/codex-control-plane-refactor/README.md](docs/workstreams/codex-control-plane-refactor/README.md)：控制平面设计记录。

## 参考项目

codex-helper 借鉴了这些项目的成熟设计，但定位更聚焦于 Codex CLI 本地中转与控制平面：

- [cc-switch](https://github.com/farion1231/cc-switch)：provider 管理、余额/套餐查询模板、请求用量展示。
- [aio-coding-hub](https://github.com/dyndynjyxa/aio-coding-hub)：多 CLI 网关、请求链路、成本统计和 provider 可观测性。
- [all-api-hub](https://github.com/qixing-jk/all-api-hub)：Sub2API / New API 余额、用量和账号适配经验。

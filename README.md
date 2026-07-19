# codex-helper

Codex CLI 的本地中转代理与控制台，重点解决多中转站路由、请求生命周期和可观测性问题。

很多 Codex 能力不是简单转发 `/responses` 就会稳定出现。Provider adapter、`/models` metadata、`/responses/compact`、WebSocket 和 hosted `image_generation` 都属于选中 provider 的契约；一些 sub2api 或其它 relay 在这些细节上返回的形态也不完全符合 Codex 预期。

codex-helper 把这些差异收在本地：Codex 连接本机代理，helper 再按 provider / routing 选择 OpenAI 官方或你的中转站，并提供模型列表翻译、provider-owned 能力诊断、余额观测和 fallback 策略。

当前发布版本：`v0.20.2`

English: [README_EN.md](README_EN.md)

![内置 TUI 面板](https://raw.githubusercontent.com/Latias94/codex-helper/main/screenshots/main.png)

## 支持项目开发

如果 codex-helper 对你有帮助，可以通过我当前自用的 Codex 包月服务支持项目持续开发：

- AI.INPUT.IM 官网：https://ai.input.im
- AI.INPUT.IM 充值商城：https://shop.input.im
- 我的推广链接：https://shop.input.im/?code=4394517f

可用折扣码：

- Lite 订阅套餐 9 折优惠码：`Lite9`
- Air 订阅套餐 8 折优惠码：`Air8`

## 适合谁

- 你有多个 Codex/OpenAI 兼容中转站，不想反复手改 `~/.codex/config.toml`。
- 你希望“包月中转优先，用完或失败后再兜底到备用线路”。
- 你需要一个显式、可恢复的本地代理开关，并希望需要 `auth.json` facade 的 Codex 功能也能由同一 journal 安全恢复，而不是依赖手工 hack。
- 你的 sub2api 或其它中转普通对话能跑，但 `/models`、`/responses/compact`、hosted `image_generation`、模型名映射这类 Codex 细节不稳定。
- 你想在 TUI 或桌面端里看到当前 provider、余额/套餐、请求 token、cache token、耗时、重试和成本估算。
- 你需要长期运行的本地代理，并希望日志、状态、session 绑定和 dashboard 刷新保持可控。
- 你想快速查看和恢复本机 Codex 会话。

不适合的场景：你只使用一个官方账号、完全不需要切换 provider，也不关心请求可观测性。

## 核心能力

- **本地代理**：默认监听 `127.0.0.1:3211`，Codex 继续按原方式使用。
- **完整 Codex client patch**：`[codex.client_patch]` 和 `switch on --preset ...` 可声明 provider identity、remote compaction、Responses WebSocket、`/models` 翻译和 hosted image generation。需要 auth facade 的 preset 会把原始 `auth.json` 精确备份到受保护的 helper state，并由 CAS/journal 恢复；外部编辑冲突会进入 `recovery_required`。模型缓存和 SQLite 始终不属于 helper 所有权。
- **Provider-owned 能力契约**：Responses、compact、WebSocket、hosted tool 和模型能力来自捕获的 provider/catalog 事实，不由客户端 patch 假设。
- **OpenAI Images 兼容入口**：本地代理额外暴露 `POST /v1/images/generations` 和 JSON `POST /v1/images/edits`，会转成 Responses hosted `image_generation` 请求并复用同一套 provider routing / fallback，方便本地 skill 或脚本稳定生图或带参考图生成。
- **中转能力诊断**：显式、本进程的 CLI 动作可以有界检查 `/models`、`/responses`、`/responses/compact`，展示 provider contract、观测、continuity 和 mismatch，但不会修改配置或路由。
- **provider / routing 配置**：`version = 6` route graph 格式，新增 provider 后用 routing entry/routes 决定顺序、固定、分组或标签优先。
- **会话粘性与自动兜底**：同一 Codex 会话会尽量粘住已选 provider，请求失败、上游不可用或可信余额显示耗尽时再按策略切换候选 provider/upstream。
- **provider 信号控制循环**：限流、配额、传输错误和余额耗尽会先记录为 provider signal，再生成 helper 拥有的临时 policy action 投影到路由；手动禁用优先级更高，自动 action 不会修改 Codex auth 或第三方账号文件。
- **本地并发上限**：可为 provider 或 endpoint 配置本进程并发上限，relay 账号饱和时自动跳过并走 fallback。
- **余额/套餐**：支持 Sub2API、New API 和常见 `/user/balance` 探测；失败不计为耗尽。
- **出站代理兼容**：本地代理和出站网络代理是两层概念；当前出站请求受系统/环境代理变量影响，还没有 `config.toml` 专用代理段。
- **请求可观测**：记录 provider、model、token、cache token、缓存命中率、TTFB、总耗时、输出速度、重试链、provider signal / policy action 证据和估算成本。
- **TUI / Desktop**：TUI 内置在命令行里；旧 `codex-helper-gui`/egui 入口已移除。Tauri 桌面端代码位于 `apps/desktop`，已完成 Windows packaged smoke，但当前公开 release 仍不发布桌面安装包，等签名、发布通道和回滚流程就绪后再进入正式桌面发布。

## 快速开始

### 安装

推荐使用预编译安装脚本（无需本机安装 Rust）：

macOS / Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Latias94/codex-helper/releases/download/v0.20.2/codex-helper-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Latias94/codex-helper/releases/download/v0.20.2/codex-helper-installer.ps1 | iex"
```

安装后会得到两个命令：`codex-helper` 和短别名 `ch`。旧 egui GUI 入口 `codex-helper-gui` 已移除；Tauri 桌面端当前仍是源码内预览路径，公开 release 暂不发布桌面安装包；需要本地验证时可从 `apps/desktop` 运行 `pnpm tauri:build`。

如果不想 pipe shell，可以到 [GitHub Releases](https://github.com/Latias94/codex-helper/releases) 下载对应平台压缩包，并使用同名 `.sha256` 文件校验。

Rust 用户也可以使用 `cargo-binstall`：

```bash
cargo install cargo-binstall
cargo binstall codex-helper
```

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
- 读取唯一支持的 `version = 6` `~/.codex-helper/config.toml`；已有 v5 配置会先备份再自动迁移；
- 交互终端中打开 TUI；
- 退出时停止当前前台控制台启动的代理。

自动配置迁移只会把受支持的旧语法转换为 version 6，并保留源文件的精确备份；它不会复制、删除或重新解释任何凭据值。把值导入 OS 凭据存储是另一项必须显式执行的操作，例如 `codex-helper credential import relay.primary --from-env RELAY_TOKEN`。

只启动代理、不打开 TUI：

```bash
codex-helper serve --no-tui
```

高级：后台服务/附着代理（只有显式安装服务或使用 `--resident`/`daemon`/`tui` 子命令时，代理才会独立于当前控制台继续运行）：

```bash
codex-helper service install --codex
codex-helper service status
codex-helper daemon status
codex-helper tui --codex
codex-helper service stop
```

默认 `codex-helper serve` 的内置 TUI 遵循“界面拥有代理”：退出界面会停止它自己启动的代理，但不会执行 `switch on/off`。`daemon status` 只读查询 resident proxy；已安装的本地服务使用 `service start/stop/restart` 管理，不提供远程 HTTP shutdown 命令。`tui` 子命令附着到已有 resident proxy：在 daemon 所在机器，本机签名 operator capability 可执行 daemon 明确声明的余额刷新、路由和会话操作；本机签名不可用时会降级为只读。`RemoteObserver` 绝对只读，不发送 operator mutation。退出 attached TUI 不会停止代理。需要自动拉起/崩溃重启时可用 `codex-helper daemon supervise --codex`，supervisor 会写入轻量 crash marker 到 `~/.codex-helper/run/` 便于排查。

`daemon status` 会尽量显示当前 resident proxy 的 owner marker（manual CLI、supervisor 或未来桌面/托盘 owner）；marker 只用于可观测性，读取或清理失败不会阻断代理启动/退出。面向未来桌面端的 sidecar 语义已经预留为隐藏的 managed 启动模式，普通用户无需手动判断或使用。

Tauri 桌面端采用更接近 Clash 的常驻客户端语义：关闭主窗口隐藏到托盘，`Quit App` 只退出桌面进程，两者都不会停止 runtime。停止 runtime 属于显式的本地 CLI/service 操作，不在桌面端 query-only 控制面内。Windows NSIS packaged 路径已通过隔离生命周期 smoke，但尚未进入公开发布；macOS/Linux packaged parity、签名发布链路和回滚流程仍需单独完成。

显式切换 Codex 客户端到 helper：

```bash
codex-helper switch on
codex-helper switch on --port 4321
codex-helper switch on --base-url https://relay.example/v1
codex-helper switch on --preset imagegen-bridge
codex-helper switch on --preset official-imagegen --compaction remote-v2 --responses-websocket
codex-helper switch on --client-facade openai-tools
codex-helper switch status
codex-helper switch off
```

NAS / 远端 relay target：

```bash
ch relay add nas \
  --proxy-url http://nas.local:3211 \
  --admin-url https://nas.example.com:4211 \
  --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN

ch relay list
ch relay status nas
ch relay nas
ch relay local --no-tui
ch relay nas --attach-only
ch relay off
```

`ch` 仍然是本机前台启动入口；`ch relay local` 是同一行为的显式 target 写法。`ch relay <name>` 只启动或附着到目标 runtime，并打开只读 TUI，不会修改本机 Codex 配置；`--no-tui` 只适用于启动内置本地 target，`--attach-only` 要求 runtime 已运行。远端 target 始终通过只读 TUI 附着。要让 Codex 指向该目标，另行执行 `codex-helper switch on --base-url <PROXY_URL>`。admin token 只从 `--admin-token-env` 指定的环境变量读取，值不会写进 `~/.codex-helper/config.toml`。远程 admin URL 必须使用 HTTPS；HTTP 只允许 loopback，包括在客户端终止的可信隧道。远端 target 必须在 `relay add` 时显式提供可信的 `--admin-url`；proxy 响应和重定向不会替换该 authority。

容器和服务器不提供客户端本地 transcript/session 能力。本地 `session` 命令只读取执行该命令机器上的 Codex 会话文件。

客户端 switch 负责把 Codex 指向一个 helper URL，并应用完整的 `[codex.client_patch]`。可用 `--preset default|chatgpt-bridge|imagegen-bridge|official-relay|official-imagegen` 临时覆盖 preset，并用 `--compaction`、`--responses-websocket[=false]`、`--translate-models[=false]` 和 `--hosted-image-generation` 覆盖其余字段；不带覆盖时读取 `~/.codex-helper/config.toml` 的全部五项配置。`--client-facade compatible|openai|openai-tools` 仍作为旧精简接口的兼容快捷方式。Client patch 只决定 Codex 暴露和发送哪些能力，不能保证选中的 relay 真正支持协议；运行时仍以 provider/catalog 契约为准。

`switch on` 同时记录原 Codex selector、helper stanza、相关 feature flags，以及需要时的 auth facade。`chatgpt-bridge` 只接受完整且可验证的现有 ChatGPT 登录，并保留 token；imagegen preset 可临时呈现语义上的空 `{}` auth facade，以满足 Codex 对 hosted tool 的客户端 gating。原始 auth 字节不写入 JSON journal，而是保存在 helper 私有 state backup；journal 只记录随机备份名和指纹。`switch off` 通过 no-replace CAS 精确恢复。认证型 patch 关闭后仍显示 `Off`，但会保留私有 backup/journal，以便 Codex 稍后重新写入语义相同的 facade 时再次执行 `switch off` 自动修复；下一次 `switch on` 会安全接管同一恢复点。无法归因的外部编辑、备份缺失或指纹不一致会进入 `recovery_required`，且不会覆盖竞争写入。helper capability marker 不会上送，真实 actor authorization 只可能透传到未配置 helper 凭据的 OpenAI 官方源站。

Client patch 不读取或修改 `models_cache.json` 和 Codex SQLite，也不恢复旧 `remote-control` SQL hack。它只在显式 `switch on/off` 生命周期内管理已经记录的 `config.toml` 片段和可选 `auth.json` facade。

从 0.20.3 或更早版本升级时，如果 `~/.codex/codex-helper-switch-state.json` 仍存在，新版 `switch off` 会安全自动恢复旧 helper 管理过的 selector/provider stanza 和可验证的 auth facade；`switch on` 会先完成同样的恢复，再创建新 journal。恢复只在当前文件仍匹配旧 helper patch 时写入；损坏、未知版本或新旧 journal 冲突都会保留原 state 并失败关闭。旧 state 可能包含原始 auth，不要删除、编辑或分享。旧 `switch remote-control enable` 写入的 `remote_connections` 和 Codex SQLite 状态不会被新版自动撤销，也不要用 SQL hack 清理；完整顺序、v5 到 v6 迁移和退休字段见[中文配置兼容性说明](docs/CONFIGURATION.zh.md#配置兼容性)。

Relay 能力由选中 provider 的 adapter、catalog 和有界观测决定，不由 switch 配置推断。可用下面的本地命令查看 provider contract、实际 `/models` / `/responses` / `/responses/compact` 结果、continuity 和 mismatches：

```bash
codex-helper codex relay-capabilities --model gpt-5.5 --provider ciii --endpoint default
```

第三方 relay 应显式配置 helper 侧凭据。v6 可以把 bearer 或 `X-API-Key` 绑定到原生凭据、绝对路径的只读 secret file、环境变量或兼容期 inline 值；选择 `auth_token_ref` / `api_key_ref` 后不会再回退同类 legacy 来源。本机已安装 service 优先使用当前登录用户的 Credential Manager、Keychain 或 Secret Service，Docker/headless server 使用环境变量或 mounted secret；server 不支持 native reference，也不会创建明文或 SQLite fallback。完整命令、解析顺序和 readiness 语义见[凭据与 service 说明](docs/CONFIGURATION.zh.md#provider-字段)。Codex 客户端认证只允许透传给官方 OpenAI origin，避免把账号 header 泄露给中转。

为了不拖能力较强的中转后腿，codex-helper 默认会在路由前归一化压缩 HTTP 请求体（`zstd`、`gzip` / `x-gzip`、`br`、`deflate`）。对 Codex `/responses`、`/responses/compact` 和 Responses WebSocket，helper 还会从已有请求证据补齐缺失的 `session_id`、`x-session-id`、官方 `session-id` / `thread-id` 和 `prompt_cache_key`，来源包括 header session、body `session_id`、`prompt_cache_key` 和 `metadata.session_id`。`previous_response_id` 只用于 stale-response 修复，不作为 session identity 来源；helper 不会凭空生成 session id，也不会覆盖用户已经带上的 session 字段。

已选 provider endpoint 的会话 affinity 会持久化到 helper state，所以 helper 重启后不会让 Codex remote compaction 会话静默换到另一个 provider endpoint。带有 `encrypted_content`、`previous_response_id` 或 `compaction_summary` 的 v1 compact，以及带 `compaction_trigger` 的 remote compaction v2，都会按 state-bound compact 处理：多 endpoint graph 使用 `hard` 时，没有可证明 affinity 会返回明确的连续性错误；默认 `fallback-sticky` 则沿当前 route graph 尝试并由上游判断状态是否有效，成功后记录新 affinity。这个判断保持 provider-opaque：helper 不推断 relay 背后是 OpenAI、sub2api、New API 还是其它中转。这样 relay 粘性可以贯穿 `/responses`、`/responses/compact` 和 v2 compact 的 `/responses` 请求；但这不会替上游补出 compact 或 WebSocket 能力。极少数中转如果必须接收原始 Codex 压缩 body，可用 `CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough` 启动 helper。

Codex 请求语义还有两个小修复：如果上游明确返回 `previous_response_id` 对应 response 不存在，helper 会移除该字段并对同一个上游重试一次；如果中转无视 `Accept-Encoding: identity` 返回 gzip JSON，helper 会先解压再转发普通 JSON。`service_tier` 只做观测和日志归因，日志会区分 requested / effective / actual，不会因为 helper 默认配置改写客户端请求里的 fast mode。

Hosted image generation、remote compaction 和 Responses WebSocket 都要求上游真正支持对应协议。Live smoke 必须显式确认，且只用于诊断，不会开启客户端功能或修改路由。

本地代理还提供 OpenAI Images 兼容的生图和参考图编辑入口，适合给 Codex skill 或脚本调用，而不是依赖 Codex 客户端是否成功暴露 hosted tool：

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

参考图模式使用 JSON `POST /v1/images/edits`，接受 `images` 数组，数组元素可以是 `{"image_url":"..."}`、`{"file_id":"..."}`，也可以直接写图片 URL / data URL 字符串；helper 会把这些引用转成 Responses `input_image` 内容：

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

这两个入口内部仍走 `/v1/responses` + hosted `image_generation`，因此真实上游必须支持该能力；当前只支持 `n=1` 的单图结果。JSON edits 不实现 mask 解析，带 `mask` 的 JSON 和 multipart edits 会按普通代理请求直通上游。返回形态为 OpenAI Images 风格的 `data[0].b64_json`。

注意：任何对 `~/.codex/config.toml` 的修改都只会被新启动的 Codex 会话读取；修改后请完整重启 Codex App、TUI 或 `codex exec` 会话。

要把模型流量交给 relay，请把客户端指向 helper，再由 helper 配置选择上游：

1. 运行 `codex-helper switch on`，把 Codex 的 `~/.codex/config.toml` 指向本地 `codex_proxy`。
2. 在 `~/.codex-helper/config.toml` 配置 `codex.providers.*` 和 `codex.routing`。
3. 如果 relay 需要带前缀的模型名，给 provider 配置 `model_mapping`。

这条链路不代理 Codex 登录。只有选择需要 auth facade 的 client patch 时，helper 才在显式 switch 生命周期内临时改写 `auth.json` 的客户端视图，并在关闭时按原始字节恢复；登录凭据本身仍由 Codex 创建和维护。

Codex 侧的本地代理入口通常由 `switch on` 写入，不建议手写覆盖其它 Codex 配置：

```toml
# ~/.codex/config.toml
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
```

codex-helper 侧只负责上游和路由：

```toml
# ~/.codex-helper/config.toml
version = 6

[codex.providers.relay]
base_url = "https://relay.example/v1"
auth_token_env = "RELAY_API_KEY"

[codex.routing]
entry = "relay_first"

[codex.routing.routes.relay_first]
strategy = "ordered-failover"
children = ["relay"]
```

如果中转站要求带 provider 前缀的模型名，可以用 provider 级 `model_mapping` 改写请求体里的 `model`：

```bash
codex-helper provider add relay --base-url https://relay.example/v1 --auth-token-env RELAY_API_KEY --supported-model gpt-5.5 --model-map gpt-5.5=openai/gpt-5.5
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
version = 6

[codex.providers.input]
base_url = "https://ai.input.im/v1"
auth_token_env = "INPUT_API_KEY"
tags = { billing = "monthly" }

[codex.providers.input.limits]
max_concurrent_requests = 5
limit_group = "input-account"

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

[中文配置参考](docs/CONFIGURATION.zh.md) 和 [English configuration reference](docs/CONFIGURATION.md) 内容对齐，任选一种语言阅读即可；常用 route graph 模板在配置文档的“配置模板 / Recipes”章节。

## 代理说明

codex-helper 有两层“代理”：

- **本地代理**：Codex 连接 `127.0.0.1:3211`，请求先进入 codex-helper，再由 routing 选择 provider。显式执行 `switch on` 让 Codex 指向 helper 后，即使没有配置出站网络代理，请求也会经过这个本地 proxy server。
- **出站网络代理**：codex-helper 访问 provider、relay 或 balance API 时是否经过网络代理。当前版本还没有 `config.toml` 专用配置段，但底层 HTTP client 会受 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY` 等系统/环境变量影响。

更详细的边界和未来配置方向见 [配置参考的本地代理和出站代理章节](docs/CONFIGURATION.zh.md#本地代理和出站代理)。

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
codex-helper session list --truncate 120
codex-helper session search "rate limit"
codex-helper session search "rate limit" --truncate 120
codex-helper session recent
codex-helper session last
codex-helper session transcript <SESSION_ID> --tail 40

# 请求日志与统计
codex-helper usage quota --target local
codex-helper usage quota --target local --json
codex-helper usage summary
codex-helper usage tail --limit 20
codex-helper usage find --errors --limit 10
codex-helper usage chain --trace-id <TRACE_ID> --json

# 价格
codex-helper pricing list
codex-helper pricing status
codex-helper pricing force-refresh
codex-helper pricing import-basellm --model gpt-5 --dry-run

# 诊断
codex-helper status
codex-helper doctor
codex-helper codex relay-capabilities --model gpt-5.5 --provider ciii --endpoint default
codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5
codex-helper codex relay-live-smoke --acknowledgement run-live-codex-relay-smoke --model gpt-5.5 --provider ciii --compact-v2
codex-helper codex relay-evidence --limit 20
codex-helper --version
```

## UI 入口

### TUI

`codex-helper` 默认在交互终端打开 TUI。

常用页面：

- `Overview`：代理状态、当前会话和最近请求。
- `Routing`：provider/endpoint 顺序、configured/effective/routable 状态、自动控制、capacity 和紧凑余额/配额；完整 route graph 与候选路径请使用 `routing show` / `routing explain` 查看。
- `Sessions`：session identity、effective route、route affinity、单会话覆盖。
- `Usage`：远端共享 quota pool 的 used/remaining、15/60 分钟速率、reset 前所需速率、pace、ETA，以及本地今日请求、token、估算成本和 project 归因。
- `Requests`：已提交的 request/attempt 事实、endpoint 最近样本、token、cache token、耗时、重试、request chain 和成本。

TUI 和桌面端消费同一份 typed、redacted `OperatorReadModel`，对远程 runtime control plane 只使用 `GET` / `HEAD`。模型明确区分 `ready`、`stale`、`disconnected` 和 `auth_required`；连接或认证失败时不会用本机配置、SQLite 或空 runtime 伪造 fallback view。远程 operator clients 与 control plane 都是只读的；attached TUI 不处理 `n` / `o`，也不会检查或修改本机 Codex 配置。持久 provider/routing intent 通过本地 CLI 或 `config.toml` 修改。终端场景需要切换 Codex 客户端时，只能另行显式执行本地 `switch on/off` CLI，或在 integrated local TUI 的 Settings 页面使用 `n` / `o`；两者都不是远程 control-plane 操作。

远端 quota sampler 由目标 daemon 独占，附着客户端不会启动第二个 sampler，也不会通过远端 control plane 强制刷新或修改运行态。远端 pool counter 可能包含使用同一账号或 key 的其他电脑，是共享总消耗的事实源；本机 project 归因来自 daemon 已提交到 `state.sqlite` 的 request ledger，绝不会按远端差额放大本地请求价格。更完整的 source/scope/confidence、coverage、raw unit 和 conversion-generation 限制见 [中文配置参考](docs/CONFIGURATION.zh.md#usage-页面)。

### Desktop Preview

新的 Tauri 桌面端位于 `apps/desktop`，技术栈是 React 19、Tailwind CSS 4、shadcn/ui 风格组件和 TanStack Router/Query/Table。它展示 typed、redacted `OperatorReadModel`，并保留本地 proxy 生命周期、显式 Codex switch、关闭隐藏到托盘、单实例和开机启动设置；不导入配置、不编辑 provider，也不通过远程 control plane 修改 provider/routing/config。Windows NSIS packaged sidecar 已完成隔离 smoke，但当前公开 release 仍不发布桌面安装包，正式 release 会等签名密钥、HTTPS 发布端点、artifact hosting 和回滚流程就绪后再启用。桌面端打包策略见 [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md)。

## 配置文件位置

- 主配置：`~/.codex-helper/config.toml`
- 运行时状态：`~/.codex-helper/state/state.sqlite`
- 余额适配（可选、operator-owned；缺失时只使用内存内置项，无效输入不会被覆盖）：`~/.codex-helper/usage_providers.json`
- 价格覆盖：`~/.codex-helper/pricing_overrides.toml`
- 请求过滤：`~/.codex-helper/filter.json`
- 提交后的调试日志：`~/.codex-helper/logs/requests.jsonl`
- Codex relay 诊断证据：`~/.codex-helper/logs/codex_relay_evidence.jsonl`

Codex 文件仍以 Codex 为权威来源：

- `~/.codex/auth.json`
- `~/.codex/config.toml`

显式本地 `switch on/off` 会管理 `~/.codex/config.toml` 中记录过的 client patch，并可在需要时临时管理 `auth.json` facade；原始 auth 通过私有备份和 CAS 精确恢复。Codex 模型缓存和 SQLite 始终保持不动。

## 设计边界

codex-helper 刻意避免这些做法：

- 每个 provider 复制一份完整 Codex 配置。
- 根据 provider 名字猜测包月/按量。
- 在没有可靠测量前做“智能速度优先”或“成本优先”幻觉策略。
- 把余额查询失败当作 provider 不可用或已耗尽。
- 让 UI 保存复杂 provider 时悄悄丢掉高级字段。

## 更多文档

- [docs/CONFIGURATION.zh.md](docs/CONFIGURATION.zh.md)：中文完整配置参考，包含 routing 模板、余额适配、代理说明、配置兼容性和只读 operator 视图。
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md)：English configuration reference covering routing, balance adapters, pricing, configuration compatibility, and query-only operator views.
- [CHANGELOG.md](CHANGELOG.md)：版本变更和升级注意事项。
- [docs/DESKTOP_RELEASE.md](docs/DESKTOP_RELEASE.md)：Tauri 桌面端打包、sidecar 和 release gate 说明。
- [docs/workstreams/codex-routing-scheduler-observability-refactor/README.md](docs/workstreams/codex-routing-scheduler-observability-refactor/README.md)：路由调度状态、限流/过载结果、并发上限和 TUI 指标的无畏重构设计。
- [docs/workstreams/codex-tui-operator-polish/README.md](docs/workstreams/codex-tui-operator-polish/README.md)：TUI 用量、路由、窄终端和快捷键操作体验优化计划。
- [docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md](docs/workstreams/codex-operator-experience-refactor/GAP_MATRIX.md)：与 cc-switch、aio-coding-hub、all-api-hub 的差距分析。
- [docs/workstreams/codex-control-plane-refactor/README.md](docs/workstreams/codex-control-plane-refactor/README.md)：控制平面设计记录。

## 参考项目

codex-helper 借鉴了这些项目的成熟设计，但定位更聚焦于 Codex CLI 本地中转与控制平面：

- [cc-switch](https://github.com/farion1231/cc-switch)：provider 管理、余额/套餐查询模板、请求用量展示。
- [aio-coding-hub](https://github.com/dyndynjyxa/aio-coding-hub)：多 CLI 网关、请求链路、成本统计和 provider 可观测性。
- [all-api-hub](https://github.com/qixing-jk/all-api-hub)：Sub2API / New API 余额、用量和账号适配经验。

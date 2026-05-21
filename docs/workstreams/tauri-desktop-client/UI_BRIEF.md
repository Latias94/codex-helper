# Tauri Desktop Client — UI Brief

Status: Draft pre-imagegen brief
Last updated: 2026-05-21

## Decision Summary

The first Tauri client should look like a calm local relay control center, not a SaaS billing portal and not an observability cockpit.

Design north star:

- a developer opens the app to confirm the local proxy is running, Codex is connected, the active provider is healthy, and today's usage/cost is acceptable;
- the app should feel native enough for a desktop utility, but still use standard React + Tailwind + shadcn/ui primitives;
- advanced routing, sessions, diagnostics, traces, and raw TOML must remain available through progressive disclosure, not top-level navigation.

Recommended visual direction:

> A restrained mint/teal local dashboard: fixed sidebar, soft cool page wash, white cards, precise hairline borders, compact tables, clear status badges, and a serious distinction between Quit, Detach, and Stop Proxy.

## Reference Synthesis

| Reference | Borrow | Do not borrow |
| --- | --- | --- |
| User-provided AIO Coding Hub screenshot | Fixed desktop shell, left sidebar, bottom proxy status card, subtle grid background, page cards, dashboard split between runtime status and recent records. | Broad top-level navigation, blue-first accent, large generic charts that crowd the local-proxy story. |
| `repo-ref/aio-coding-hub` | App shell, page header actions, usage filters, provider card + active order pattern, settings main/sidebar layout, loading/error/stale state discipline. | Workspaces/prompts/MCP/skills/console/logs/CLI manager as first-version nav. |
| `awesome-design-md/supabase` | Emerald/green technical accent, clean product UI, near-monochrome surfaces with one intentional color event. | Marketing-page hero treatment or dark featured cards. |
| `awesome-design-md/mintlify` | Soft sky/mint wash, readable documentation-grade spacing, Inter + mono pairing, rounded pills. | Documentation sidebars/TOC and cloud illustration motifs. |
| `awesome-design-md/linear.app` | Typography precision, subtle borders, minimal color discipline, “software-craft” restraint. | Dark luxury canvas and lavender brand accent as the main direction. |
| `awesome-design-md/ollama` / `notion` | Local-first simplicity, friendly plain language, low-drama empty states. | Mascot/illustration-heavy or playful database-card styling. |

## Image References And Feedback

Use these project-local images as the current visual source of truth for imagegen continuation and later shadcn/ui prototyping:

- `docs/workstreams/tauri-desktop-client/assets/dashboard-approved-v1.png`
  - Accepted as the Dashboard baseline.
  - Keep the restrained mint/teal Tauri shell, the left navigation, the top runtime actions, the Codex-focused relay switch, the concise status cards, recent requests, provider health, and usage trend.
  - Do not reintroduce Claude/Anthropic rows on the Dashboard first screen. The Dashboard should focus on Codex relay state unless a later explicit requirement changes that.
- `docs/workstreams/tauri-desktop-client/assets/api-keys-approved-v1.png`
  - Superseded as a standalone page, but retained as the provider credential list density reference.
  - The same image is also copied to `docs/workstreams/tauri-desktop-client/assets/provider-credentials-list-reference-v1.png`.
  - Borrow its clean toolbar + table + calm security-note density for provider credentials inside the Providers page.
  - Do not use the earlier dense API key draft pattern with a large proxy entrance card, permanent selected-key side panel, metric cards, or repeated Codex switch/status controls.
- `docs/workstreams/tauri-desktop-client/assets/providers-approved-v1.png`
  - Accepted as the Providers page baseline.
  - Keep provider cards as the default view, show auth/env-var on each provider, and preserve the right-side default route order panel.
  - Do not reintroduce standalone API Keys navigation or marketplace-style provider browsing.
- `docs/workstreams/tauri-desktop-client/assets/usage-approved-v1.png`
  - Accepted as the Usage page baseline.
  - Keep the full-width request table, practical pagination footer, token direction/cache indicators, first-token latency column, and estimated-cost tooltip.
  - Do not add a permanent right-side cost panel; costs must remain framed as estimates.
- `docs/workstreams/tauri-desktop-client/assets/settings-generated-too-complex-v1.png`
  - Rejected as a Settings direction.
  - It is too console-like: too many horizontal rows, too much always-visible runtime/detail information, and a danger area that makes Settings feel like a dense control plane.
  - Keep it only as an anti-reference for over-complicated Settings density.
- `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-a.png`
  and `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-b.png`
  - Use these as Settings structure references only.
  - Borrow the simple desktop-utility feel: readable setting groups, compact controls, two-column adaptive cards, and low visual noise.
  - Do not copy their product scope, broad navigation, black/blue palette, or proxy-specific terminology.
- `docs/workstreams/tauri-desktop-client/assets/settings-candidate-v2.png`
  - Candidate Settings revision generated from the simplified two-column prompt and accepted by the user.
  - It is closer to the desired adaptive settings-card direction than the rejected `settings-generated-too-complex-v1.png`.
- `docs/workstreams/tauri-desktop-client/assets/settings-approved-v1.png`
  - Accepted as the Settings page baseline.
  - Keep the simple responsive two-column settings-card layout, no permanent right sidebar, collapsed/secondary advanced tools, and compact bottom dangerous actions with `Stop Proxy` visually stronger than `退出应用` and `Detach`.

## Primary User Questions

The dashboard must answer these in under five seconds:

1. Is the local proxy running, stopped, or merely attached?
2. Is Codex currently pointed at the local proxy?
3. Which provider/station will the next request use?
4. How much did I use today, and is the cost/balance safe?
5. Do I need to take action: refresh, switch on, add a provider, run diagnosis, or stop the proxy?

## Information Architecture

Top-level pages:

1. Dashboard
2. Providers
3. Usage
4. Settings

Non-top-level advanced entry points:

- Provider credentials and API keys: Providers page cards/list, provider detail drawer, or provider edit sheet.
- Sessions and per-session overrides: Settings > Advanced Tools, plus request/provider detail drawers.
- Routing graph: Providers > Advanced or Settings > Advanced Tools.
- Relay diagnostics: Dashboard quick action, Providers row action, Settings > Advanced Tools.
- Request trace/control trace: Usage row detail drawer.
- Raw TOML editor/log folder: Settings > Advanced Tools.

## Desktop Layout Specification

Target concept-board size:

- Primary: 1360 × 860 or 1440 × 900.
- Secondary compatibility: 1280 × 820.

Layout:

- Sidebar width: 232–256 px.
- Top drag-safe region: 28–36 px.
- Content padding: 24–32 px.
- Page max density: 2 major columns on Dashboard and Providers; avoid four-column card walls.
- Background: very light cool mint/blue wash with optional subtle grid at 4–8% opacity.
- Cards: white or near-white panels with 1 px cool border and soft shadow.
- Tables: compact but not cramped; 13–14 px body text; masked secrets in mono.

Sidebar:

- Brand block: codex-helper, local relay helper.
- Navigation: Dashboard, Providers, Usage, Settings.
- Utility: language switch, dark mode toggle, collapse control.
- Footer runtime card:
  - Local Proxy: Running / Stopped / Attached.
  - Port: `3211`.
  - Ownership copy: “Started by this app” or “Attached only”.
  - App version.

Top bar:

- Page title and concise subtitle.
- Runtime badge: Running / Attached / Stopped.
- Balance pill.
- Notification/diagnosis icon.
- Refresh action.

## Visual Tokens

Use these as the first imagegen/prototype token set. Exact Tailwind variables can be adjusted later.

| Token | Value | Role |
| --- | --- | --- |
| canvas | `#F5FBF8` | Main app wash. |
| canvas-alt | `#F3F8FC` | Cool blue secondary wash. |
| sidebar | `#F8FAFC` | Sidebar base. |
| panel | `#FFFFFF` | Card/table surface. |
| panel-soft | `#F8FFFC` | Highlighted soft card surface. |
| border | `#DCE7E3` | Default hairline. |
| border-strong | `#C9D8D3` | Strong dividers. |
| ink | `#0F172A` | Primary text. |
| ink-muted | `#64748B` | Secondary text. |
| ink-subtle | `#94A3B8` | Captions and timestamps. |
| primary | `#0F9F8F` | Main teal action/accent. |
| primary-hover | `#0B8277` | Hover/pressed action. |
| primary-soft | `#DDFBF4` | Active nav and selected chips. |
| success | `#10B981` | Healthy/running/success. |
| warning | `#F59E0B` | Stale/warning/diagnosis recommended. |
| danger | `#EF4444` | Stop proxy, errors, destructive actions. |
| info | `#2563EB` | Secondary informational metrics. |
| purple | `#7C3AED` | Rare secondary capability badge. |

Typography:

- UI font: Inter, system-ui, `Microsoft YaHei UI`, sans-serif.
- Mono font: JetBrains Mono, ui-monospace, SFMono-Regular, Consolas.
- H1: 26–30 px, 650 weight, tight line-height.
- H2/card title: 15–17 px, 600 weight.
- Body: 13–14 px.
- Caption: 12 px.
- Mono secrets/endpoints: 12–13 px.

Radius and elevation:

- App panels: 18–22 px radius.
- Cards/tables: 14–18 px radius.
- Buttons/inputs/chips: 8–12 px radius, pills only for badges and compact segmented controls.
- Shadow: subtle, never glassy; prefer `0 12px 30px rgba(15, 23, 42, 0.06)`.

## Page Requirements

### Dashboard

Goal: simple overview and next action.

Hero/status strip:

- Local Proxy card: Running, port, ownership mode.
- Codex Connection card: Connected / Not connected, preset.
- Active Provider card: provider/station, route type.
- Today card: requests, tokens, estimated cost.

Main panels:

- Work Status:
  - Codex row with switch status, active provider, primary action.
  - Safe actions: Run diagnosis, Switch On, Switch Off, Advanced connection settings.
  - Attached-mode note if relevant.
- Recent Requests:
  - latest 5 rows;
  - status, model, provider, input/output/cache tokens, cost, duration, time;
  - “View all usage”.
- Provider Health:
  - provider, balance/quota, health, latency, today's cost.
- Usage Trend:
  - one line/area chart for token or cost trend;
  - optional compact model/provider distribution only if it does not crowd the page.

Dashboard must avoid a full observability-wall feeling.

### Providers

Goal: provider/station health, credentials, capabilities, and default route management without exposing route graphs by default.

Provider model:

- A provider is the user-facing object.
- API key / token / env var is only the provider authentication field.
- Do not create a standalone API Keys top-level page for MVP.
- Provider edit/detail is where users add or change credentials.

Required elements:

- Search and tag chips.
- Provider cards as default view.
- Optional compact list/table mode for provider credentials, using `provider-credentials-list-reference-v1.png` as density reference.
- Right-side “Default Route” or “Active Order” panel.
- Card fields:
  - provider name;
  - host;
  - auth source: env var, masked key, or missing credential;
  - enabled/active/default badge;
  - balance/quota;
  - health;
  - latency;
  - capabilities: models, responses, compact, imagegen, websocket;
  - recent requests and cost.
- Actions:
  - Set Active
  - Probe
  - Refresh Balance
  - Edit
  - Disable / Enable
  - Advanced

Credential list/table mode:

- It is inside Providers, not a top-level page.
- Toolbar: search, status/provider capability filters, endpoint pill, Refresh, Import config, Add Provider.
- Columns: Provider, Host, Auth / Env Var, Usage today, Last Used, Status, Actions.
- One visible row action is enough: Probe or Enable. Put Copy, Edit, Disable, Delete in an overflow menu.
- Include the note: “建议优先使用环境变量保存 token，避免把真实密钥写入配置文件。”

Avoid:

- standalone API Keys navigation item;
- large local proxy entrance card;
- permanent selected-key side panel;
- charts or metric cards on the credentials list;
- repeated Codex relay switch;
- subscription, user account, marketplace, or SaaS billing metaphors.

Advanced panel:

- model mapping summary;
- station/upstream list;
- route settings link;
- diagnostics link.

### Usage

Goal: request history and cost visibility.

Required elements:

- Summary cards: total requests, total tokens, total cost, average duration.
- Filters: provider/key, time range, model, status, endpoint/type.
- Actions: Refresh, Reset, Export CSV.
- Table columns:
  - API Key / Provider
  - Model
  - Reasoning Effort
  - Endpoint
  - Type
  - Billing Mode
  - Tokens
  - Cost
  - First Token
  - Duration
  - Time
- Detail drawer or popover:
  - input cost, output cost, cache read/creation cost;
  - service tier multiplier, provider multiplier;
  - retry chain and route decision;
  - raw trace only as an advanced tab.

## Pagination And Table Density

Tables must be designed as real data surfaces, not static mockup lists.

- Prefer server-backed pagination from the first implementation. Do not use infinite scroll for request logs or provider/credential lists.
- Default page size: 25 rows. Offer 10, 25, and 50 as compact choices.
- Show pagination controls only when the result count exceeds one page. Keep the footer quiet for short local lists.
- Preserve filters, search text, sort, page, and page size in URL/search state or persisted UI state once routing exists.
- Keep the toolbar sticky within the page content only if the table can scroll vertically inside the content area; otherwise keep the whole page natural.
- Use skeleton rows for loading, inline retry for failed refresh, and an explicit stale-data timestamp.
- Keep routine tables to 6-7 visible columns at 1280 px. Move secondary fields into a row drawer, popover, or overflow menu.
- Provider credential list mode should remain the cleanest table: no charts, no right panel, no dense row action cluster.
- Usage can be denser than provider credentials, but compact token/cost details should be grouped into one or two cells and expanded through a cost/detail popover.
- Provider cards are preferred over a provider table. If a table view is later added, it should be a secondary compact mode.
- Avoid horizontal scrolling unless a user explicitly opens an advanced trace/raw data view.

### Settings

Goal: grouped settings, not a giant form.

Layout:

- Simple responsive Settings grid, not a dashboard/control-console layout.
- At desktop width, use two adaptive columns of setting cards. On narrower widths, cards stack to one column.
- Do not use a permanent right-side runtime/about sidebar. Runtime state, app info, and paths are normal setting cards, not a monitoring sidebar.
- Avoid giant full-width horizontal rows full of controls. Each card should contain a small, coherent group.
- Keep the page calm and practical, closer to mature desktop utility settings than SaaS account settings.

Cards / sections:

1. Desktop Behavior
   - Launch at login.
   - Tray enabled.
   - Close behavior: Exit app / Minimize to tray.
   - Startup behavior.
2. Local Proxy
   - Host.
   - Port.
   - Endpoint.
   - Runtime owner.
   - Admin token status.
   - Reload runtime.
3. Codex Connection
   - Switch status.
   - Preset: default, chatgpt-bridge, official-relay, official-imagegen.
   - Responses WebSocket toggle.
   - Switch On / Switch Off.
4. Appearance And Language
   - Language.
   - Theme: Follow system / Light / Dark.
   - Optional compact density preference.
5. Advanced Tools, collapsed by default
   - Session overrides.
   - Routing graph.
   - Relay diagnostics.
   - Request trace.
   - Raw TOML editor.
   - Logs folder.
6. About And Paths
   - Version.
   - Config path.
   - Logs path.
   - Cache path.
   - Check update.
7. Dangerous Actions
   - Lifecycle explanation close to the actions.
   - Quit App.
   - Detach.
   - Stop Proxy.

Lifecycle copy:

- “Started by this app”: quitting can stop the proxy.
- “Attached to existing proxy”: quitting only detaches.
- “Stop Proxy” must look more serious than “Quit App”.

## Required States

Imagegen/prototype should include normal state plus at least hints for:

- running proxy;
- stopped proxy;
- attached mode;
- no providers configured;
- no usage records;
- provider health warning;
- diagnosis recommended;
- loading skeletons;
- desktop runtime unavailable;
- stale data or failed refresh.

## Data Contract Map

Initial UI should be mockable, then wired read-only through existing admin API.

| UI Area | Primary existing source |
| --- | --- |
| Dashboard runtime cards | `/__codex_helper/api/v1/operator/summary`, `/__codex_helper/api/v1/runtime/status` |
| Recent requests | `/__codex_helper/api/v1/request-ledger/recent` |
| Usage totals and grouping | `/__codex_helper/api/v1/request-ledger/summary`, later `UsageBalanceView`-backed endpoints if exposed |
| Provider list | `/__codex_helper/api/v1/providers`, `/__codex_helper/api/v1/providers/runtime` |
| Provider balances | `/__codex_helper/api/v1/providers/balances/refresh` plus cached balance snapshots in provider/usage views |
| Provider capabilities | `/__codex_helper/api/v1/codex/relay-capabilities`, `/__codex_helper/api/v1/codex/relay-live-smoke` |
| Routing summary | `/__codex_helper/api/v1/routing`, `/__codex_helper/api/v1/routing/explain` |
| Settings runtime actions | `/__codex_helper/api/v1/runtime/reload`, `/__codex_helper/api/v1/runtime/shutdown`, plus Tauri commands for host-local lifecycle |
| Logs/files/tray/autostart | Tauri commands only |

## Copy Rules

Use user-facing terms:

- Local proxy
- Connected to Codex
- Active provider
- Default route
- Balance / quota
- Run diagnosis
- Attached mode
- Quit App
- Detach
- Stop Proxy

Avoid first-screen architecture terms:

- control plane;
- route graph;
- circuit breaker;
- affinity;
- upstream index;
- owner marker;
- runtime shutdown endpoint.

Those terms can appear in Advanced drawers or diagnostics only.

## Imagegen Prompt Pack

Use the following as the first concept prompt. Prefer Chinese UI labels for density validation, while keeping endpoint/model identifiers in English.

```text
Create a high-fidelity desktop app UI concept for a Tauri client named codex-helper.

Product: codex-helper is a local proxy and relay control center for Codex. It runs a proxy at 127.0.0.1:3211, connects Codex to it, manages relay providers and each provider's local credentials, shows usage/cost/balance, and hides advanced routing/session diagnostics behind Advanced sections.

Style: restrained mint/teal local developer dashboard, light theme, subtle cool mint/blue background wash with a very faint grid, white rounded cards, precise 1px borders, soft shadow, Inter-like typography, JetBrains Mono for endpoints/secrets. Main accent teal #0F9F8F. Use green for healthy, amber for stale/warnings, red for destructive Stop Proxy, blue/purple only for secondary metrics. No neon, no cyberpunk, no glassmorphism, no gradient text, no dense observability cockpit.

Window: 1360x860 desktop Tauri window. Fixed left sidebar 240px, content padding 28px, drag-safe top area. Chinese UI labels. Layout must support long Chinese labels.

Sidebar: brand “codex-helper”; nav items 仪表盘, 供应商, 用量, 设置; bottom utilities 深色模式, 折叠; runtime footer card showing 本地代理 Running · 3211, 已连接 Codex, v0.16.0.

Top bar: page title and subtitle, refresh button, notification icon, language selector, runtime badge Running, balance pill $12.48, local profile chip 本机.

Dashboard page: calm overview. Top compact status cards: 本地代理 Running 3211, Codex 连接 Connected, 当前供应商 CodeX Air, 今日用量 128 requests / 1.8M tokens / $0.42, 平均响应 2.4s, Provider Health 3/4 healthy. Main grid: Work Status panel with a Codex relay switch row plus Run Diagnosis / Switch On / Switch Off / Advanced Connection Settings; Recent Requests panel with five rows status/model/provider/tokens/cost/duration/time; Provider Health panel with balance and latency; Usage Trend chart. Include a small attached-mode note example.

Also create small page thumbnails or tabs for Providers, Usage, Settings: Providers cards plus right-side Default Route order and visible auth/env-var fields; provider credential list mode with masked env vars and a local endpoint pill; Usage filters and request table with cost popover hint; Settings grouped sections and a dangerous Stop Proxy area distinct from Quit App.

The overall feel should be friendly, precise, and local-first, like a mature desktop utility rather than an admin console.
```

Negative prompt:

```text
Avoid: SaaS login/payments/subscription pages, marketing hero sections, dark-only UI, neon/cyberpunk, heavy gradients, frosted glass, giant equal metric card wall, top-level Sessions/Diagnostics/Route Graph pages, raw TOML as default, modal-first destructive actions, noisy charts, overly playful illustrations.
```

## Next Providers Page Imagegen Prompt

Use this after the accepted Dashboard concept and provider credential list density reference:

```text
$imagegen Use the approved codex-helper Dashboard image as the shell reference. Use the provider credential list reference only for table density and credential-row simplicity.

Keep exactly the same codex-helper shell:
- Tauri desktop window, 1360x860, light theme
- left sidebar with codex-helper brand
- selected sidebar item: “供应商” or “Providers”
- top bar with 刷新, notification icon, 中文 selector, Running badge, 余额 $12.48, 本机
- restrained mint / teal color palette
- very light cool mint-blue background wash with subtle grid
- white rounded cards, precise 1px cool gray borders, soft shadow
- shadcn/ui-like components
- Chinese UI labels
- local-first desktop utility feeling

Do not redesign the app shell. Only replace the main content area with the 供应商 page.

Product context:
codex-helper manages local relay providers/stations for Codex. Provider API keys, tokens, and env vars are provider auth fields, not a separate top-level page. This page is for choosing the active provider order, checking health/balance/capabilities, probing providers, and editing local provider config. It is not a SaaS provider marketplace, not an API-key dashboard, and not an observability dashboard.

Page:
供应商

Subtitle:
管理 Codex relay provider、默认路由顺序和健康状态

Main layout:
Use provider cards as the primary surface, with a narrow right-side default route/order panel. Include credential/auth source on each provider card. Keep the page cleaner than a cloud admin console.

Top toolbar:
- Search input: “搜索供应商、Host 或能力”
- Service segmented control: “Codex” selected, “全部” secondary
- Tag chips: “全部”, “Healthy”, “支持 imagegen”, “支持 compact”
- Buttons on right:
  - “刷新状态”
  - “导入配置”
  - primary teal button “添加供应商”

Provider cards grid:
Use two columns of clean cards, not a dense table.

Card 1:
- Name: CodeX Air
- Host: ai.input.im
- Badges: 默认, Healthy
- Balance: $12.48
- Latency: 820ms
- Today: 128 requests · $0.42
- Capabilities: responses, compact, imagegen
- Auth: env CODEX_RELAY_API_KEY
- Actions: “Probe”, “刷新余额”, “编辑”

Card 2:
- Name: OpenAI Backup
- Host: api.openai.com
- Badges: Standby
- Balance: $31.20
- Latency: 1.2s
- Today: 12 requests · $0.09
- Capabilities: responses, compact
- Auth: env OPENAI_API_KEY
- Actions: “设为默认”, “Probe”, “编辑”

Card 3:
- Name: RightCode
- Host: relay.rightcode.dev
- Badges: Stale
- Balance: 未知
- Latency: 2.8s
- Today: 42 requests · $0.18
- Capabilities: responses
- Auth: sk-••••••••da44
- Actions: “Probe”, “刷新余额”, “编辑”

Card 4:
- Name: Test Relay
- Host: 127.0.0.1:8787
- Badges: Disabled
- Balance: -
- Latency: -
- Today: 0 requests
- Capabilities: mock, responses
- Auth: TEST_RELAY_TOKEN
- Actions: “启用”, “编辑”

Right-side panel:
Title: “默认路由顺序”
Show ordered list:
1. CodeX Air — 默认
2. OpenAI Backup — 备用
3. RightCode — 健康检查过期

Include small controls:
- “上移/下移” icons or handles
- “保存顺序” primary small button
- “高级路由设置” secondary text button

Bottom note:
“高级路由、模型映射和诊断日志放在供应商详情里，默认页面只展示健康状态和优先级。”

Visual requirements:
- Provider cards should be scannable and balanced.
- Auth/env var should be visible enough that users understand where the provider credential comes from.
- Right route-order panel should be useful but not too heavy.
- Do not show route graphs by default.
- Do not show raw TOML as the main view.
- Avoid charts on this page.
- Keep action count low; advanced actions go into menus or details.

Avoid:
standalone API Keys page, marketplace UI, SaaS billing/payment page, huge observability dashboard, route graph canvas, raw config editor as the main page, dark-only UI, cyberpunk, neon, glassmorphism, gradient text, Anthropic/Claude branding on the first providers page, excessive blue accent.
```

## Next Usage Page Imagegen Prompt

Use this after the accepted Dashboard and Providers concepts:

```text
$imagegen Use the approved codex-helper Dashboard image as the shell reference and the approved Providers page as the density/style reference.

Keep exactly the same codex-helper shell:
- Tauri desktop window, 1360x860, light theme
- left sidebar with codex-helper brand
- selected sidebar item: “用量”
- sidebar nav should only show: 仪表盘, 供应商, 用量, 设置
- top bar with 刷新, notification icon, 中文 selector, Running badge, 余额 $12.48, 本机
- restrained mint / teal color palette
- very light cool mint-blue background wash with subtle grid
- white rounded cards, precise 1px cool gray borders, soft shadow
- shadcn/ui-like components
- Chinese UI labels
- local-first desktop utility feeling

Do not redesign the app shell. Only replace the main content area with the 用量 page.

Product context:
codex-helper records local proxy requests for Codex relay providers. This page helps a developer inspect recent requests, token usage, estimated cost, first-token latency, total duration, cache usage, and route/provider result. Costs are estimates from local/provider pricing metadata; actual billing remains subject to the upstream provider. It is not a cloud billing page and not a noisy observability cockpit.

Page:
用量

Subtitle:
查看 Codex 中转请求、Token、成本估算和延迟表现

Main layout:
A clear request-history page with compact summary cards, filters, and one full-width primary table. Use the sub2api reference only for practical table density, pagination, cost tooltip, token icons, first-token column, and page footer. Do not copy its account/subscription/sidebar product structure.

Top summary cards:
Use four compact cards:
- 今日请求: 128
- 今日 Tokens: 1.8M
- 预估花费: $0.42
- 平均响应: 2.4s

Filter toolbar:
- Time range segmented control: “今天”, “7 天”, “30 天”
- Provider select: “全部供应商”
- Model select: “全部模型”
- Status select: “全部状态”
- Search input: “搜索 request id、模型或供应商”
- Buttons on right:
  - “刷新”
  - “重置”
  - “导出 CSV”

Main table card:
Title: “请求记录”
Subtitle: “本机代理记录的最近请求，用于排查预估成本、延迟和路由结果。”

Table layout:
- Full width. Do not reserve a permanent right-side detail panel.
- Use compact row height, sticky header, subtle row dividers, and a bottom pagination footer.
- If the table is wider than the content area, use a quiet horizontal scrollbar inside the table card and keep the first column visually stable.
- Add small info icons beside estimated cost and token/cache values.
- Show one open dark tooltip anchored to the first row's “预估费用” info icon, similar to the sub2api reference, but label all values as estimated.

Table columns:
- 状态
- 模型
- 推理
- 供应商
- Endpoint
- 类型
- 计费
- Tokens
- 预估费用
- 首 Token
- 耗时
- 时间
- 客户端
- 操作

Rows:
1. 200 成功
   gpt-5.5
   XHigh
   CodeX Air
   /v1/responses
   同步
   按量
   12,543 in ↓ · 45,210 out ↑ · 38% cache
   $0.028
   740ms
   2.1s
   15:24:31
   codex-tui / Windows
   查看

2. 200 成功
   gpt-5.5
   XHigh
   CodeX Air
   /v1/responses
   流式
   按量
   8,302 in ↓ · 22,118 out ↑ · 42% cache
   $0.014
   690ms
   1.6s
   15:18:07
   codex-tui / Windows
   查看

3. 200 成功
   gpt-5.4
   High
   OpenAI Backup
   /v1/responses
   同步
   按量
   3,219 in ↓ · 9,876 out ↑ · 35% cache
   $0.006
   820ms
   1.2s
   15:12:44
   codex-tui / Windows
   查看

4. 429 限流
   gpt-5.5
   XHigh
   RightCode
   /v1/responses
   流式
   按量
   1,204 in ↓ · 0 out
   $0.000
   -
   900ms
   15:08:11
   codex-tui / Windows
   重试详情

5. 200 成功
   gpt-5.5
   High
   CodeX Air
   /v1/responses/compact
   同步
   按量
   15,991 in ↓ · 61,203 out ↑ · 41% cache
   $0.041
   780ms
   2.7s
   15:06:22
   codex-tui / Windows
   查看

Cost tooltip:
Show a small dark tooltip/popover near the first row's cost cell:
- Title: “预估成本明细”
- Input estimate: $0.006
- Output estimate: $0.018
- Cache read estimate: $0.004
- Provider multiplier: 1.0x
- Final estimate: $0.028
- Small note: “实际费用以供应商结算为准”

Optional small chart:
Do not show a chart in this screenshot unless there is clear spare space below the table. The full-width request table is more important than the chart.

Pagination:
At table bottom show:
- “显示 1 至 20，共 128 条”
- page size select: “每页 20”
- numbered pagination: 1, 2, 3, …, 7
- previous/next icon buttons

Visual requirements:
- Table is the main focus.
- Keep token/cost cells compact and readable.
- Use “预估费用” and “预估花费”, not exact “费用/总消费” language.
- Cost detail appears as a tooltip/popover from an info icon, not as a permanent right-side panel.
- Use green success badges, amber warning/error badges, muted gray for secondary metadata.
- Do not use a billing/subscription style.
- Do not show account invoices, payment method, or user seats.
- Do not show raw trace JSON by default; details can be opened by row action later.

Avoid:
SaaS billing dashboard, invoices, payment cards, subscription UI, huge chart wall, dense observability cockpit, route graph canvas, raw trace JSON as main view, dark-only UI, cyberpunk, neon, glassmorphism, gradient text, standalone API Keys page, API Keys sidebar item, excessive blue accent.
```

## Next Settings Page Imagegen Prompt

Use this after the accepted Dashboard, Providers, and Usage concepts, and after rejecting the first over-complicated Settings concept:

```text
$imagegen Use the approved codex-helper Dashboard image as the shell reference, and use the approved Providers and Usage pages as density/style references.

Also use these Settings-specific references:
- Anti-reference: settings-generated-too-complex-v1.png. Do not repeat its crowded control-console density, giant horizontal rows, or permanent runtime sidebar feeling.
- Structure references: settings-simple-reference-a.png and settings-simple-reference-b.png. Borrow their simple desktop-utility settings feel: grouped cards, compact controls, two adaptive columns, low visual noise.

Keep exactly the same codex-helper shell:
- Tauri desktop window, 1360x860, light theme
- left sidebar with codex-helper brand
- selected sidebar item: “设置”
- sidebar nav should only show: 仪表盘, 供应商, 用量, 设置
- top bar with 刷新, notification icon, 中文 selector, Running badge, 余额 $12.48, 本机
- restrained mint / teal color palette
- very light cool mint-blue background wash with subtle grid
- white rounded cards, precise 1px cool gray borders, soft shadow
- shadcn/ui-like components
- Chinese UI labels
- local-first desktop utility feeling

Do not redesign the app shell. Only replace the main content area with the 设置 page.

Product context:
codex-helper is a local desktop proxy helper for Codex. Settings manages desktop behavior, local proxy runtime, Codex connection, appearance, paths, advanced tools, and dangerous lifecycle actions. It should not look like a SaaS account settings page or a dense observability console.

Page:
设置

Subtitle:
配置桌面行为、本地代理、Codex 连接和高级工具

Important layout:
Use a simple responsive two-column settings grid, like mature desktop utility settings.
- No permanent right sidebar
- No giant full-width form
- No dense dashboard of status panels
- Cards should be medium-sized setting groups
- Cards can align in two columns on desktop and naturally stack on narrower widths
- Each card has a title, one-line description, and compact controls
- Keep spacing calm and readable

Top status strip:
Under the title, show a very slim full-width status strip:
- 本地代理 Running · 3211
- Codex 已连接
- 当前供应商 CodeX Air
- 最近刷新 12 秒前
- small button: 刷新状态
This strip should be subtle, not a dashboard panel.

Card 1: 桌面行为
Description: 控制应用启动、托盘和窗口关闭方式。
Controls:
- 开机启动 toggle off
- 启用托盘 toggle on
- 关闭窗口时 segmented control: 最小化到托盘 selected / 退出应用
- 启动时自动启动本地代理 toggle on

Card 2: 外观与语言
Description: 调整界面语言和显示偏好。
Controls:
- 默认语言 select: 中文
- 主题 segmented control: 跟随系统 selected / 浅色 / 深色
- 界面密度 segmented control: 舒适 selected / 紧凑

Card 3: 本地代理
Description: 本机代理监听地址和运行时配置。
Controls:
- Host input: 127.0.0.1
- Port input: 3211
- Endpoint pill: http://127.0.0.1:3211 with copy icon
- Runtime owner badge: 由此应用启动
- Admin token badge: 已配置
Actions:
- 复制 Endpoint
- 重新加载运行时
- 打开日志目录

Card 4: Codex 连接
Description: 控制 Codex 是否通过本地代理中转。
Controls:
- Codex 中转 switch on
- 当前预设 select: chatgpt-bridge
- 当前供应商 select: CodeX Air
- Responses WebSocket toggle on
- capability badges: responses, compact, imagegen
Actions:
- 运行诊断
- 切换预设
- 关闭中转

Card 5: 高级工具
Description: 日常使用不需要打开这些选项。
Use collapsed accordion/list rows, visually secondary:
- 会话覆盖 — 为单个会话选择 provider 或模型策略 — 打开
- 高级路由 — 查看模型映射和默认路由 — 打开
- Relay 诊断 — 探测连接和能力 — 打开
- 请求 Trace — 查看详细请求链路 — 打开
- 原始配置 — 只读预览配置文件 — 打开
- 日志与缓存 — 打开日志、缓存目录 — 打开

Card 6: 关于与路径
Description: 版本、本机路径和更新信息。
Show:
- Version: v0.16.0
- Config: ~/.codex-helper/config.toml
- Logs: ~/.codex-helper/logs
- Cache: ~/.codex-helper/cache
Actions:
- 打开配置目录
- 检查更新

Card 7: 危险操作
Description:
退出应用、Detach 和 Stop Proxy 是不同动作。若只是关闭窗口，请使用退出或最小化到托盘；Stop Proxy 会停止当前本地代理运行时。
Show three concise lifecycle rows:
- 退出应用 — 关闭桌面客户端
- Detach — 仅断开当前窗口，不停止已有代理
- Stop Proxy — 停止本地代理运行时
Actions:
- secondary button: 退出应用
- secondary button: Detach
- red danger button: Stop Proxy
Stop Proxy must be visually stronger than Quit App or Detach, but the card should still be compact and restrained.

Visual requirements:
- Settings should feel simpler than Dashboard, Providers, and Usage.
- Make it look like a practical desktop utility settings page.
- Two-column adaptive card layout is preferred.
- Avoid a permanent right status/about sidebar.
- Avoid over-packed full-width rows with too many columns.
- Avoid raw TOML as the main view.
- Advanced tools should be collapsed and visually secondary.
- Dangerous actions should be at the bottom and clearly separated.
- Keep text compact; do not overfill the screen.

Avoid:
SaaS account/profile settings, billing/subscription settings, user management, API Keys sidebar item, marketplace provider browsing, raw TOML editor as the main screen, route graph canvas, dense observability dashboard, huge charts, dark-only UI, cyberpunk, neon, glassmorphism, gradient text, excessive blue accent.
```

## Acceptance Checklist

- The first screen answers proxy/Codex/provider/usage/health/action clearly.
- Top-level navigation remains four pages: Dashboard, Providers, Usage, Settings.
- Advanced features are visible but not dominant.
- Quit, Detach, and Stop Proxy are visually and textually distinct.
- Provider management shows cards and active order, not only a dense table.
- Usage table is readable at 1280 px width.
- Sidebar footer clearly shows proxy status and port.
- Empty/warning/stale states teach the next action.
- Visual language matches the mint/teal restrained product direction.

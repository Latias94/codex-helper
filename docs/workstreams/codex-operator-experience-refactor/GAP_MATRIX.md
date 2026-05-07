# Gap Matrix: Codex Operator Experience

> 中文速览：这份矩阵把当前项目、`cc-switch`、`aio-coding-hub`、Codex 源码语义放在一起看。结论是：我们不需要复制它们的全产品，但需要补齐“请求可观测 + 成本/余额 + 策略解释 + TUI 稳定性”这条链路。

## Reference Summary

### `repo-ref/cc-switch`

Useful patterns:

- desktop-first provider management
- provider presets
- system tray switching
- balance / usage scripts
- request logs with token, cache, latency, first-token, and cost fields
- model pricing table
- failover queue and circuit breaker UI

Relevant files:

- `repo-ref/cc-switch/README_ZH.md`
- `repo-ref/cc-switch/src/types/usage.ts`
- `repo-ref/cc-switch/src-tauri/src/database/schema.rs`
- `repo-ref/cc-switch/src-tauri/src/proxy/usage/logger.rs`
- `repo-ref/cc-switch/src-tauri/src/proxy/usage/parser.rs`
- `repo-ref/cc-switch/src/components/usage/UsageDashboard.tsx`
- `repo-ref/cc-switch/src/components/usage/RequestLogTable.tsx`

Do not copy blindly:

- It is broader than Codex and manages multiple CLI ecosystems.
- Its desktop stack and SQLite-first product choices do not need to dictate this Rust workspace's core shape.

### `repo-ref/aio-coding-hub`

Useful patterns:

- unified local gateway product framing
- intelligent failover with circuit state
- sticky session
- request trace and provider chain UI
- cache-aware usage fields
- high-precision cost calculation
- model price sync with cache metadata
- provider limit/cost windows

Relevant files:

- `repo-ref/aio-coding-hub/README.md`
- `repo-ref/aio-coding-hub/src-tauri/src/infra/request_logs/types.rs`
- `repo-ref/aio-coding-hub/src-tauri/src/domain/cost.rs`
- `repo-ref/aio-coding-hub/src-tauri/src/infra/model_prices_sync.rs`
- `repo-ref/aio-coding-hub/src/components/ProviderChainView.tsx`
- `repo-ref/aio-coding-hub/src/components/home/RequestLogDetailSummaryTab.tsx`
- `repo-ref/aio-coding-hub/src/services/gateway/traceStore.ts`

Do not copy blindly:

- It is a broader multi-CLI gateway.
- Its trace/event model is useful, but our route semantics must stay station/profile/session-binding-first.

### `repo-ref/codex`

Useful semantics:

- `model_provider`
- `service_tier`
- `input_tokens`
- `cached_input_tokens`
- `output_tokens`
- `reasoning_output_tokens`

Relevant files:

- `repo-ref/codex/codex-rs/analytics/src/events.rs`
- `repo-ref/codex/codex-rs/analytics/src/facts.rs`
- `repo-ref/codex/codex-rs/tui/src/status/card.rs`
- `repo-ref/codex/codex-rs/core/src/tasks/mod.rs`

Use it for:

- field naming alignment
- Codex-native status/usage expectations
- avoiding invented terminology where Codex already has a clear concept

## Capability Matrix

| Capability | Current `codex-helper` | `cc-switch` signal | `aio-coding-hub` signal | Gap | Priority |
| --- | --- | --- | --- | --- | --- |
| Codex-first relay | Strong | Supports Codex among many CLIs | Supports Codex among many CLIs | Keep our specialization | Keep |
| Station/profile/session binding | Strong | Provider-oriented | Provider/CLI-oriented | Our advantage; preserve | Keep |
| TUI stability | Known issues in Stations/header/resize | GUI-first | GUI-first | TUI needs render hygiene | P0 |
| GUI operator console | Exists, still maturing | Strong desktop UI | Strong dashboard UI | Needs request/cost/balance polish | P1 |
| Request JSONL | Exists | SQLite request logs | SQLite request logs | Needs v2 schema and trace chain | P0 |
| Trace ID | Partial/local request ID | Request ID | Trace ID first-class | Add stable trace key across events | P0 |
| Route chain | Retry info exists, not detailed enough | Failover queue/status | Provider chain detail | Attempt-level decisions needed | P0 |
| Service tier / fast | Requested/effective/actual exists | Provider/model dependent | Request detail badges | Need consistent UI and pricing linkage | P0/P1 |
| Token usage | Basic input/output/reasoning/total | Cache read/create | Cache read/create/5m/1h | Usage v2 needed | P0 |
| Cost calculation | Core cache-aware pricing engine + confidence labels | Model pricing table | High precision cost engine | Add source-backed sync / overrides | P1 |
| Model price sync | Not first-class | Seeded pricing table | External sync + cache | Add optional source-backed catalog | P1 |
| Balance/quota | `usage_providers.rs` marks exhausted | Balance scripts/adapters | Provider limit/cost windows | Promote to first-class balance state | P1 |
| Health vs quota | Partially separated by `usage_exhausted` | Provider/circuit UI | Circuit + limits | Need clear semantics and UI | P1 |
| Automatic switching | HA/failover base exists | Failover queue | Intelligent failover/sticky | Need policy preview and explanation | P1/P2 |
| Long-horizon audit | JSONL + runtime rollups | SQLite | SQLite + charts | Consider SQLite after v2 schema | P2 |
| Provider presets | Configurable, not polished as marketplace | 50+ presets | Provider management | Add curated Codex relay templates later | P2 |
| LAN/remote attach | Strong existing workstream | Desktop-local | Desktop-local gateway | Preserve remote-safe boundaries | Keep/P2 |

## Current Project Findings

### TUI

Observed from local files:

- `crates/tui/src/tui/mod.rs`
  - page switches call `terminal.clear()`
  - `Event::Resize` only sets `should_redraw = true`
- `crates/tui/src/tui/view/pages/stations.rs`
  - Stations table does not use the same `HighlightSpacing::Always` pattern used by other table pages
- `crates/tui/src/tui/view/chrome.rs`
  - header/footer strings are dense and single-line oriented

Likely effect:

- stale cells or repeated-row artifacts after selection movement, resize, or narrow layout pressure.

Recommended fix:

- full clear on resize
- consistent highlight spacing
- width-aware chrome compaction

### Usage and Cost

Observed from local files:

- `crates/core/src/usage.rs`
  - `UsageMetrics` now normalizes input/output/reasoning, cached input, cache read, cache creation, and TTL-specific cache creation fields.
- `crates/core/src/pricing.rs`
  - core owns cache-aware request cost calculation, model price lookup, and confidence labels.
- `crates/core/src/state/runtime_types.rs`
  - request and rollup cost fields are carried through `UsageBucket` / `CostSummary`.
- `crates/gui/src/gui/pages/stats_summary.rs` and `crates/tui/src/tui/view/stats.rs`
  - UI renders core-computed cost and confidence instead of doing local price math.

Gap:

- model price sync is still not source-backed
- local price overrides now have a typed CLI and a local-running GUI editor under Stats
- long-horizon cost audit still depends on runtime rollups and JSONL replay

Recommended fix:

- add optional source-backed price catalog sync
- add source-backed catalog sync and provider-specific price/multiplier presets when the pricing workspace matures
- evaluate SQLite only after request/usage/cost schema stops moving

### Balance and Quota

Observed from local files:

- `crates/core/src/usage_providers.rs`
  - default providers include PackyCode budget HTTP JSON and YesCode profile
  - polling is request-adjacent and rate-limited
  - results update load-balancer `usage_exhausted`
- `crates/core/src/lb.rs`
  - load balancer can skip usage-exhausted upstreams

Gap:

- useful behavior exists but is not productized as balance status
- balance/quota is not yet visible enough in station detail
- stale/error/exhausted states need stronger modeling

Recommended fix:

- first-class balance snapshot DTO
- UI station balance display
- route eligibility reason: quota exhausted

### Request Chain

Observed from local files:

- `crates/core/src/logging.rs`
  - request logs include station/provider/upstream/session/usage/retry/service tier
- `crates/gui/src/gui/pages/components/request_details.rs`
  - GUI can derive output tokens per second from usage/duration/ttfb
  - request detail already has route-oriented display hooks

Gap:

- retry info is not a full decision chain
- skipped providers/stations are not explained enough
- trace ID is not yet the universal join key

Recommended fix:

- attempt-level DTO with decision/reason/circuit/balance/capability fields
- route chain in GUI/TUI request details

## Priority Recommendation

### Do First

- TUI render hygiene.
- Usage v2 schema.
- Request trace/attempt chain.
- API DTO alignment.

Rationale:

- These are semantic and trust foundations.
- Cost, balance, and policy UI will be wrong or duplicated if built before this.

### Do Second

- Core pricing engine.
- Balance adapter model.
- GUI request observatory.
- TUI parity summaries.
- Policy preview.

Rationale:

- These are the visible features users are comparing with `cc-switch` and `aio-coding-hub`.
- They should build on the v2 request facts.

### Do Later

- SQLite ledger.
- provider preset marketplace.
- richer charts.
- advanced route policy engine.
- WebUI/LAN expansion.

Rationale:

- Valuable, but premature before request/cost/balance semantics stabilize.

## Product Positioning

The strongest positioning is:

**`codex-helper` is the Codex-first local relay and operator console for people who want reliable provider switching without losing session continuity.**

`cc-switch` is broader desktop provider management.

`aio-coding-hub` is a broader multi-CLI gateway.

`codex-helper` should win by being:

- closer to Codex semantics
- safer about session continuity
- clearer about effective route
- good enough in GUI
- still excellent in TUI
- honest about cost, balance, and failover decisions

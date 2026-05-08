# Design: Codex Operator Experience

> 中文速览：目标不是把 GUI 做成另一个供应商列表，而是让用户在 Codex 工作流中清楚看到“当前请求走了哪里、为什么这么走、花了多少钱、还能不能继续用、是否应该切换”。核心设计是统一请求观测模型、成本模型、余额模型和策略解释模型。

## Problem Statement

The current project already has a strong Codex-first control-plane base. The remaining product gap is the operator experience around that base.

An operator still needs better answers to these questions:

- Did the TUI render stale rows or status cells?
- Which provider/station handled this request?
- Was this request fast/priority/default, and was that requested or actually returned?
- How many input, output, reasoning, cached, cache-read, and cache-created tokens were used?
- What did the request cost, and how confident is that number?
- Which stations were considered, skipped, retried, or failed over?
- Is a provider unhealthy, quota-exhausted, stale, or simply unsupported for this model?
- What policy will be used for the next request in this session?

## Design Goals

- Stabilize the TUI so operator trust is not undermined by visual artifacts.
- Make request observability a canonical core domain.
- Make token usage cache-aware and aligned with Codex-native semantics.
- Add a pricing engine that supports cache and service-tier differences.
- Promote balance/quota polling into first-class provider status.
- Make routing decisions and failover chains explainable.
- Let GUI and TUI render from the same API/DTO contract.
- Keep Codex session continuity as a hard product constraint.

## Non-goals

- Cloning the full `cc-switch` product.
- Becoming a general multi-CLI marketplace before Codex UX is excellent.
- Adding desktop UI features that bypass core semantics.
- Treating every provider-specific API as a first-class built-in before an adapter contract exists.
- Enabling aggressive cross-station failover by default.

## Product Model

### Operator Console

The operator console is the product layer above the control plane.

It contains:

- `Overview`
  - active sessions
  - active requests
  - recent failures
  - cost today
  - stations needing attention
- `Requests`
  - request list
  - request detail
  - route chain
  - usage/cost/cache/timing cards
  - raw sanitized trace
- `Stations`
  - health
  - balance/quota
  - route eligibility
  - drain/breaker
  - capability mismatch warnings
- `Profiles / Policies`
  - session binding
  - fast/default/deep-think policy templates
  - failover boundaries
  - cost/latency/quota preferences
- `Pricing / Balances`
  - model price catalog
  - sync status
  - provider balance adapters
  - unknown/stale/exact confidence states

### TUI Contract

TUI is not expected to match every GUI detail, but it must cover the critical operator loop:

- stable navigation
- current session route
- station health/eligibility
- recent request status
- fast/service tier
- key usage and cost summaries
- balance/exhaustion state
- policy hints for automatic switching

## Current Architecture Read

Current local modules relevant to this workstream:

- TUI render loop:
  - `crates/tui/src/tui/mod.rs`
  - `crates/tui/src/tui/view/chrome.rs`
  - `crates/tui/src/tui/view/pages/stations.rs`
- Usage parsing:
  - `crates/core/src/usage.rs`
- Request logs and control trace:
  - `crates/core/src/logging.rs`
  - `crates/core/src/request_ledger.rs`
- Runtime/session/request state:
  - `crates/core/src/state.rs`
  - `crates/core/src/state/session_identity.rs`
  - `crates/core/src/state/runtime_types.rs`
- Provider usage/quota polling:
  - `crates/core/src/usage_providers.rs`
- GUI request detail and stats:
  - `crates/gui/src/gui/pages/components/request_details.rs`
  - `crates/gui/src/gui/pages/stats_summary.rs`
- TUI stats:
  - `crates/tui/src/tui/view/stats.rs`

## Target Architecture

```text
Codex client
    |
    v
Proxy data plane
    |
    +--> route policy evaluator
    |       |
    |       +--> station health
    |       +--> balance/quota snapshots
    |       +--> capability catalog
    |       +--> session binding/profile policy
    |
    +--> observed request event
            |
            +--> runtime state projection
            +--> request JSONL / future ledger
            +--> control trace stream
            +--> operator API DTO
                    |
                    +--> TUI
                    +--> GUI
                    +--> future WebUI/attach clients
```

The core rule:

**A request is finalized once, then projected many ways.**

## Canonical Request Observability

### Request Identity

Add or normalize:

- `trace_id`
  - stable join key across attempt events, request completion, control trace, and UI
- `request_id`
  - local monotonically useful ID if already present
- `session_id`
- `client/device`
- `cwd`
- `service`
- `method/path`

### Model and Tier Fields

Separate requested, effective, and actual fields:

- `requested_model`
- `effective_model`
- `actual_model`
- `requested_service_tier`
- `effective_service_tier`
- `actual_service_tier`
- `fast_mode`
  - derived display field, not a replacement for service tier
- `reasoning_effort`

Reason:

- Codex and upstream relays may rewrite or normalize model/tier values.
- Operators need to know whether a mismatch came from request payload, profile, station mapping, or upstream response.

### Usage v2

Recommended normalized DTO:

```rust
pub struct TokenUsageV2 {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_creation_5m_input_tokens: i64,
    pub cache_creation_1h_input_tokens: i64,
}
```

Parsing should understand:

- Codex/OpenAI-style:
  - `input_tokens`
  - `output_tokens`
  - `total_tokens`
  - `input_tokens_details.cached_tokens`
  - `output_tokens_details.reasoning_tokens`
- Chat Completions compatibility:
  - `prompt_tokens`
  - `completion_tokens`
  - `prompt_tokens_details.cached_tokens`
  - `completion_tokens_details.reasoning_tokens`
- Anthropic-style:
  - `cache_read_input_tokens`
  - `cache_creation_input_tokens`
  - `cache_creation_5m_input_tokens`
  - `cache_creation_1h_input_tokens`

Codex reference signal:

- `repo-ref/codex` tracks `input_tokens`, `cached_input_tokens`, `output_tokens`, and `reasoning_output_tokens` in analytics/status surfaces.

### Performance Fields

Keep and standardize:

- `duration_ms`
- `ttfb_ms`
- `generation_ms`
- `output_tokens_per_second`
- `attempt_count`
- `streaming`

`generation_ms` should be derived consistently:

- `duration_ms - ttfb_ms` when valid
- otherwise `duration_ms`

## Route Decision Chain

Each request should expose a chain of route decisions.

Recommended attempt DTO:

```rust
pub struct RouteAttempt {
    pub attempt_index: u32,
    pub station_name: String,
    pub provider_id: Option<String>,
    pub upstream_base_url: String,
    pub decision: RouteDecisionKind,
    pub reason: Option<String>,
    pub skipped: bool,
    pub status_code: Option<u16>,
    pub error_kind: Option<String>,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub circuit_state_before: Option<String>,
    pub circuit_state_after: Option<String>,
    pub balance_state: Option<String>,
    pub capability_match: Option<bool>,
}
```

Decision kinds:

- `selected`
- `skipped_disabled`
- `skipped_draining`
- `skipped_breaker_open`
- `skipped_quota_exhausted`
- `skipped_capability_mismatch`
- `failed_transport`
- `failed_status`
- `retried_same_station`
- `failed_over_cross_station`
- `completed`

UI should answer:

- what was tried
- what was skipped
- what failed
- why the final station won
- whether failover crossed a session-continuity boundary

## Pricing Engine

### Price Catalog

Core owns a local price catalog:

- bundled seed prices for common Codex/OpenAI models
- local overrides
- optional sync adapter
- source metadata:
  - source name
  - fetched_at
  - etag / last-modified when supported
  - version/hash

Candidate external source:

- `https://basellm.github.io/llm-metadata/api/all.json`

This is a candidate, not a hard dependency. The product must still work with bundled/local prices.

### Cost Amount

Use integer precision internally.

Recommended:

- store USD as femto-USD integer where detailed per-token prices matter
- format to decimal strings only at API/UI boundaries

Reason:

- per-token prices can be tiny
- floats are acceptable for charts but not as the ledger source of truth

### Cost Breakdown

Recommended DTO:

```rust
pub struct CostBreakdown {
    pub input_cost_usd: Option<String>,
    pub output_cost_usd: Option<String>,
    pub cache_read_cost_usd: Option<String>,
    pub cache_creation_cost_usd: Option<String>,
    pub service_tier_multiplier: Option<String>,
    pub provider_cost_multiplier: Option<String>,
    pub total_cost_usd: Option<String>,
    pub confidence: CostConfidence,
    pub pricing_source: Option<String>,
}
```

Confidence:

- `exact`
- `estimated`
- `unknown`

Unknown must not be rendered as zero.

### Service Tier / Fast Pricing

The pricing engine needs hooks for:

- priority/fast tier multiplier
- flex/default tier differences
- provider-specific markups/discounts
- long-context premiums if supported by a price source

## Balance and Quota Model

Current `usage_providers.rs` should evolve into balance adapters.

Recommended DTO:

```rust
pub struct ProviderBalanceSnapshot {
    pub provider_id: String,
    pub station_name: Option<String>,
    pub upstream_index: Option<usize>,
    pub source: String,
    pub fetched_at_ms: u64,
    pub stale_after_ms: Option<u64>,
    pub stale: bool,
    pub status: BalanceSnapshotStatus,
    pub exhausted: Option<bool>,
    pub total_balance_usd: Option<String>,
    pub subscription_balance_usd: Option<String>,
    pub paygo_balance_usd: Option<String>,
    pub monthly_budget_usd: Option<String>,
    pub monthly_spent_usd: Option<String>,
    pub error: Option<String>,
}
```

Rules:

- quota exhausted means route-ineligible
- balance stale means unknown, not exhausted
- balance API failure should be visible but should not automatically trip health
- hard auth failures may still be provider faults when proven

Adapter types:

- built-in HTTP JSON budget adapter
- YesCode profile adapter
- custom HTTP extractor adapter
- custom command/script adapter only if sandbox and secret redaction are explicit

## Automatic Switching Policy

Policies should be visible templates, not hidden behavior.

Recommended policy dimensions:

- session stickiness
- same-station retry
- cross-station failover before first output
- cross-station failover after first output
- health weight
- latency weight
- cost weight
- quota/balance eligibility
- capability requirements
- fast/service tier requirements

Suggested default profiles:

- `manual-sticky`
  - no automatic cross-station failover
- `balanced`
  - same-station retry, cautious cross-station before first output
- `fast-first`
  - requires fast/priority support, latency weighted
- `cost-primary`
  - quota/cost weighted, still preserves session continuity
- `recovery`
  - more aggressive pre-output failover, operator-visible

The UI must preview:

- eligible stations
- skipped stations and reasons
- fallback order
- behavior after first token

Current GUI direction:

- Retry edits show a draft resolved policy before writeback.
- Drafts that allow cross-station failover before first output are called out as a session-continuity risk.
- Runtime resolved policy remains visible separately so users can compare current behavior with the pending form.

## TUI Stabilization Design

### Render Invalidation

Replace boolean-only redraw with explicit invalidation.

Suggested model:

```rust
enum RenderInvalidation {
    Redraw,
    FullClear,
}
```

Events requiring full clear:

- page switch
- terminal resize
- entering/leaving large overlay if stale cells are observed
- theme/layout mode changes

Current implementation uses explicit redraw vs full-clear invalidation, with page switches and terminal resize forcing a clear before the next draw.

### Table Stability

All stateful tables should use consistent highlight spacing.

Known immediate target:

- `crates/tui/src/tui/view/pages/stations.rs`

Other table pages already use `HighlightSpacing::Always` in several places. Stations should align with that behavior.

Stations now also synchronizes table offset from the selected row and visible row count before rendering. This keeps navigation deterministic instead of relying on ratatui's render-time offset mutation as the only source of truth.

### Header and Footer Compaction

`chrome.rs` should expose width-aware lines:

- wide:
  - full route/status text
- medium:
  - compact labels
- narrow:
  - key route and health only

Avoid long single-line paragraphs that rely on terminal wrapping.

Current header implementation fits lines by display width, including CJK characters, and compacts page tabs to numeric labels while preserving the selected page label on narrow terminals.

## GUI Design Direction

GUI should become the richest operator console, but should still consume core-owned DTOs.

### Request Detail

Request detail should be split into sections:

- identity
- timing
- token usage
- cost
- cache
- route chain
- service tier / fast
- raw sanitized trace

### Station Detail

Station detail should show:

- enabled/drain/breaker
- health
- balance/quota
- capability
- cost multiplier
- recent success/error
- route eligibility
- policy role

### Overview

Overview should answer:

- is the relay healthy
- what is active now
- what failed recently
- what costs are accumulating today
- which stations are blocked by balance/health/policy
- which sessions have overrides

## Storage Direction

Keep JSONL as the simple append-only log for now, but design the DTO so SQLite can be added later without another semantic rewrite.

JSONL remains good for:

- local audit trail
- simple export
- debugging

SQLite becomes useful for:

- long-horizon aggregation
- filtered request search
- price/balance history
- charts
- retention policies

Do not start with SQLite until the request/usage/cost schema is stable enough.

Current bridge:

- `crates/core/src/request_ledger.rs` owns JSONL-backed request log reading, filtering, usage aggregation, and compact request formatting.
- `codex-helper usage tail/summary/find` now uses that core query API instead of carrying private CLI parsing logic.
- `codex-helper usage summary --by station|provider|model|session` gives immediate long-horizon grouped usage views while the durable ledger remains undecided.
- GUI Requests can opt into the local JSONL ledger while the proxy is running locally, projecting log rows back into the shared `FinishedRequest` detail/list components. Attached mode stays runtime/API-backed until a remote-safe ledger API exists.
- This validates the operator query surface before choosing a durable index. Future SQLite should be a rebuildable query/cache layer over canonical request records, while JSONL remains the export/debug source.

## Migration Strategy

1. Add fields with serde defaults.
2. Keep old `UsageMetrics` compatibility readers.
3. Emit v2 request DTOs from new completions.
4. Replay old logs into v2 with unknown cache/cost fields.
5. Move UI rendering to v2 fields.
6. Remove duplicated UI cost math.
7. Promote balance adapters while reading legacy `usage_providers.json`.

## Open Questions

- Should cost ledger store femto-USD as `i64` or an explicit decimal string in public API?
- Should price sync be opt-in by default because it reaches an external URL?
- How much custom balance scripting is acceptable before it becomes a security/support burden?
- Should GUI expose after-first-token cross-station failover only behind an advanced warning?
- Should request trace data remain JSONL-first or move to SQLite once v2 is stable?

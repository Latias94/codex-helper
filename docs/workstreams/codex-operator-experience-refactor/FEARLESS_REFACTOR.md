# Fearless Refactor Doctrine: Codex Operator Experience

> 中文速览：这份文档不是鼓励粗暴重写，而是定义“什么时候应该清理旧设计、什么时候必须保持兼容”。目标是把核心产品做正确：请求可解释、费用可信、余额清楚、切换策略安全，TUI/GUI 都基于同一套控制平面事实。

## Product Thesis

`codex-helper` should optimize for one primary workflow:

> A user is actively using Codex, and the local relay lets them inspect, steer, and recover provider routing without breaking flow.

This implies:

- Codex session continuity is more important than maximizing generic failover.
- Request observability is not optional telemetry; it is a control-plane input.
- Price and balance data must be explainable enough to affect routing decisions.
- GUI/TUI are surfaces over the same operator contract, not separate products with separate truths.

## Refactor Permission

Fearless refactor is allowed when it removes ambiguity in one of these areas:

- request identity and trace identity
- usage token semantics
- cost calculation semantics
- balance/quota semantics
- provider/station health semantics
- session continuity and failover boundaries
- TUI rendering correctness
- API DTO consistency across GUI/TUI/attach clients

Fearless refactor is not permission to:

- break existing Codex traffic paths for cosmetic cleanup
- remove legacy persisted config compatibility without a migration path
- silently enable cross-station failover after first output
- let GUI invent fields that core cannot expose
- make balance depletion look the same as upstream transport failure

## Deletion and Replacement Candidates

### Replace Env-only UI Pricing

Previous direction:

- `crates/tui/src/tui/view/stats.rs`
- `crates/gui/src/gui/pages/stats_summary.rs`

These used to be the risk zone for simple UI-side input/output price estimates.

Target:

- A core pricing engine owns model price lookup and cost calculation.
- UI code only renders already computed cost summaries and confidence labels.
- Env prices may remain as a local override source, not as the primary architecture.

Current status:

- Core owns cache-aware request cost calculation and rollup cost summaries.
- GUI/TUI render core cost totals, parts, and confidence labels.
- The remaining work is price catalog sync / local overrides, not UI-side math removal.

Removal condition:

- Request and rollup DTOs expose cost fields.
- Pricing engine has tests for input/output/cache-read/cache-create/service-tier multiplier.
- UI has no duplicated cost math.

### Promote Usage Providers into Balance Adapters

Current direction:

- `crates/core/src/usage_providers.rs`
- `usage_providers.json`
- load-balancer `usage_exhausted` flags

This already gives useful quota-aware behavior, but the concept is too narrow.

Target:

- `balance` or `provider_status` domain module.
- Typed balance snapshots:
  - total balance
  - subscription balance
  - pay-as-you-go balance
  - monthly budget
  - monthly spent
  - exhausted
  - source
  - fetched_at
  - stale/error state
- Balance affects route policy through an explicit signal, not by masquerading as transport health.

Removal condition:

- Existing `usage_providers.json` can be migrated or read as compatibility input.
- Station/upstream state can show balance and usage exhaustion separately.
- Tests cover "quota exhausted skips route" without poisoning health.

### Split Raw Usage from Normalized Billing Usage

Current direction:

- `crates/core/src/usage.rs` has `UsageMetrics` with input/output/reasoning/total.

Target:

- `RawUsagePayload`
  - stored for diagnostics when available
- `TokenUsage`
  - normalized request usage for display and rollups
- `BillingUsage`
  - cache-aware and pricing-ready usage:
    - input tokens
    - cached input tokens
    - output tokens
    - reasoning output tokens
    - cache read input tokens
    - cache creation input tokens
    - cache creation 5m input tokens
    - cache creation 1h input tokens

Removal condition:

- Existing JSONL logs deserialize through defaults.
- Existing TUI/GUI token displays keep working.
- New fields are tested against Codex, OpenAI-compatible, and Anthropic-style shapes.

### Consolidate Request Completion Writes

Current direction:

- Runtime state, request JSONL, control trace, and UI snapshots all carry overlapping pieces.
- JSONL request reading/querying has been centralized in core `request_ledger`, but completion event construction is still broader than one internal event.

Target:

- A single internal `ObservedRequestCompleted` event is finalized once.
- State, log, trace, and API projections are derived from that event.
- Attempt chain, route decision, usage, cost, timing, service tier, and balance signals share the same `trace_id/request_id`.

Removal condition:

- Streaming and non-streaming paths produce equivalent completion events.
- Retry/failover tests assert the same DTO shape.
- GUI/TUI request detail pages read the same field names.

### Normalize TUI Render Invalidation

Current direction:

- Page switches clear the terminal.
- Resize events only set `should_redraw`.
- Some table state is synchronized inside page render functions.

Target:

- A small render invalidation model:
  - `NormalRedraw`
  - `FullClear`
  - `ResizeClear`
- Resize and page-switch force a full clear before drawing.
- Table highlight spacing is consistent across all stateful tables.
- Header/status/footer text has width-aware compaction.

Current status:

- The render loop uses explicit redraw vs full-clear invalidation, with full clear on page switch and terminal resize.
- Stations aligns with stateful table highlight spacing and synchronizes viewport offset from the visible row count before rendering.
- Header/status lines are display-width fitted, and page tabs degrade to compact numeric labels while keeping the selected page visible.

Removal condition:

- TUI smoke tests or snapshot-style tests cover resize/page switch layout invariants where practical.
- Manual verification includes narrow terminal width and Stations navigation.
- No repeated-row or stale-cell reports are reproducible in the known paths.

## Compatibility Rules

### Persisted Data

- Existing config files must still load.
- Existing request JSONL must still replay.
- New serialized fields require serde defaults.
- Deletions need either migration or compatibility readers.

### Runtime API

- Canonical v1 station/profile/session language stays.
- New request observability fields should be additive first.
- Compatibility aliases may exist, but UI should prefer canonical fields.

### UI

- GUI and TUI should not parse private log files when core can expose the data.
- UI may hide unavailable fields, but must not silently reinterpret them.
- "Unknown price" and "$0 cost" are different states.
- "Balance stale" and "balance empty" are different states.

## Safety Boundaries

### Failover

Default policy:

- Same-station retry before first output is safe.
- Cross-station failover before first output is allowed only when policy permits it.
- Cross-station failover after first output is disabled by default.

Reason:

- Codex session continuity and streaming behavior are more important than chasing every availability edge case.

### Balance

Balance/quota is a route-eligibility signal, not a health signal.

Examples:

- `401` invalid auth: hard operator fault and health negative.
- transport timeout: health negative.
- quota exhausted: route-ineligible until balance changes, but not a transport failure.
- balance API unavailable: stale/unknown, not necessarily route-ineligible.

### Pricing

Cost must carry confidence:

- `exact`
  - model price and usage fields are complete enough
- `estimated`
  - fallback price or partial usage is used
- `unknown`
  - no defensible price

Never present unknown cost as zero cost.

## Architecture Bias

Prefer these shapes:

- core owns semantics
- UI owns presentation
- logs are append-only facts
- projections can be rebuilt
- request trace is the join key
- model pricing is source-versioned
- balance adapters are explicit and sandboxed

Avoid these shapes:

- duplicated pricing math in every UI
- implicit string parsing for provider decisions
- treating request logs as the only live state source
- route policy hidden behind button labels
- provider-specific special cases leaking into page renderers

## Testing Bias

P0 tests should focus on:

- TUI redraw invalidation behavior where unit-testable
- usage extraction from representative response shapes
- JSONL compatibility with old request records
- streaming/non-streaming request completion equivalence
- route chain attempt recording

P1 tests should focus on:

- cost calculation precision
- cache-aware billing
- price source fallback
- balance adapter parsing
- quota-exhausted route eligibility
- GUI/TUI DTO rendering with missing/unknown fields

## Commit Discipline

This workstream should land in thin, reviewable slices:

- TUI render hygiene
- usage schema v2
- request event/trace unification
- pricing engine
- balance adapters
- GUI request detail upgrade
- policy UX

Each slice should leave the product in a runnable state.

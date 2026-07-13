---
title: Remote Quota Pace and BaseLLM Pricing - Plan
type: feat
date: 2026-07-12
deepened: 2026-07-12
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---
# Remote Quota Pace and BaseLLM Pricing - Plan

> **As-built reconciliation (2026-07-13):** This feature is implemented on the canonical relay
> runtime. `RuntimeStore` in `~/.codex-helper/state/state.sqlite` is the only durable quota,
> BaseLLM, request, policy, and affinity authority. Quota state and BaseLLM LKG/attempt state are
> versioned SQLite documents; request accounting hydrates only from committed terminals.
> `requests.jsonl` and `control_trace.jsonl` are bounded post-commit debug sinks and are never
> replayed into production state. `file_replace` remains valid only for operator-owned
> configuration and manual pricing overrides, never as a runtime-state backend.

## Goal Capsule

| Field | Value |
|---|---|
| Objective | Make the TUI answer how much shared remote quota was consumed, whether the current 15/60-minute pace will exhaust it before its real reset, and how much locally observed usage belongs to each project, while making BaseLLM-backed cost estimates tier-aware and automatically refreshed. |
| Authority | Remote quota-pool counters are authoritative for shared total burn. The local request ledger is authoritative only for local request and project attribution. Manual pricing overrides outrank validated remote prices, and no inferred identity may be presented as an exact shared pool. |
| Execution profile | Deep cross-cutting feature work across canonical RuntimeStore persistence, provider usage adapters, pricing, request-ledger provenance, proxy lifecycle sampling, core analytics DTOs, attached read models, TUI rendering, CLI status, tests, and bilingual documentation. |
| Stop conditions | Stop if an implementation scales local request prices from shared remote burn, distributes external usage across local projects, persists raw credentials, sums pools whose shared identity is unproven, overwrites a valid cache with an invalid candidate, or labels a rolling/reset-unknown window as a calendar day. |
| Tail ownership | Land dependency-ordered units with focused nextest gates, preserve bounded debug JSONL and attached read-only behavior, remove superseded persistence and cache-writing paths, and finish with workspace formatting, lint, and test gates. |

---

## Product Contract

### Summary

codex-helper should treat remote relay counters and local request records as two complementary products.
The remote side should report shared quota-pool burn, 15/60-minute rates, reset-aware pace, remaining balance, and exhaustion ETA even when another computer uses the same key.
The local side should report estimated cost by Git project and preserve an explicit external/unattributed remainder instead of pretending every remote dollar came through this proxy.

Cost estimates should load a validated BaseLLM last-known-good catalog automatically, select the canonical provider for the active service, honor context tiers, and keep `pricing_overrides.toml` as the highest-priority manual layer.
The relay's remote billed counters remain the financial source of truth; BaseLLM remains a third-party estimate catalog.

### Problem Frame

The current Page 5 Usage view already provides a local-day `UsageDayView`, 24 hourly buckets, and provider/endpoint/model/session/project summaries.
That earlier work should remain the local analytics baseline rather than being planned again.
Its project key is still the raw `cwd`, its project list is a short side summary, and its fourth KPI block is Retry Gate rather than a user-facing quota pace answer.

Remote balance adapters already expose `today_used_usd`, cumulative usage, remaining quota, limits, and reset timestamps for common relays.
Their history is grouped by provider endpoint in a process-local 64-point deque, refresh is mainly startup/request/manual driven, and duplicate endpoint observations can represent the same remote pool.
This cannot observe another computer during local idle time, survive a restart, or justify an exact combined total.

The current pricing importer fetches BaseLLM only on an explicit CLI command, flattens every provider by bare model ID, imports base prices only, and writes the result into the manual override file.
The background BaseLLM metadata job fetches the same JSON with validators but does not update pricing.
For `gpt-5.6-sol`, this misses the strict `> 272000` context tier and underprices the affected requests.

### Actors

- A1. A local operator uses the interactive or attached TUI to monitor daily package consumption and decide whether the current pace is sustainable.
- A2. The proxy daemon owns remote sampling, persistence, cost capture, project attribution, and the snapshot contract.
- A3. An attached TUI reads the daemon-owned version 5 operator bundle through read-only control-plane requests, never starts local persistence or polling work, and never inspects or patches local Codex configuration.
- A4. A maintainer uses pricing CLI commands to inspect state, acquire the exclusive writer lease for an explicit refresh, or promote selected remote rows into manual overrides.

### Requirements

**Remote quota pools and sampling**

- R1. Remote key, wallet, or subscription counters must be the total-burn source for their declared scope, including consumption produced by other computers or keys when the scope says they share the pool.
- R2. Every remote observation must carry a secret-free pool identity, scope, source, counter semantics, capability set, identity confidence, and aggregation eligibility before snapshots are deduplicated or summed.
- R3. Pool identity precedence must be remote stable pool ID, then explicit `quota_pool_id`, then endpoint origin plus scope plus a keyed local credential fingerprint; ambiguous identities must stay separate and must not produce a trusted combined total.
- R4. A single proxy-runtime-owned sampler must refresh supported pools while local request traffic and the TUI are idle, reuse provider throttling/suppression, honor shutdown, and avoid duplicate samplers in attached clients.
- R5. A schema-versioned RuntimeStore document must retain the current period anchor, recent dense samples, last successful observation, adjustment/epoch markers, and enough cross-restart state for 15/60-minute analysis within the canonical SQLite authority.
- R6. Reset, top-up, refund, counter rollback, plan/limit change, out-of-order response, stale carry-forward, and settlement delay must segment or invalidate a rate window rather than become negative or fabricated spend.
- R7. New API quota-unit conversion must prefer the relay's per-origin `quota_per_unit`, then an explicit configured divisor, and otherwise expose raw/estimated units with reduced confidence rather than silently claiming the default divisor is exact USD.

**Rate, pacing, and attribution**

- R8. Core analytics may display a fresh authoritative remote daily/window total from one valid observation. It must calculate 15-minute and 60-minute burn rates, required rate until the explicit reset, pace ratio, exhaustion ETA, and projected reset balance only from enough fresh samples in one continuous epoch; low-sample state must suppress those derived values without hiding the direct total.
- R9. Calendar day, rolling 24-hour, custom subscription window, and reset-less wallet semantics must remain distinct; the UI may say "today" or "midnight" only when the source contract proves that meaning.
- R10. Local project attribution must normalize new requests to a Git root when possible, retain a canonical path fallback and unknown bucket, and intersect requests with their provider endpoint/pool instead of comparing every local request with every remote pool.
- R11. Reconciliation must expose local priced projects, local unknown project, external/unattributed positive remainder, and a signed negative reconciliation gap; remote deltas must never multiply local request prices or be proportionally assigned to projects.
- R12. When the remote and local windows, pool identity, pricing coverage, or committed-terminal coverage do not align, the snapshot must lower confidence and show the observed start/coverage rather than invent a full-day allocation.

**Pricing and request cost provenance**

- R13. BaseLLM imports must remain provider-namespaced and choose a deterministic canonical provider for the active service, with `codex -> openai` as the first required mapping; bare-model deduplication across unrelated providers is not allowed.
- R14. `ModelPrice`, remote cache rows, local override rows, snapshots, and cost calculation must support context tiers as per-field overlays on the base price.
- R15. A context tier must use billable ordinary input plus cache-read input for threshold selection, use a strict greater-than comparison, apply the selected tier to the whole request, and apply service/provider multipliers only after component costs.
- R16. Automatic BaseLLM synchronization must use conditional HTTP requests, hard response limits, semantic validation, last-known-good promotion, source metadata, bounded retry/backoff, and failure retention without writing `pricing_overrides.toml`.
- R17. Effective catalog precedence must be `bundled < validated remote LKG < local manual override`; a manual row replaces the whole model, including remote tiers, and its effective shadowing must be visible in status output.
- R18. Each committed request terminal must preserve its calculated cost, selected tier, effective pricing source/generation, stable project root, and provider endpoint/pool attribution key so RuntimeStore hydration never silently reprices it.

**TUI, operator read model, and operations**

- R19. Core must expose a bounded quota-pool analytics DTO beside `UsageDayView`; the TUI must not independently deduplicate pools, calculate slopes, infer resets, or reconcile projects.
- R20. Page 5 must make pool consumption and pace first-viewport signals, provide scrollable pool and project views, preserve local-day provider/endpoint/model/session context, and cover fresh, syncing, stale, offline, ambiguous, unlimited, exhausted, just-reset, and low-sample states.
- R21. Local and attached TUI modes must consume the same version 5 operator read model; attached mode remains observer-only, does not expose or execute the integrated TUI's `n` / `o` local Codex switch, and treats unavailable or stale bundles explicitly rather than synthesizing zero values.
- R22. No raw key, bearer token, console credential, full credential hash, or unsanitized provider error may appear in persisted samples, admin snapshots, TUI output, reports, or tests.
- R23. CLI status, configuration docs, README, and changelog must explain remote source/scope, sampling age, price source/generation, tier behavior, override precedence, coverage limits, and the difference between remote billed usage and local estimated attribution.

### Key Flows

- F1. On daemon startup, open and migrate RuntimeStore, load the quota and BaseLLM documents, publish effective pricing, and only then bind listeners and start one shutdown-aware price sync and provider sampler. Missing state starts empty/bundled; an invalid quota document blocks publication, while corrupt BaseLLM state falls back to bundled pricing and future document schemas remain unmodified.
- F2. On the Usage page, select a quota pool and read its remote settled/observed usage, remaining quota, 15/60-minute rate, required rate, reset, pace, ETA, freshness, source, scope, and confidence. Low-sample or stale pools terminate in an explicit unavailable/frozen prediction state.
- F3. Switch to Projects to inspect all locally observed Git-root rows matched to the selected pool, followed by local unknown, external/unattributed, and signed reconciliation status for the same aligned window.
- F4. During BaseLLM sync, send stored weak ETag and Last-Modified validators, promote a valid canonical-provider catalog on 200, retain body and validators on 304, and retain the prior LKG on any transport, HTTP, body, schema, semantic, sanity, or SQLite commit failure. LKG and attempt state are one RuntimeStore document committed with revision compare-and-swap; a stale response adopts the current committed document instead of overwriting it.
- F5. In attached mode, fetch one canonical operator bundle with read-only control-plane requests, render the same quota and routing DTOs as local mode, and never start a sampler, open a writer, inspect or patch local Codex configuration, or infer missing remote facts.

### Acceptance Examples

- AE1. Given two computers consume the same key and the remote pool burns `$100` while this daemon records `$60` of aligned priced requests, the pool shows `$100` total, `$60` local projects, and `$40` external/unattributed without changing any local request price.
- AE2. Given two provider-endpoint observations carry the same authoritative pool key, they collapse to one pool; given two keys might share a wallet but have no authoritative or explicit shared ID, they remain separate and no trusted grand total is shown.
- AE3. Given sampling first starts at 10:00 with no remote today field or retained reset anchor, the UI says "observed since 10:00" and does not backfill midnight-to-10:00 usage.
- AE4. Given a counter resets near its declared boundary, the next valid observation opens a new epoch; given the same decrease occurs away from a boundary, the interval is marked adjustment/inconsistent and excluded from the rate.
- AE5. Given exactly 272,000 ordinary-plus-cache-read input tokens, base prices apply; given 272,001, the selected context tier prices the entire request and no second long-context multiplier is applied.
- AE6. Given a valid BaseLLM LKG and a later 304 without validators, the body and previous validators remain unchanged while `last_checked_at` advances; given malformed 200 or 500, the old LKG remains effective.
- AE7. Given BaseLLM has the same model ID under `openai` and `routing-run` with different prices, Codex selects the namespaced `openai` row and status exposes that provenance.
- AE8. Given a manual override supplies base prices without tiers for a model that has remote tiers, the manual whole-model row wins, the remote tiers do not leak through, and status reports that the manual row shadows them.
- AE9. Given requests originate from two subdirectories of one Git repository, the project table combines them under the Git root; missing or deleted paths remain in explicit fallback/unknown coverage.
- AE10. Given an attached operator bundle is unavailable or stale, the TUI preserves only facts from a valid retained bundle, disables runtime actions, and never renders missing quota analytics as `$0`.
- AE11. Given the supplied 8,405-row `gpt-5.6-sol` audit data, the tier-aware estimator reproduces `$501.493510` for 2026-07-11, `$500.108175` for 2026-07-12, and `$1,001.601685` total with zero row residual.

### Scope Boundaries

In scope:

- Shared-pool analytics for the existing common relay balance/usage adapter family.
- Secret-free pool identity, capability discovery, New API unit conversion, and explicit operator pool IDs.
- Daemon-owned RuntimeStore documents for bounded quota samples and last-known-good BaseLLM pricing.
- Context-tier pricing, canonical provider selection, automatic BaseLLM refresh, and deterministic manual override precedence.
- Stable request cost/project/pool provenance in committed RuntimeStore terminal payloads.
- Core dashboard snapshot, local and attached TUI behavior, TUI report/export, CLI pricing status/refresh, focused tests, and bilingual documentation.

Out of scope:

- An unbounded long-range analytics warehouse or rotated debug-log backfill guarantees.
- Tauri desktop redesign or other GUI surface work.
- Arbitrary new relay protocols, generic user-console JWT login flows, or administrator-only upstream-account capacity APIs.
- Automatic changes to routing, provider accounts, keys, subscriptions, or third-party dashboards.
- Exact invoice claims for BaseLLM-derived local costs or exact project attribution for usage not observed by this daemon.
- Cross-machine event ingestion beyond what the remote pool counter already includes.

---

## Planning Contract

### Key Technical Decisions

- KTD1. Keep local-day and remote-period analytics as separate core contracts. `UsageDayView` continues to describe locally observed calendar-day requests; a new quota-pool view owns remote scope, epoch, rate, reset, prediction, confidence, and aligned reconciliation.
- KTD2. Pool identity is evidence-ranked, issuer-namespaced, and revisioned. A remote ID is scoped by origin/issuer plus remote scope before it may merge endpoints; explicit `quota_pool_id` is next; a domain-separated credential digest keyed by a persistent per-install identity key may merge repeated local views of the same key. Every request always captures `ProviderEndpointKey` and captures a pool-membership revision only when evidence already exists. Identity upgrades or credential/key rotation start a new revision and never retroactively merge overlapping history.
- KTD3. `ProxyRuntime` owns both long-lived background tracks. Runtime construction loads valid price/quota state before serving operator bundles; `ProxyRuntime::start` starts exactly one BaseLLM sync task and one quota sampler, retains their shutdown-aware handles, and joins them on shutdown. Shutdown prevents another cycle but waits for an already-started BaseLLM fetch and SQLite compare-and-swap to finish; no detached blocking write may outlive `wait` or `abort_and_wait`. CLI and server entry points do not spawn ad hoc background refresh tasks; explicit `force-refresh` is a foreground writer-lease operation, and attached TUI processes remain read-only without a local Codex-switch exception.
- KTD4. `QuotaPoolRegistry` is the only in-memory quota state machine and writer. All startup, request-triggered, manual, and scheduled balance results converge through one serialized observation path. An accepted policy observation and its quota-registry document commit in the same SQLite immediate transaction; either both advance or neither does. A stale or inactive-incarnation disposition remains in observation history but must not advance active quota membership, balance, suppression, or adapter hints. After a successful commit, policy, quota, and balance projections publish together from locks acquired before the transaction, with no cancellation point between durable commit and in-memory publication. Error and unknown carry-forward results update attempt state without appending an amount point.
- KTD5. A rate epoch is defined by a complete normalization signature: pool identity revision, counter kind, quantity unit and conversion generation, scope, window/reset semantics, limit/plan identity, and continuity markers. Any signature change opens a new epoch. Rate math then uses at least three fresh points and positive adjacent deltas within one epoch; 60 minutes drives ETA/reset projection, 15 minutes shows acceleration, and insufficient span, stale data, gaps, or adjustments produce unavailable rather than extrapolation.
- KTD6. Quota quantity and reconciliation are type-safe. Raw quota units remain a unit-tagged fixed-point quantity and cannot enter USD arithmetic until a sourced conversion produces a USD normalization generation. Eligible USD reconciliation computes one checked `SignedUsdDelta` in femto-USD, derives nonnegative `external_unattributed`, and retains a negative inconsistency gap; `UsdAmount` remains nonnegative and is never reused for signed values. Incompatible units or incomplete windows show the two sides separately with reconciliation unavailable.
- KTD7. Runtime persistence and operator-file persistence are separate contracts. RuntimeStore owns one SQLite writer lease and document-revision compare-and-swap for quota and BaseLLM state. The shared atomic file helper is limited to operator configuration and manual override files; it is not a second runtime-state backend.
- KTD8. BaseLLM and overrides remain keyed by `(canonical_provider, normalized_model)`. Runtime lookup overlays only the active service's canonical provider, explicit CLI import is provider-filtered, and a versioned manual schema preserves legacy Codex/OpenAI rows without allowing aliases or identical model IDs to collide across providers.
- KTD9. Runtime pricing is one immutable `EffectivePricingCatalogSnapshot`, not merely a remote-cache generation. Its content revision hashes the normalized bundled catalog, canonical mapping, validated remote LKG, and manual overrides. A coordinated refresh validates outside SQLite, then compares the captured RuntimeStore document revision and atomically commits LKG plus attempt state; 304 may update check metadata only when its validators still describe current content and never changes content revision. Manual-file changes rebuild the effective snapshot through bounded metadata/hash detection rather than per-request disk parsing.
- KTD10. Context tiers are ordered per-field overlays. Select the greatest known context threshold strictly below the request's ordinary-input-plus-cache-read count, overlay its supplied component prices on the base row, and charge nonzero token categories only when the effective price exists. Unknown tier types are skipped with warnings; malformed known context tiers reject the candidate rather than silently reverting to base pricing.
- KTD11. Request completion creates one immutable accounting record. It fixes `ended_at`, captured `CostBreakdown`, effective catalog revision, project identity, endpoint, and optional pool-membership revision once, commits that terminal payload to RuntimeStore, then publishes live projections. A bounded sparse time-by-endpoint/pool-by-project attribution index hydrates from committed terminals and exposes retention, count, dedupe, boundary, price, and unmatched coverage without JSONL replay state.
- KTD12. The version 5 operator bundle is the sole local/attached read contract. It carries explicit quota capabilities and freshness, and attached clients are limited to GET/HEAD observer operations with no local client-config exception; Codex switching remains a separate explicit CLI action or an integrated local TUI action, and no compatibility adapter recreates removed routing or persistence concepts.

### High-Level Technical Design

```mermaid
flowchart LR
  subgraph Pricing[Pricing track]
    BaseLLM[BaseLLM all.json] --> Conditional[Conditional fetch + validation]
    Conditional --> PriceDoc[RuntimeStore BaseLLM LKG + attempt document]
    Bundled[Bundled catalog] --> Merge[Service-aware catalog merge]
    PriceDoc --> Merge
    Manual[Manual overrides] --> Merge
    Merge --> Effective[Immutable effective catalog revision]
    Effective --> Estimator[Tier-aware estimator]
  end

  subgraph Remote[Remote quota track]
    Adapter[Existing usage adapters] --> Record[Unified balance-recording entry]
    Record --> Registry[Single-writer pool registry]
    Registry --> SampleStore[RuntimeStore quota document]
    SampleStore --> PoolAnalytics[Epoch + 15m/60m + pace + ETA]
  end

  subgraph Local[Local attribution track]
    Request[Finished request] --> Estimator
    Estimator --> Completion[Immutable accounting record]
    Completion --> Ledger[Committed SQLite terminal]
    Ledger --> Attribution[Bounded retained-terminal projection]
    Ledger -. post-commit only .-> Debug[Bounded requests/control trace JSONL]
  end

  PoolAnalytics --> Reconcile[Aligned signed reconciliation]
  Attribution --> Reconcile
  Reconcile --> Snapshot[DashboardSnapshot quota analytics]
  Snapshot --> LocalTUI[Local TUI]
  Snapshot --> AttachedTUI[Attached TUI]
```

The two tracks intentionally meet only after both have explicit time and identity coverage.
Remote counters answer total pool burn.
Captured local request facts answer project attribution.
The core reconciliation builder produces the only DTO consumed by the TUI.

### Alternatives Rejected

| Alternative | Decision |
|---|---|
| Standalone quota checkpoint and BaseLLM LKG/attempt JSON files | Rejected. They create a second persistence authority, require independent locking/recovery rules, and can diverge from request and provider facts committed in `state.sqlite`. |
| Minimal accounting JSONL as a durable ledger with startup replay | Rejected. Rotation, append failure, and replay bounds cannot provide canonical accounting or attribution coverage. JSONL remains debug output only. |
| One RuntimeStore writer lease plus versioned documents and committed terminals | Chosen. SQLite transactions define publication order, document revisions provide BaseLLM compare-and-swap, and read-only consumers use `RuntimeStoreReader` or the operator read model. |

### Sequencing

1. Extend RuntimeStore with the schema-revision-two document/private-key tables and preserve the single-writer lease.
2. Build provider-aware tiered pricing while quota identity/store work starts independently.
3. Add the validated BaseLLM LKG and deterministic three-layer catalog merge.
4. Add the revisioned pool identity registry, typed quota quantities, and the quota RuntimeStore document.
5. Capture stable cost, project root, endpoint, and optional pool-membership facts in new request records and build the bounded attribution index.
6. Start one shutdown-aware daemon sampler through the existing refresh/suppression path.
7. Replace shared-balance calibration with pool burn, reset-aware pacing, and aligned project reconciliation.
8. Add the snapshot capability contract and redesign Page 5 around quota pools plus full project ranking.
9. Finish CLI status/force refresh, docs, cleanup, and full verification.

### System-Wide Impact

- RuntimeStore schema revision two adds `runtime_documents` for the quota registry and the combined BaseLLM LKG/attempt payload, plus `runtime_private_keys` for the installation-local quota identity. All rows remain scoped to the validated store identity in `state.sqlite`.
- `ProviderBalanceSnapshot` gains pool scope/capability/identity evidence, while routing-facing balance behavior remains unchanged.
- `ProxyRuntime` gains ownership of long-lived sampling and price-sync tasks plus their shutdown handles. Server and interactive modes inherit the same lifecycle; attached clients and state-only test construction start neither task.
- `FinishedRequest` accounting facts are frozen into the committed logical-request terminal. Startup projections query those terminals through RuntimeStore; bounded debug logging occurs only after commit and cannot affect accounting, attribution, or recovery.
- Price lookup becomes service-aware and reads one immutable effective catalog revision instead of rebuilding or reparsing disk state per request. Pricing and model-compatibility consumers switch generations together.
- The former shared-balance multiplier path is removed; reset and pacing behavior lives in `quota_analytics.rs`, and dead calibration DTOs and fields are removed.
- Page 5 changes interaction state because pools and projects become first-class selectable tables. Existing provider/endpoint/local-day analytics remain reachable, and the separate Routing page uses provider terminology.
- Quota DTOs gain unit-tagged quantities, conversion generation, and a separate signed USD delta. New API dollar display may change when a relay advertises a non-default `quota_per_unit`; conversion changes open a new epoch and the UI exposes source/confidence instead of joining incompatible generations.

### Risks & Dependencies

| Risk | Mitigation |
|---|---|
| Remote caches settle in steps or lag by several minutes. | Require multiple valid points, expose observation age/span, prefer the adapter's more responsive counter, and freeze forecasts when stale. |
| Duplicate endpoint views inflate a shared pool. | Deduplicate only with evidence-ranked pool identity and make ambiguous pools ineligible for a trusted aggregate. |
| A reset, refund, or top-up contaminates burn rate. | Create explicit epochs/adjustments and calculate rates only inside an uninterrupted segment. |
| A corrupt or partial runtime write destroys the last valid state. | Commit RuntimeStore mutations in SQLite transactions, validate the exact schema manifest on open, and test injected migration/document failures and rollback. |
| A complete but stale BaseLLM response overwrites a newer catalog. | Capture the document revision before validation and use RuntimeStore compare-and-write; a stale commit adopts the currently committed LKG/attempt document. |
| A future state or document schema is opened by this binary. | Reject an unsupported RuntimeStore schema before mutation; treat a future BaseLLM document as read-only and never replace it with an empty/default payload. |
| BaseLLM schema or upstream provider set changes. | Parse defensively, retain provider namespace and provenance, validate canonical provider/model/tier counts, and keep the prior LKG on suspicious 200 responses. |
| Automatic or manual prices make historical project costs jump during the day. | Capture cost and the full effective catalog revision in new request records, reload manual changes into a new immutable snapshot, and display mixed/reconstructed coverage instead of repricing captured rows. |
| Continuous polling hammers a relay or amplifies 429s. | Preserve the two-minute hard floor, use a five-minute analytics default unless explicitly slower/faster within safeguards, honor `Retry-After`, jitter schedules, and reuse terminal suppression. |
| Project-to-pool matching is incomplete for retained terminals without captured membership. | Keep such requests endpoint-only and expose unmatched/price coverage rather than forcing attribution or rewriting history. |
| A request is published live before its durable terminal commits. | Commit the terminal first, publish projections second, and treat post-commit debug-log failure as observability loss only. Hydration uses keyset-paginated committed terminals with retention/count/dedupe/boundary coverage. |
| Raw quota units or a changed divisor create a false USD slope or reconciliation. | Persist original unit and conversion generation, split epochs on normalization changes, and disable USD reconciliation unless both sides share a compatible generation. |
| TUI density hides data or overlaps on small terminals. | Use contextual KPI rows and focusable full tables, shorten labels without losing state, and retain 128x26 plus 76x22 render tests for every state class. |
| Attached state is stale or unavailable. | Fetch one version 5 operator bundle, retain only explicitly valid stale facts, disable runtime actions, and never infer missing values from defaults. |
| Secret-derived identity leaks credentials. | Use a per-install keyed one-way fingerprint, persist only a short opaque pool key, sanitize errors, and add negative serialization tests. |
| The installation identity key is lost, corrupted, or copied between installations. | Create it once in the protected RuntimeStore private-key table, never log or expose it through operator DTOs, and treat store replacement as a new installation identity without merging old history. |

### Sources & Research

- Load-bearing internal research: `docs/research/2026-07-12-upstream-usage-balance-apis.md` defines remote scopes, New API/Sub2API endpoints, shared-device coverage, reset semantics, and sampling limits.
- Load-bearing billing research: `docs/research/2026-07-12-sub2api-billing-and-basellm-pricing.md` proves cache normalization, the strict 272,000 boundary, BaseLLM source/provenance, and the supplied CSV totals.
- Implemented baseline: `docs/plans/2026-07-07-002-refactor-tui-usage-day-panel-plan.md`, `crates/core/src/state/runtime_types.rs`, and `crates/tui/src/tui/view/stats.rs` show that local-day analytics and Page 5 already exist.
- As-built remote path: `crates/core/src/usage_providers.rs`, `crates/core/src/balance.rs`, `crates/core/src/state.rs`, `crates/core/src/quota_analytics.rs`, and `crates/core/src/dashboard_core/operator_summary.rs` show adapter parsing, canonical quota-document persistence, analytics, and operator read delivery.
- As-built pricing path: `crates/core/src/pricing.rs`, `crates/core/src/basellm_catalog.rs`, `crates/core/src/runtime_host.rs`, and `src/commands/pricing.rs` show provider-tier projection, RuntimeStore LKG/attempt CAS, resident background sync, explicit writer refresh, and manual override precedence.
- Local attribution path: `crates/core/src/sessions.rs`, `crates/core/src/runtime_store.rs`, `crates/core/src/state/attribution_index.rs`, and `crates/core/src/proxy/request_observer.rs` provide Git-root inference, committed-terminal hydration, bounded projections, and the request-completion boundary. `crates/core/src/logging.rs` is post-commit debug output only.
- Persistence patterns: `crates/core/src/runtime_store.rs`, `crates/core/src/runtime_store/metadata.rs`, and `crates/core/src/runtime_store/lifecycle.rs` define the canonical writer lease, schema migration, documents, private key, committed terminals, and read-only query surface. `file_replace.rs` is limited to operator-owned configuration and manual overrides.
- Load-bearing upstream contracts: `repo-ref/new-api/controller/token.go`, `repo-ref/new-api/model/token.go`, `repo-ref/new-api/controller/misc.go`, `repo-ref/sub2api/backend/internal/handler/gateway_handler.go`, and `repo-ref/sub2api/backend/internal/service/gateway_usage_billing.go` establish remote counter scope, update order, and quota-unit/reset behavior.
- UI and sync prior art: `repo-ref/aio-coding-hub/src/components/home/HomeOAuthQuotaPanel.tsx`, `repo-ref/aio-coding-hub/src/components/usage/UsageLeaderboardTable.tsx`, `repo-ref/aio-coding-hub/src/pages/UsagePage.tsx`, and `repo-ref/aio-coding-hub/src-tauri/src/infra/model_prices_sync.rs` support stale-data display, leaderboard share, canonical provider mapping, and conditional price sync.
- External contracts: BaseLLM `https://basellm.github.io/llm-metadata/api/all.json`, RFC 9110/9111 conditional request semantics, reqwest 0.13.4 timeout/stream behavior, and serde default/unknown-field behavior. These are load-bearing for U2, U3, and U8.

---

## Implementation Units

### U1. Establish the Canonical RuntimeStore Persistence Boundary

- **Goal:** Make `state.sqlite` the only runtime-state authority while keeping operator configuration and manual pricing overrides as a separate file-owned contract.
- **Requirements:** R5, R16, R18, R22; supports F1 and F4.
- **Dependencies:** None.
- **Files:**
  - `crates/core/src/runtime_store.rs`
  - `crates/core/src/runtime_store/metadata.rs`
  - `crates/core/src/runtime_store/lifecycle.rs`
  - `crates/core/src/state.rs`
  - `crates/core/src/file_replace.rs`
  - `crates/core/Cargo.toml`
  - `Cargo.lock`
- **Approach:** Migrate the exact RuntimeStore schema from revision one to revision two in one immediate SQLite transaction. Add a store-scoped private-key row for quota identity and one versioned document table with fixed quota-registry and BaseLLM-catalog kinds. Preserve the existing exclusive writer lease, WAL durability settings, exact schema validation, database-identity checks, read-only reader, and committed terminal ledger. Runtime documents validate JSON and schema versions; BaseLLM uses document-revision compare-and-write, while quota ingestion is serialized by the runtime-owned registry. Keep atomic file replacement only for operator configuration and `pricing_overrides.toml`; it must not persist runtime facts.
- **Patterns to follow:** Use `RuntimeStore::write_store_transaction` as the durable publication boundary, bind every row to `store_id`, reject unknown database revisions before mutation, and expose only typed reader/query methods. Debug JSONL output follows a successful terminal commit and never participates in recovery.
- **Test scenarios:**
  - A revision-one store migrates atomically to revision two without changing `store_id`; an injected migration failure rolls back both tables and both schema revision markers.
  - The exact schema manifest includes the private-key and document tables, and a reader rejects revision one until the writer completes migration.
  - A second writer fails the lease while readers continue querying committed facts; replacing the database file is detected.
  - Runtime document batches are atomic, reject duplicate kinds/invalid JSON, increment revisions, and return stale on a mismatched compare-and-write revision.
  - Quota identity remains stable across reopen, differs across stores, is redacted from debug output, and is never serialized into operator DTOs.
  - `requests.jsonl` and `control_trace.jsonl` rotation or append failure cannot change committed request, quota, attribution, or pricing state.
- **Verification:** RuntimeStore migration, writer-lease, schema-manifest, document CAS, private-key, reader, and committed-terminal tests prove one durable authority. File replacement tests cover only the remaining operator-owned files.

### U2. Add Provider-Aware Context-Tier Pricing

- **Goal:** Make the shared price schema and estimator represent deterministic provider provenance and BaseLLM context tiers end to end.
- **Requirements:** R13, R14, R15, R17, R18; covers AE5, AE7, AE8, and AE11.
- **Dependencies:** None.
- **Files:**
  - `crates/core/src/pricing.rs`
  - `src/cli_types.rs`
  - `src/commands/pricing.rs`
  - `crates/core/tests/pricing_tier_regression.rs`
  - `crates/core/tests/fixtures/pricing/gpt-5.6-sol-audit.json`
- **Approach:** Change catalog identity to `(canonical_provider, normalized_model)` before adding provider/source namespace and ordered context-tier overlays to `ModelPrice`, `ModelPriceView`, `LocalModelPriceOverride`, snapshots, validation, and explicit BaseLLM import. Use a versioned nested provider map or equivalently unambiguous serialized key while reading legacy `[models.<id>]` rows as Codex/OpenAI and preserving round-trip compatibility. Make price lookup service-aware before model/alias lookup, and default explicit Codex BaseLLM import to `openai` unless the operator names another provider. Centralize threshold selection after `BillableTokenUsage` normalization and before component charging; record the matched tier and effective source in `CostBreakdown`. Keep manual rows whole-model replacements so missing manual tiers intentionally shadow only the corresponding provider row.
- **Patterns to follow:** Preserve fixed-point `UsdAmount`, canonical usage buckets, captured provider-price epochs, and the single convention-aware cost calculation boundary. Follow the canonical provider mapping in `repo-ref/aio-coding-hub/src-tauri/src/infra/model_prices_sync.rs` without copying its database layer.
- **Test scenarios:**
  - `gpt-5.6-sol` at 272,000 input-side tokens uses `$5/$30/$0.5/$6.25`; 272,001 uses `$10/$45/$1/$12.5` for the entire request.
  - Cache-heavy input counts once toward the threshold and once in component billing; cache creation uses the tier overlay when present.
  - Unsorted tiers select the greatest satisfied threshold; duplicate or malformed known-context thresholds fail validation; an unknown tier type produces a warning and does not affect context pricing.
  - A partial valid tier overlays only supplied fields, and a nonzero component with no effective price yields partial/unpriced rather than free.
  - Service/provider multipliers apply once after tiered component totals.
  - Same model IDs under `openai` and `routing-run` remain distinct and Codex chooses `openai`.
  - A legacy manual TOML fixture loads as Codex/OpenAI, round-trips without semantic loss, and aliases collide only within one provider namespace.
  - A manual row shadows only the same provider/model; an identically named model under another provider remains available.
  - A privacy-scrubbed fixture with only timestamp/day, token components, multiplier, and expected cost reproduces the supplied 8,405-row daily and total audit values exactly.
- **Verification:** Core price tests prove boundaries, overlay behavior, provider selection, fixed-point totals, and the CSV regression; root CLI tests prove deterministic provider filtering and tier-preserving manual import.

### U3. Promote BaseLLM Metadata Sync into a Validated Catalog LKG

- **Goal:** Load and refresh a compact provider-namespaced BaseLLM price catalog automatically without mutating manual overrides or losing a valid prior cache.
- **Requirements:** R13, R16, R17, R23; covers AE6, AE7, and AE8.
- **Dependencies:** U1, U2.
- **Files:**
  - `crates/core/src/basellm_metadata.rs`
  - `crates/core/src/basellm_catalog.rs`
  - `crates/core/src/pricing.rs`
  - `crates/core/src/proxy/models_compat.rs`
  - `crates/core/src/runtime_host.rs`
  - `crates/core/src/lib.rs`
  - `src/cli_app.rs`
  - `src/cli_types.rs`
  - `src/commands/pricing.rs`
- **Approach:** Refactor the metadata-only downloader into a coordinated BaseLLM catalog sync that projects metadata and provider-namespaced prices from one bounded response, then migrate `models_compat` and pricing lookup to the same immutable in-process catalog snapshot. Load the remote LKG/attempt document before serving requests, merge `bundled < remote LKG < manual`, and identify the effective catalog by normalized content revision rather than fetch count. Detect manual-file metadata/hash changes with bounded polling plus explicit in-process invalidation, rebuild once, and atomically swap the shared snapshot; each request retains one `Arc` generation through calculation. Store source URL, weak ETag, Last-Modified, fetch/check/validate timestamps, schema generation, content hash, provider/model/tier counts, warnings, and the last attempt in one RuntimeStore document. Use explicit connect/read/total timeouts, same-origin HTTPS redirect policy for the built-in source, streaming decompressed-byte limits, `Retry-After`, and bounded exponential backoff with jitter. Validate outside SQLite, then compare the captured document revision during commit; stale 200/304 results adopt the current document instead of overwriting or retagging it. A 304 retains body and validators and leaves content revision unchanged. Missing/corrupt state may fall back, while a future document schema is reported read-only and never overwritten.
- **Patterns to follow:** Reuse the current BaseLLM parser's tolerant metadata fields, reqwest streaming already enabled in `crates/core/Cargo.toml`, and U1's RuntimeStore document CAS. Preserve the CLI's explicit manual-import path as a separate operator action; automatic refresh never calls it or writes `pricing_overrides.toml`.
- **Test scenarios:**
  - Valid 200 promotes a complete canonical-provider catalog; a second request sends weak ETag plus Last-Modified and a validator-free 304 changes only the check timestamp.
  - DNS/connect/read/total timeout, 404/429/500/206, cross-origin redirect, early disconnect, invalid UTF-8/JSON, chunked or decompressed-body overflow, missing `openai.models`, invalid decimal/tier, suspicious model-count collapse, or SQLite commit failure leaves the prior LKG and effective catalog unchanged.
  - LKG and last-attempt metadata commit as one document revision; no reader can observe validators/check state that belongs to different content.
  - A 304 with no valid local body retries unconditionally rather than accepting an empty catalog.
  - Interleaved old-200/new-200 and old-304/new-200 responses cannot let stale validators, check metadata, or content overwrite the newer document revision.
  - A corrupt manual override falls back to bundled plus remote LKG, while a valid manual row still wins.
  - A manual price change becomes a new effective content revision without restart; 304 changes only check metadata; a request concurrent with catalog swap observes exactly one revision.
  - Pricing lookup and `models_compat` switch to the same generation, and neither reparses BaseLLM state per request/model-list conversion.
  - Loading a future unsupported LKG document schema reports read-only state and leaves the committed payload untouched after refresh attempts.
  - `pricing status` reports generation, age, provider/model/tier counts, warnings, last error category, and manual-shadowed rows without exposing secrets.
- **Verification:** Hermetic local HTTP fixture tests cover the fetch state machine and hard limits; catalog tests prove three-layer precedence and RuntimeStore document publication.

### U5. Introduce Quota Pool Identity and the Semantic Sample Store

- **Goal:** Convert balance responses into deduplicable, secret-free pool observations and retain only the state needed for cross-restart pace analysis.
- **Requirements:** R1, R2, R3, R5, R6, R7, R22; covers AE2, AE3, and AE4.
- **Dependencies:** U1.
- **Files:**
  - `crates/core/Cargo.toml`
  - `Cargo.lock`
  - `crates/core/src/balance.rs`
  - `crates/core/src/runtime_identity.rs`
  - `crates/core/src/usage_providers.rs`
  - `crates/core/src/quota_pool.rs`
  - `crates/core/src/lib.rs`
  - `crates/core/src/runtime_store.rs`
  - `crates/core/src/runtime_store/metadata.rs`
  - `crates/core/src/state.rs`
- **Approach:** Define and register the new top-level `quota_pool` module in `crates/core/src/lib.rs`, then add pool scope, counter kind, reset/window semantics, capability flags, identity evidence/confidence, aggregation eligibility, and a unit-tagged fixed-point `QuotaQuantity`. Persist each observation's original unit plus conversion source/divisor/generation; never rewrite old raw samples after conversion discovery. Namespace remote stable IDs by origin/issuer and scope. Add optional `quota_pool_id` and reset timezone/divisor hints to `usage_providers.json`; load or create one restricted per-install identity key in RuntimeStore and use a domain-separated keyed digest of origin, scope, and credential for fallback identity. A replacement RuntimeStore creates a new installation identity and does not merge old pool history. Extend existing Sub2API/New API/common adapters to populate evidence and probe New API's public status `quota_per_unit` per origin before claiming USD.

  Make a runtime-owned `QuotaPoolRegistry` the only writer and membership timeline. All refresh paths converge at the same `ProxyState` observation boundary after validating endpoint context and raw response freshness. Build the candidate registry outside publication, then commit an accepted provider-policy observation and the quota-registry document in one SQLite immediate transaction. An ignored stale or inactive-incarnation observation is retained only as history and leaves the active quota candidate, balance projection, suppression state, and adapter hints unchanged. Acquire projection locks in one fixed order before the transaction; after commit, replace the policy, quota, and balance projections synchronously and notify observers only after all three agree. Persist active epoch anchors, recent valid observations, last success/attempt, adjustment and membership revisions, and bounded inactive history. Reject stale/conflicting generations and never replace an invalid/future document with an empty default.
- **Patterns to follow:** Use `ProviderEndpointKey` for request association but not as proof of a shared pool. Follow U1's RuntimeStore document/private-key APIs and the existing balance-recording convergence point for all adapter triggers. Keep the semantic registry document independent from the full `ProviderBalanceSnapshot` schema.
- **Test scenarios:**
  - An issuer-namespaced remote stable ID outranks explicit ID, explicit ID outranks digest fallback, repeated provider-endpoint views of one fallback key merge, and identical remote IDs from different origins/scopes remain distinct.
  - The install identity is stable across RuntimeStore reopen, differs across stores, inherits protected database permissions, and never appears in a reader-facing operator DTO.
  - Raw credentials, the full digest input/output, and the installation identity key do not appear in serialized samples, snapshots, logs, or fixtures.
  - Valid `used`, `remaining`, direct today, limit, window start, and reset values round-trip; stale/error carry-forward does not append a new amount sample.
  - New API custom `quota_per_unit`, explicit divisor fallback, missing/invalid status, raw-unit fallback, and HTTP 200 business failure receive the correct unit, conversion generation, and confidence.
  - Raw-to-USD discovery, configured-to-remote divisor promotion, and divisor changes across restart open a new normalization epoch and never create a synthetic delta.
  - Startup, request-triggered, manual, and scheduled refreshes produce the same registry delta; concurrent completions retain every valid point, reject duplicate/out-of-order data, and cannot commit an older document generation.
  - A generation-two completion followed by a delayed generation-one completion, and a delayed completion for an inactive credential incarnation, retain the newer active policy, quota, balance, suppression, and adapter-hint state while preserving ignored observation history.
  - Injected policy, quota-document, transaction-end, and task-cancellation boundaries publish neither a partial durable pair nor a partial in-memory projection; successful publication remains identical after restart.
  - Missing state starts an empty registry; corrupt or unsupported documents are surfaced and remain unchanged rather than being replaced by a default payload.
  - Current epoch anchor and recent two-hour density survive restart while old inactive pools and redundant points are pruned by both age and count.
- **Verification:** Pool identity, adapter parsing, privacy, and persisted-store tests pass without network or user configuration.

### U4. Capture Stable Request Cost, Project, and Pool Facts

- **Goal:** Make local project attribution stable across RuntimeStore hydration and queryable for arbitrary remote quota periods without creating an unbounded analytics warehouse.
- **Requirements:** R10, R11, R12, R18, R22; supports F3 and covers AE1, AE9, and AE11.
- **Dependencies:** U2, U3, U5.
- **Files:**
  - `crates/core/src/state/session_identity.rs`
  - `crates/core/src/state/runtime_types.rs`
  - `crates/core/src/state/attribution_index.rs`
  - `crates/core/src/state.rs`
  - `crates/core/src/sessions.rs`
  - `crates/core/src/proxy/request_observer.rs`
  - `crates/core/src/proxy/service_core.rs`
  - `crates/core/src/logging.rs`
  - `crates/core/src/request_ledger.rs`
- **Approach:** At request completion, retain one immutable accounting record containing one `ended_at`, strictly serialized captured component costs/total, selected tier, effective catalog revision, normalized project root/fallback, `ProviderEndpointKey`, and optional pool-membership key/revision/confidence from the registry at that instant. Commit this record as the RuntimeStore logical-request terminal before publishing live projections or debug output. A debug-log filter, rotation, or append failure cannot suppress or alter the committed accounting record. Missing pool evidence leaves endpoint-only attribution, and a later identity revision never silently rewrites prior membership.

  Add a sparse time-bounded and count-bounded attribution index keyed by time bucket, endpoint/eligible pool revision, project, and price coverage. On startup, hydrate it by keyset-paginating committed RuntimeStore terminals inside the retained horizon. Expose loaded/queried bounds plus retention, count, dedupe, partial-boundary, price, overflow, and unmatched coverage. Captured cost facts are never repriced during hydration; invalid, unpriced, or reconstructed coverage cannot claim exact reconciliation.
- **Patterns to follow:** Keep `FinishedRequest` serde defaults for committed payload evolution, use immutable logical-request identity for dedupe, and reuse `infer_project_root_from_cwd`. Treat committed terminals as the durable source and the index as a rebuildable bounded projection; request/control-trace JSONL readers are diagnostic only.
- **Test scenarios:**
  - The committed terminal and live projection expose the same ended-at timestamp, component/total cost, tier, effective revision, project root, endpoint, and optional membership revision from one completion record.
  - Injected terminal-commit failure publishes neither live accounting nor a successful debug completion; a later debug append failure leaves the committed terminal and live projection intact.
  - A request ending across local midnight lands in the same hydrated/live bucket; two subdirectories combine under one Git root, while missing/deleted/relative paths remain fallback or unknown without panic.
  - A request completed before pool discovery remains endpoint-only; a later identity upgrade, credential rotation, or proof that two endpoints share a pool starts a membership revision without retroactively double-counting history.
  - Sparse queries cover crossing-midnight, rolling 24-hour, custom 24-hour, and monthly intervals for two pools; a retained-history or partial-bucket boundary lowers confidence and never fabricates external usage.
  - Startup hydration and concurrent live completion count each committed logical request once, preserve captured cost after catalog changes, and mark reconstructed, invalid-captured, unpriced, unmatched, overflow, and dedupe coverage separately.
  - A committed terminal without endpoint, project, cost, tier, or membership facts remains readable and contributes only under explicit unknown coverage.
  - No serialized request or fixture contains credential material used by pool identity.
- **Verification:** Request completion, terminal commit/publish ordering, RuntimeStore hydration, bounded attribution, project-root, and debug-sink failure tests prove one accounting path and honest coverage.

### U6. Run One Continuous Daemon Quota Sampler

- **Goal:** Observe shared remote usage during local idle time while preserving existing relay protection and explicit refresh behavior.
- **Requirements:** R4, R5, R6, R7, R22; implements F1 and supports F2.
- **Dependencies:** U3, U5.
- **Files:**
  - `crates/core/src/usage_providers.rs`
  - `crates/core/src/runtime_host.rs`
  - `crates/core/src/proxy/service_core.rs`
  - `crates/core/src/proxy/providers_api.rs`
  - `crates/core/src/proxy/mod.rs`
  - `src/cli_app.rs`
  - `crates/server/src/main.rs`
- **Approach:** Make `ProxyRuntime::start` create and retain exactly one sampler task after persisted registry state is loaded. The task performs an initial refresh, wakes on a jittered scheduler, calls the existing non-forced provider refresh path, and exits and joins through the runtime shutdown receiver. Remove manual startup spawns from CLI/server entry points; cloning `ProxyService` cannot create another sampler. Preserve the two-minute hard floor and explicit configured intervals; use a five-minute default for analytics-capable providers so a healthy 15-minute window can contain at least three points. Honor slower explicit intervals, terminal suppression, current-period exhaustion wakeups, `Retry-After`, and exponential failure backoff. Request, manual, startup, and scheduled results all feed the U5 registry through the same state recording entry, while attached TUI startup never creates runtime work.
- **Patterns to follow:** Reuse `ProxyRuntime` task/shutdown ownership, `refresh_provider_balances_for_proxy`, and current refresh coalescing/suppression. Avoid a public fire-and-forget sampler API, a second HTTP client, or an adapter-specific store path.
- **Test scenarios:**
  - A fake-clock daemon samples while no requests occur and stops without another refresh after shutdown.
  - Interactive and server runtimes each start exactly one sampler and one price-sync task; attached TUI and state-only test construction start none.
  - Repeated `ProxyService` clones or manual/request refreshes do not add sampler tasks; runtime shutdown waits for task exit and no later I/O occurs.
  - Jittered schedules still honor the hard floor and configured slower interval; concurrent manual/request/background triggers coalesce.
  - 429 `Retry-After`, terminal auth error, current-period exhaustion, reset wakeup, and repeated transient failures produce the expected suppression/backoff without zero samples.
  - A restart loads the previous point and continues the epoch, while a long offline gap remains an explicit gap and is not interpolated.
- **Verification:** Tokio paused-time lifecycle tests and existing provider polling/suppression suites prove ownership, cadence, coalescing, and shutdown behavior.

### U7. Build Pool Rates, Reset Pace, and Signed Project Reconciliation

- **Goal:** Produce one bounded core analytics view that truthfully combines remote pool burn with eligible local project estimates.
- **Requirements:** R1, R6, R8, R9, R10, R11, R12, R19; implements F2 and F3 and covers AE1 through AE4.
- **Dependencies:** U4, U5, U6.
- **Files:**
  - `crates/core/src/quota_analytics.rs`
  - `crates/core/src/lib.rs`
  - `crates/core/src/state/runtime_types.rs`
  - `crates/core/src/state.rs`
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/core/src/dashboard_core/types.rs`
  - `crates/core/src/proxy/api_responses.rs`
- **Approach:** Register the new top-level `quota_analytics` module in `crates/core/src/lib.rs`, then add pure pool epoch/rate/pacing/reconciliation builders over the U5 normalization signature. Prefer a valid cumulative used counter, otherwise a responsive remaining decrease, and use direct remote daily/window values for display without treating them as a slope; remaining-only burn is a lower-bound estimate. Require at least three fresh points plus minimum covered span inside one identity/counter/unit/conversion/window epoch for rate and forecast calculations, while allowing a fresh authoritative direct daily/window total to render in low-sample state. Use 60-minute burn for ETA/reset projection, 15-minute burn for acceleration context, explicit remote reset first, configured IANA reset fallback second, and unknown otherwise. Classify `pace_ratio = rate_60m / required_rate` with a 0.8-1.2 on-pace deadband and user-neutral faster/slower wording; reset-less wallets omit reset pace.

  Query U4's attribution index only for `[epoch_start, min(now, epoch_end))` and require compatible USD normalization plus complete enough time/pool/price coverage before reconciliation. Compute the difference once with checked signed femto-USD arithmetic, serialize a canonical `SignedUsdDelta` with no negative zero, derive nonnegative external usage, and retain negative inconsistency. Raw/incompatible units or truncated coverage show remote and local values separately with reconciliation unavailable. Delete `UsageBalanceCalibration` and its multiplier application; move reusable reset/pacing helpers behind the new pool model.
- **Patterns to follow:** Preserve nonnegative fixed-point `UsdAmount` for costs/balances, introduce a separate signed delta only in analytics, reuse local-day helpers in `usage_day.rs`, and keep quota calculations as pure functions in `quota_analytics.rs`. Do not let operator/TUI code redo math or unit conversion.
- **Test scenarios:**
  - Three or more monotonic points produce correct 15/60 rates; two points, short span, stale latest point, long gap, or mixed epoch returns low-sample/unavailable.
  - Midnight reset, rolling 1d, custom 24h, monthly, reset-less wallet, and DST 23/25-hour calendar windows retain distinct labels and reset behavior.
  - Refund/top-up, used rollback, remaining increase, plan/limit/reset change, out-of-order point, and settlement delay segment the epoch or invalidate the affected rate window rather than merely lowering confidence or producing negative burn.
  - Identity revision, counter source, raw/USD unit, configured/remote divisor generation, scope, window, reset, limit, or plan change opens a new epoch and cannot produce a cross-boundary slope.
  - Remote 100/local projects 55/local unknown 5 yields external 40; remote 50/local 60 yields external 0 plus signed gap -10 and inconsistent confidence. Negative delta JSON/TUI round-trips exactly, normalizes negative zero, and handles arithmetic bounds without wraparound.
  - Raw quota or mismatched conversion generations never subtract local USD; both values remain visible and reconciliation is unavailable/estimated as appropriate.
  - Requests mapped to another pool or outside the aligned epoch do not enter reconciliation; missing price/project/endpoint increments the correct coverage counter.
  - Retention/count/dedupe truncation, incomplete price coverage, or a partial start bucket lowers coverage and cannot be labeled a full-period external remainder.
  - Explicit pool identity deduplicates provider-endpoint copies; ambiguous pool rows remain separate and no aggregate claims exactness.
  - `quota_resets_at_ms` outranks configured reset fallback, and only a proven calendar-day reset uses "today/midnight" wording.
- **Verification:** Pure analytics tests cover counter/epoch/rate/pacing/reconciliation matrices; operator-read-model tests prove bounded rows and capability metadata.

### U8. Make Quota Pace and Full Projects First-Class in Page 5

- **Goal:** Turn the existing local-day Usage page into a user-facing shared-quota and attribution surface without losing local operational context.
- **Requirements:** R8, R9, R10, R11, R12, R19, R20, R21, R22; implements F2, F3, and F5 and covers AE1, AE2, AE3, AE9, and AE10.
- **Dependencies:** U7.
- **Files:**
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/core/src/dashboard_core/types.rs`
  - `crates/core/src/proxy/api_responses.rs`
  - `crates/tui/src/tui/model.rs`
  - `crates/tui/src/tui/types.rs`
  - `crates/tui/src/tui/state.rs`
  - `crates/tui/src/tui/attached.rs`
  - `crates/tui/src/tui/input/normal.rs`
  - `crates/tui/src/tui/view/stats.rs`
  - `crates/tui/src/tui/i18n.rs`
  - `crates/tui/src/tui/report.rs`
- **Approach:** Add quota analytics to the canonical version 5 operator bundle consumed by both local and attached models. Recompose Page 5 so the selected pool's remote used/remaining, 15/60 rate, required rate, reset/ETA, faster/on-pace/slower classification, freshness, source/unit, scope, and confidence occupy the primary KPI area instead of Retry Gate. Keep local-day totals and hourly shape available as secondary context. Expand focus/navigation to pools, projects, and provider endpoints; Projects becomes a full scrollable table with local known/unknown and external/reconciliation rows, while model/session summaries remain compact. Raw-unit pools show their actual unit and omit USD reconciliation; retention/count/dedupe/boundary/price coverage is visible without presenting it as a project row. Keep explicit refresh through the daemon and preserve valid cached facts with stale/offline state on failure. The separate Routing page and provider-info overlay use provider/endpoint terminology only.
- **Patterns to follow:** Retain ratatui table state, stable layout constraints, formatting helpers, `g` refresh handling, bilingual i18n constants, local/attached input parity, and current TestBackend render assertions. Follow aio-coding-hub's stale quota and leaderboard-share behavior, not its web card layout.
- **Test scenarios:**
  - Fresh, syncing, stale/offline with cached data, no adapter, ambiguous identity, unlimited, exhausted, just reset, no rate, and low-sample states render distinct concise labels.
  - Pools and full Projects tables scroll/select in local and attached modes; focus changes do not resize or overlap the layout.
  - A pool row exposes source/scope/data age/confidence without any key or unsanitized remote error.
  - Remote/local/external/signed-gap values remain arithmetically consistent and do not present external as a local project path.
  - Raw-unit or mismatched-generation pools render both sides with reconciliation unavailable; signed negative gaps have the same value in core JSON, report export, and TUI.
  - Faster/on-pace/slower labels honor the 0.8/1.2 boundaries, and reset-less wallets do not mention midnight pace.
  - Local and attached modes render one version 5 operator bundle consistently; unavailable/stale attached states never synthesize zero quota facts or enable writer actions.
  - 128x26 and 76x22 backends render every major state without overlap, clipped headers, or layout jumps.
  - TUI report/export carries the same core values and coverage labels as the screen.
- **Verification:** TUI model, attached observer input, stats rendering, i18n, and report tests prove state coverage, read-only behavior, privacy, and narrow layouts.

### U9. Add Operator Controls, Documentation, and Final Cleanup

- **Goal:** Make synchronization and quota semantics inspectable, document the new behavior, and remove old misleading paths before release.
- **Requirements:** R7, R16, R17, R20, R21, R22, R23; completes F4 and F5.
- **Dependencies:** U3, U6, U8.
- **Files:**
  - `src/cli_types.rs`
  - `src/commands/pricing.rs`
  - `src/commands/usage.rs`
  - `crates/core/src/basellm_metadata.rs`
  - `README.md`
  - `README_EN.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
  - `CHANGELOG.md`
- **Approach:** Add read-only remote price status plus an explicit `force-refresh` command that opens RuntimeStore as the sole writer and operates on the BaseLLM LKG/attempt document. `force-refresh` must acquire the exclusive writer lease; when the resident daemon owns it, the command fails explicitly and tells the operator that the daemon performs background refresh. Keep explicit import-to-manual behavior clearly named and provider-filtered. Report remote body generation separately from effective catalog revision and last-check metadata, including manual shadow/reload and document read-only states. Extend user-facing usage/report output only where it consumes the same pool DTO. Document `quota_pool_id`, installation-local identity behavior, sampling intervals, reset/divisor/conversion-generation fallbacks, raw-unit limitations, pool scope/confidence, external/signed-gap and coverage meaning, BaseLLM source/age/tier behavior, manual precedence, and the debug-only JSONL boundary. Remove metadata-only sync wrappers, dead balance-calibration DTOs/tests, per-request catalog/model-compat reparsing, duplicate TUI math, and obsolete persistence descriptions after all consumers migrate.
- **Patterns to follow:** Keep clap help concise, output both human-readable and existing JSON modes where applicable, preserve bilingual README/config structure, and write changelog entries as user-visible outcomes rather than internal DTO names.
- **Test scenarios:**
  - Status works offline from LKG and distinguishes never-synced, fresh, stale, last-error, unsupported/read-only state, content/check generations, and manual-shadowed/reloaded states.
  - Force refresh updates the RuntimeStore LKG/attempt document but not `pricing_overrides.toml`; it succeeds only while holding the exclusive writer lease and fails explicitly while the resident daemon owns that lease. Explicit manual import changes only the requested provider/model rows and preserves tiers.
  - CLI JSON output contains no secrets and remains parseable when optional remote fields are absent.
  - Docs never call rolling 1d "today", never call BaseLLM an exact bill, and explain that ambiguous pools are not summed.
  - Repository search finds no production call that applies a shared balance delta as a local price multiplier, loads BaseLLM metadata per model conversion/request, or spawns background refresh outside `ProxyRuntime` ownership.
- **Verification:** Root CLI tests, documentation audit, dead-path search, and the full verification contract pass.

---

## Verification Contract

| Gate | Applies To | Done Signal |
|---|---|---|
| Canonical persistence | U1, U3, U5 | `cargo nextest run -p codex-helper-core runtime_store basellm quota --no-fail-fast` proves atomic schema migration, exclusive writer lease, document CAS, committed terminals, quota/private-key durability, stale-generation rejection, and future-schema preservation. |
| Tiered pricing | U2-U4 | `cargo nextest run -p codex-helper-core pricing tier request_ledger --no-fail-fast` reproduces 272000/272001, provider-scoped manual behavior, one immutable effective revision per committed request, hydration without repricing, and the supplied audit totals. |
| Quota identity and adapters | U5-U7 | `cargo nextest run -p codex-helper-core quota_pool usage_provider new_api sub2api --no-fail-fast` proves issuer/scope identity, per-install unlinkability, typed units, conversion generations, privacy, and ambiguous aggregation behavior. |
| Sampling and analytics | U6-U7 | `cargo nextest run -p codex-helper-core quota_sampler quota_analytics --no-fail-fast` proves one runtime-owned sampler, joined shutdown, normalization epochs, 15/60 rates, reset pace, signed delta serialization, and raw-unit reconciliation refusal. |
| Operator read model | U7-U8 | `cargo nextest run -p codex-helper-core operator_read_model operator_summary --no-fail-fast` and `cargo nextest run -p codex-helper-tui model attached --no-fail-fast` prove bounded version 5 DTOs, raw/incompatible-unit states, coverage states, one-bundle refresh, and observer-only attached behavior. |
| TUI usage experience | U8 | `cargo nextest run -p codex-helper-tui stats report i18n --no-fail-fast` proves pools/projects navigation, state labels, privacy, and 128x26/76x22 rendering. |
| CLI behavior | U2, U3, U9 | `cargo nextest run -p codex-helper pricing usage --no-fail-fast` proves provider-filtered import, LKG status/refresh, content versus check revisions, JSON output, manual precedence, document read-only reporting, and explicit force-refresh failure while the resident writer lease is held. |
| Formatting | All Rust units | `cargo fmt --all --check` produces no diff. |
| Lints | All code units | `cargo clippy --workspace --all-targets -- -D warnings` succeeds. |
| Full regression | All units | `cargo nextest run --workspace --no-fail-fast` succeeds after focused gates. |
| Documentation and privacy | U8-U9 | README/config/changelog describe scope, unit/conversion generation, identity, and confidence correctly; serialized fixture scans contain no credentials, installation identity key, IP addresses, or unsanitized provider payloads. |

---

## Definition of Done

| Scope | Done Signal |
|---|---|
| Remote total truth | Supported pools use remote scoped counters for shared burn, duplicate proven identities collapse once, and ambiguous identities remain visibly unaggregated. |
| Sampling continuity | One `ProxyRuntime`-owned sampler records valid observations during local idle time, serializes them through one registry, persists monotonic bounded semantic state, resumes across restart, joins shutdown, and never invents data across gaps or failures. |
| Rate and pace | 15/60-minute rates, required rate, reset projection, and ETA use one fresh normalization epoch with explicit confidence; identity/unit/conversion changes split epochs, and reset-less or non-calendar windows receive accurate labels. |
| Local attribution | One immutable committed terminal drives live and hydrated projections; the bounded time/pool/project index supports aligned periods, and retention/count/dedupe/boundary/price/overflow/unmatched coverage stays explicit. Debug JSONL never participates in accounting. |
| Reconciliation | Compatible USD local known, local unknown, external/unattributed, and signed negative delta align to one pool/window/generation; raw or incomplete coverage is unavailable rather than coerced, and no shared delta changes a request price or gets distributed to projects. |
| Pricing correctness | Service-aware canonical provider selection and context tiers reproduce the 272k boundary and the `$501.493510`/`$500.108175`/`$1,001.601685` audit values. |
| Remote pricing resilience | Startup uses bundled plus any valid BaseLLM LKG plus manual overrides in one effective content revision; stale 200/304 responses and every failure class preserve newer content, manual changes reload without per-request parsing, and automatic sync never writes manual overrides. |
| Persistence safety | RuntimeStore transactions, the exclusive writer lease, exact schema migration, and document revisions prevent partial or stale runtime publication. Future schemas remain unmodified; atomic file replacement is limited to operator configuration and manual overrides. |
| TUI product shape | Page 5 prioritizes pool quota/pace, offers full pool/project tables, preserves local-day context, handles all declared states, and remains coherent at wide and narrow sizes. |
| Read boundary and privacy | RuntimeStore/OperatorReadModel are the only production read paths; `requests.jsonl` and `control_trace.jsonl` are bounded debug sinks only. Future state schemas are preserved, and no credential, installation identity key, or sensitive raw provider data enters documents, snapshots, reports, fixtures, or logs. |
| Operator documentation | CLI status/refresh, README, bilingual configuration docs, and changelog explain sources, tiers, sampling/reset/divisor behavior, confidence, and override precedence. |
| Cleanup | Superseded shared-balance calibration, metadata-only sync, per-request disk catalog rebuild, duplicate TUI math, abandoned experiment code, and stale documentation are removed before final validation. |

---
title: Canonical Relay Runtime Modernization - Plan
type: refactor
date: 2026-07-10
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
deepened: 2026-07-10
---
# Canonical Relay Runtime Modernization - Plan

> Decision amendment (2026-07-14): version 5 remains the only runtime configuration contract, but supported legacy `config.json`, unversioned TOML, and version 1-4 TOML are now accepted as one-time migration inputs. Startup and `config migrate` use the same validated converter, create a source backup, and publish canonical version 5 TOML; the runtime still has no parallel legacy reader. This amendment supersedes the plan's earlier requirements to reject legacy files and remove every migration command/reader, while preserving the requirement to remove legacy runtime models and compatibility projections.

## Goal Capsule

| Field | Value |
| --- | --- |
| Objective | Replace duplicated config, routing, accounting, persistence, WebSocket, balance-policy, and operator-read paths with one coherent relay runtime while preserving the real on-disk route graph contract: ~/.codex-helper/config.toml with version = 5. |
| Authority | Current Codex protocol behavior, official OpenAI GPT-5.6 documentation, and the existing version 5 route graph semantics are authoritative. Historical helper compatibility types, migration readers, JSONL replay, cache invalidation workarounds, and remote mutation routes are not. |
| Execution profile | Breaking, cross-cutting Rust and TypeScript refactor. Characterize externally observable behavior first, remove replaced paths in their owning unit, and commit only green logical units. |
| Stop conditions | Stop and surface a blocker if a change invents a new helper schema/version namespace, changes the valid version 5 config contract without an explicit decision, double-bills cache writes, publishes success before durable terminal commit, silently changes endpoint after output or WS binding, mutates Codex auth/cache/SQLite, or leaks credentials. |
| Tail ownership | Complete all units in dependency order, run the Verification Contract, remove abandoned approaches, apply full code review findings, and leave no compatibility shell around deleted behavior. |

---

## Product Contract

### Summary

codex-helper keeps the current version 5 route graph on disk but interprets it through several historical Rust models and runtime projections. Request completion, JSONL replay, model translation, provider balance, policy actions, WebSocket relay, TUI, desktop, and control-plane routes then derive overlapping facts independently.

This refactor keeps the real public config file and schema while deleting internal historical generations. It introduces provider-scoped model and price epochs, evidence-based cache accounting, a helper-owned SQLite authority under the existing state directory, one request lifecycle, one route runtime snapshot, endpoint-scoped policy projection, and one typed read-only operator model.

### Problem Frame

The current loader accepts only version 5 during normal startup, but the implementation still parses it as ProxyConfigV4, compiles it into a legacy ProxyConfig, and projects it back to ProxyConfigV2 for control-plane callers. Validation, explain, HTTP, WebSocket, and mutation routes can therefore disagree about legal targets such as provider.endpoint.

GPT-5.6 adds independently billable cache writes and new model/reasoning facts. Current usage normalization collapses missing fields into zero, subtracts only cache reads from ordinary input, and allows response paths to rebuild price from mutable current configuration. The model bridge also maps official Sol, Terra, and Luna slugs to one Bedrock-style profile.

Responses WebSocket currently returns downstream 101 before it knows whether the upstream accepts the upgrade, preventing Codex from receiving HTTP 426 and falling back to HTTP. A reused socket produces one connection-level terminal record rather than one record for each response.create.

Provider refresh, route eligibility, passive health, forecast configuration, JSON policy files, JSONL request replay, and operator clients are separate mutable interpretations. Remote read tokens can still reach mutation paths, while local switch presets can modify Codex-owned auth, model cache, and SQLite state.

### Requirements

#### Canonical config and runtime ownership

- R1. The canonical user configuration remains ~/.codex-helper/config.toml and continues to serialize version = 5. No new helper schema version or versioned storage namespace is introduced.
- R2. Rust config and route-domain types use semantic unversioned names. ProxyConfigV2, ProxyConfigV4, ServiceViewV2/V4, compatibility station projections, config.json readers, legacy schema readers, and config migration commands are removed after version 5 behavior is characterized.
- R3. Version 5 config validation, route explain, request selection, and operator projection use the same compiled route graph. Legal provider.endpoint leaves, candidate order, pins, retry boundaries, model eligibility, continuity, and manual stops cannot drift between callers.
- R4. Existing operator-owned inputs such as config.toml, pricing_overrides.toml, and usage provider configuration are never treated as generated state. Old generated JSON/JSONL files become ignored after their readers are removed; they do not block startup and are never deleted automatically.
- R5. Container/server startup does not import Codex config/auth, patch Codex files, or expose host-local history unless explicitly configured in accordance with ADR-0001.

#### GPT-5.6, request dialect, and economics

- R6. Model and price facts are scoped by provider adapter, normalized endpoint origin, route scope, and a non-secret account fingerprint. A model slug alone cannot select capability, request dialect, or price.
- R7. The provider catalog represents Sol, Terra, and Luna independently, including context, default and supported reasoning effort, Responses Lite, WebSocket support, priority support, tool mode, multi-agent version, source authority, freshness, and revision.
- R8. ultra remains a Codex orchestration intent and is sent upstream as max only when the selected provider contract supports that mapping. Pro is represented as Responses reasoning.mode = pro, not a model slug or global default.
- R9. Request semantics distinguish Responses HTTP, Responses WebSocket, compact, and Chat Completions. Unknown future fields are preserved unless a provider contract explicitly rejects them; helper overrides never inject a Responses reasoning object into Chat requests.
- R10. Usage normalization preserves absent, present-zero, and present-value evidence for official cache-read/cache-write fields and compatibility aliases. Official nested fields have deterministic precedence, including explicit zero.
- R11. CacheAccountingConvention derives mutually exclusive ordinary-input, cache-read, and cache-write buckets. For GPT-5.6, ordinary input equals total input minus cache read minus cache write; contradictory evidence yields partial or unknown economics rather than saturation.
- R12. Standard and priority prices are provider-scoped and tier-specific for Sol, Terra, and Luna. Unsupported tiers, regional adjustments, or incomplete price facts remain unknown instead of falling back to standard pricing.
- R13. Any transformed model response, including decompression, filtering, or translation, gets a relay-owned ETag and corrected entity headers. Upstream ETag is retained only for byte-for-byte forwarding; Codex models_cache.json is never read, changed, or deleted.

#### Durable request lifecycle and WebSocket behavior

- R14. The helper opens and validates ~/.codex-helper/state/state.sqlite before listener bind. The database has helper ownership metadata, schema validation, WAL durability settings, and a single lifecycle writer; tests may use an explicit in-memory store.
- R15. A logical request and every actual upstream attempt have cross-restart unique IDs. Each upstream write is preceded by a durable pending attempt, and its result is committed before retry/failover selection continues.
- R16. A logical request has at most one terminal event. Terminal transactions freeze requested, mapped, reported, and pricing model evidence; requested and actual tier; usage evidence; catalog/pricing epoch; route/policy revision; cost breakdown; and confidence.
- R17. Memory rollups, health publication, operator views, and success markers are updated only from committed events. Duplicate identical terminal signals are idempotent; conflicting terminal signals are invariant violations.
- R18. Startup recovery under exclusive writer ownership marks stranded requests/attempts interrupted exactly once with unknown economics and no automatic retry. Recovery failure prevents listener bind.
- R19. HTTP and SSE use the same lifecycle. Streaming data may pass through before completion, but success terminal markers are withheld until commit; a terminal-commit failure after partial output closes with an error and cannot retry or publish success.
- R20. Responses WebSocket completes the selected upstream handshake before downstream 101 using only handshake-visible affinity, explicit connection target, or a handshake-independent singleton candidate. Upstream 426 remains HTTP 426 with no request, health, balance, ledger, or policy side effect.
- R21. A successful socket binds endpoint, continuity domain, route revision, and reviewed metadata. Every response.create creates its own logical request and attempt lifecycle; incompatible route/model/policy revisions require reconnect before upstream write. Control frames and warmup operations never fabricate normal request economics.
- R22. WebSocket metadata follows explicit allowlists. Cookies, proxy/admin authorization, and arbitrary headers cannot cross. x-codex-turn-state may be relayed for the active turn but is never persisted or logged.

#### Balance, policy, operator trust, and client ownership

- R23. Provider adapters acquire and normalize observations but cannot mutate route state. A monotonic observation generation and provider endpoint incarnation prevent stale responses or changed account/origin identities from altering current eligibility.
- R24. Cost estimate, provider quota/balance observation, operator budget, passive health, and route eligibility remain separate facts. HTTP errors, polling parse failures, and passive success cannot create or clear quota actions without an authoritative identity-matched observation.
- R25. Observation, action history, and eligibility projection commit atomically before a new RuntimeSnapshot is published. Manual controls outrank automatic recovery; persistence failure preserves the last known good projection.
- R26. Runtime reload builds and validates a complete immutable snapshot containing config, route, catalog/pricing, and policy revisions, then publishes it once. Requests capture one snapshot; readers see either the old or new bundle.
- R27. One typed OperatorReadModel exposes revisioned, redacted runtime facts to local CLI, attached TUI, desktop, and fleet clients. Disconnected/auth/stale states never fabricate local runtime facts.
- R28. The remote control plane is read-only. Only GET/HEAD routes remain; config, provider, profile, session/global override, reload, shutdown, probe, and refresh mutation routes and clients are removed.
- R29. Discovery cannot replace an established trusted origin or forward a read token to a discovered URL. Remote access requires approved HTTPS origins except explicit loopback behavior and rejects userinfo, query credentials, unsafe redirects, and private/link-local discovered targets.
- R30. switch on/off remains an explicit local human action that may patch only the helper provider stanza in Codex config.toml. Codex auth.json, models_cache.json, and SQLite are outside helper ownership. Patch recovery state lives under helper state, detects external edits, and requires human reconciliation.
- R31. Forecast calculations, test-only usage_balance projection, generic credential templates, auth facade presets, Codex cache invalidation, legacy config/runtime readers, JSONL ledger authority, remote mutations, and duplicate client-side economic assembly are deleted.

### Acceptance Examples

- AE1. Given a valid ~/.codex-helper/config.toml with version = 5, startup loads it from the existing path and never rewrites it to another schema or directory.
- AE2. Given config.json or a non-current config schema, startup reports unsupported configuration without importing, migrating, or deleting it; a valid version 5 config is never treated as stale generated state.
- AE3. Given a graph containing provider.endpoint, conditional nodes, tags, pins, and manual stops, validation, explain, HTTP, and WS-compatible planning produce the same candidate order and reasons.
- AE4. Given official OpenAI GPT-5.6 Sol, Terra, or Luna, the translated catalog reports that model's current Codex facts and never borrows a Bedrock profile or another provider's price.
- AE5. Given total input 1000, cache read 100, and cache write 200 through JSON, Chat, SSE, or WS, the committed record bills 700 ordinary, 100 read, and 200 write exactly once.
- AE6. Given an explicit nested cache-write value of zero plus a positive alias, the nested zero wins. Given contradictory totals, economics is partial/unknown and the conflict remains inspectable.
- AE7. Given Responses Pro, Chat reasoning_effort, Codex ultra intent, and a future unknown reasoning field, each is preserved or normalized only by the applicable endpoint/provider rule; no global Pro/default effort is injected.
- AE8. Given a model-body transformation or decompression, entity headers and relay ETag match the emitted bytes. Starting helper leaves Codex models_cache.json unchanged.
- AE9. Given a price/catalog reload races completion, the terminal record keeps its captured epoch and cost. Replay never recomputes it from current overrides.
- AE10. Given a request retries endpoint A then succeeds on B, both upstream attempts are durable before their writes, A's result commits before B is selected, and the logical request has one success terminal event.
- AE11. Given a crash before write, during an attempt, before terminal commit, or after commit, restart recovery is idempotent and never creates duplicate success, cost, health, or policy changes.
- AE12. Given upstream WS handshake returns 426, Codex receives HTTP 426 and falls back to HTTP with no request-side record. Given upstream 101, downstream 101 occurs only afterward.
- AE13. Given one WS connection handles warmup, normal, and tool-continuation requests, each response.create has an independent lifecycle; control frames create none; concurrent logical requests receive a protocol error.
- AE14. Given a slow exhausted observation generation 1 returns after recovered generation 2, eligibility remains recovered. Endpoint/account identity changes cannot inherit an old automatic action.
- AE15. Given a remote read token, every POST/PUT/PATCH/DELETE control-plane path is absent. A discovery redirect or cross-origin/private target never receives the token.
- AE16. Given attached clients are disconnected, unauthorized, stale, or ready, CLI/TUI/desktop expose the same safe status and revision bundle; only ready data enables runtime actions.
- AE17. Given explicit local switch on/off, only Codex config.toml is patched and a conflicting user edit forces human reconciliation. Codex auth, cache, and SQLite remain byte-for-byte untouched.
- AE18. Given production source audit after completion, no test-only usage balance, forecast engine, auth facade, model-cache deletion, migration reader, legacy route executor, JSONL accounting authority, remote mutation route, or unchecked client economic mapper remains.

### Scope Boundaries

In scope:

- The existing version 5 config and route graph as the sole public helper configuration contract.
- Semantic unversioned Rust types and one compiled route/runtime snapshot.
- Provider-scoped GPT-5.6 capabilities, request dialects, cache evidence, prices, and relay ETags.
- Helper-owned SQLite request/attempt ledger, policy state, session affinity, and revision authority at ~/.codex-helper/state/state.sqlite.
- HTTP, SSE, compact, Chat, and Responses WebSocket lifecycle parity.
- Closed provider observation adapters, durable eligibility policy, typed operator reads, read-only remote control plane, and minimal local Codex config patching.
- Precise removal of replaced config, routing, persistence, forecast, auth/cache, mutation, and client paths.

Deferred to Follow-Up Work:

- Multi-tenant credit billing, subscriptions, a distributed/fleet ledger, cross-node consensus, long-term analytics warehouse, arbitrary provider plugins, and a generic protocol intermediate representation.
- Flex, Batch, and regional pricing enforcement until authoritative provider-scoped facts and operator requirements exist.

Outside this product's identity:

- A new helper config schema version or versioned storage namespace.
- Importing or rewriting legacy helper config/state/ledger data.
- Mutating Codex authentication, model cache, SQLite, provider account state, or provider dashboards.
- Remote or agent-driven reset, switch, policy mutation, probe, refresh, reload, or shutdown.
- Treating local client configuration or stale files as remote runtime truth.

---

## Planning Contract

### Assumptions

- Existing version 5 config files remain valid throughout the refactor.
- Internal types may be renamed or deleted without compatibility aliases; serialized config remains version 5.
- Old generated JSON/JSONL files may remain on disk after their readers are deleted. The helper neither imports nor auto-deletes them.
- One helper home has one active runtime writer. A second runtime fails before listener bind.
- Debug logs, notification state, pricing overrides, and provider input configuration remain file-based non-authoritative inputs/outputs unless a unit explicitly replaces them.
- repo-ref/codex and repo-ref/sub2api are read-only references and never implementation targets.
- The rejected 2026-07-10-001 plan and its untracked fresh_state test are not execution authority because they introduce a helper schema/storage concept the product does not have.

### Key Technical Decisions

- KTD1. Preserve disk schema 5 and remove internal generations. Parse the current graph directly into semantic config types; keep no V2/V4 aliases, migration report, config.json reader, or compatibility station after the canonical runtime is complete.
- KTD2. Characterize before breaking. External route, usage, ETag, reasoning, retry, continuity, operator, and WS behavior fixtures land before their historical implementation is removed.
- KTD3. Scope facts by provider identity and epoch. Provider scope combines adapter kind, normalized origin, route scope, account fingerprint, and config revision; attempts freeze one immutable catalog/pricing epoch.
- KTD4. Normalize evidence before pricing. Tri-state source evidence and an explicit accounting convention produce exclusive buckets; inconsistent facts cannot become a zero-cost or apparently valid charge.
- KTD5. Separate request dialect from model slug. Endpoint and provider facts determine legal reasoning, compact, Chat, Responses Lite, and tier behavior. The helper preserves unknown fields and does not duplicate Codex's Responses Lite request transformation.
- KTD6. Compile one route graph and publish one RuntimeSnapshot. Config validation, request selection, explain, policy reconcile, and operator reads consume the same stable digest/revision and atomically published snapshot.
- KTD7. Use helper-owned SQLite as the durable event authority. Open ~/.codex-helper/state/state.sqlite before bind, use application ownership/schema checks, WAL, full durability, foreign keys, a busy timeout, exclusive writer ownership, and idempotent startup recovery.
- KTD8. Model logical requests and upstream attempts separately. Every network attempt is pending before write and terminal before retry; one logical terminal freezes all economic and routing facts.
- KTD9. Gate transport success on commit. Nonterminal streaming data may pass, but HTTP body publication, SSE response.completed/[DONE], and WS response.completed are held until terminal commit. After partial output, commit failure ends the stream as an error without retry.
- KTD10. Bind WS before downstream upgrade. A compiled handshake selector chooses an endpoint from handshake-visible facts, establishes the upstream socket, preserves HTTP failures, then transfers the accepted socket into the downstream upgrade task.
- KTD11. Publish eligibility only after an ordered observation transaction. Provider adapters emit observations; a monotonic generation and endpoint incarnation protect against late/stale results; request health cannot mutate quota eligibility.
- KTD12. Keep remote reads and local human writes separate. OperatorReadModel is query-only and coherent; remote routes are GET/HEAD only. switch on/off is the sole retained Codex-file mutation and touches only config.toml with recovery evidence.

### High-Level Technical Design

~~~mermaid
flowchart TB
  Config[Existing config.toml version 5] --> Compiler[Canonical graph compiler]
  ProviderFacts[Provider-scoped model and price facts] --> Catalog[Catalog epoch]
  PolicyStore[Committed eligibility revision] --> Snapshot[Immutable RuntimeSnapshot]
  Compiler --> Snapshot
  Catalog --> Snapshot
  Snapshot --> Request[Logical request lifecycle]
  Request --> Attempt[One or more upstream attempts]
  Attempt --> Store[state/state.sqlite]
  Store --> ReadModel[OperatorReadModel]
  ReadModel --> Clients[CLI / TUI / desktop / fleet]
~~~

~~~mermaid
sequenceDiagram
  participant C as Client
  participant R as Relay
  participant S as SQLite
  participant U as Upstream
  C->>R: HTTP, SSE, or response.create
  R->>S: insert pending logical request and attempt
  S-->>R: committed
  R->>U: upstream write
  U-->>R: output / terminal evidence
  R->>S: commit attempt result and logical terminal
  S-->>R: committed event
  R-->>C: success terminal marker
~~~

~~~mermaid
sequenceDiagram
  participant C as Codex
  participant R as Relay
  participant U as Upstream
  C->>R: WebSocket upgrade
  R->>R: select from handshake-visible facts
  R->>U: upstream WebSocket handshake
  alt upstream HTTP failure
    U-->>R: 426 / 401 / 429 / 5xx
    R-->>C: same safe HTTP result
  else upstream accepted
    U-->>R: 101 and allowlisted metadata
    R-->>C: 101
    loop serialized response.create
      C->>R: logical request
      R->>R: validate binding and run lifecycle
      R->>U: request frame
      U-->>R: frames and terminal evidence
      R-->>C: frames after lifecycle rules
    end
  end
~~~

~~~mermaid
flowchart TB
  Adapter[Closed provider adapter] --> Observation[Observation + generation + incarnation]
  Observation --> Transaction[SQLite policy transaction]
  Manual[Local human control] --> Transaction
  Transaction -->|commit| Projection[Eligibility revision]
  Transaction -->|failure or stale| Previous[Last known good projection]
  Projection --> Snapshot[Next RuntimeSnapshot]
  Previous --> Snapshot
~~~

### Delivery Sequence

1. Freeze external behavior and the real version 5 config contract.
2. Canonicalize config ownership, then implement usage evidence and provider catalogs.
3. Compile one route runtime and establish the SQLite authority before replacing request execution.
4. Move HTTP/SSE to the durable lifecycle, then rebuild Responses WebSocket on that lifecycle.
5. Move observations and eligibility into ordered transactions, then build canonical economics and operator projections.
6. Remove remote writes, unsafe Codex integration, forecast/dead projections, legacy readers/executors, and stale documentation in their owning units.
7. Finish with source-surface audit, full test matrix, simplification, and adversarial review.

### System-Wide Impact

| Surface | Impact |
| --- | --- |
| Configuration | Existing version 5 config remains valid; internal V2/V4 generations and migrations disappear. |
| Startup | Helper SQLite ownership/schema/recovery is verified before listeners; Codex files are not imported or patched by server startup. |
| Routing | One graph compiler, stable route revision, atomic runtime snapshot, and shared explain/selection semantics replace compatibility station projections. |
| Economics | Usage evidence, captured catalog epochs, tier-specific cache write prices, and committed terminal records replace request-time/current-config reconstruction. |
| Persistence | SQLite becomes request ledger, policy, affinity, and revision authority; old JSON/JSONL readers are removed while debug logs remain optional output. |
| WebSocket | Real upstream-first handshake, HTTP fallback, endpoint binding, per-response lifecycle, and reviewed metadata replace connection-level accounting. |
| Provider policy | Ordered identity-scoped observations publish eligibility only after durable commit; passive health stays separate. |
| Operator clients | CLI, TUI, desktop, and fleet receive one redacted revision bundle; remote mutation paths disappear. |
| Codex ownership | Only explicit local config.toml patching remains; auth, model cache, and Codex SQLite modifications are deleted. |

### Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Config rename accidentally changes the public format. | Golden round-trip fixtures assert the existing version 5 TOML and graph semantics before type/file renames. |
| SQLite blocks async request execution. | Use a single dedicated writer boundary or short synchronous critical sections; never hold an async lock across await; provide explicit test store injection. |
| Crash or retry duplicates terminal events. | Use UUID identities, conditional terminal transitions, unique constraints, and idempotent startup recovery. |
| Streaming success escapes before commit. | Withhold success terminal markers; inject commit failures across HTTP, SSE, and WS tests. |
| Catalog reload mixes old model and new price facts. | Build complete epochs off-path and atomically publish one RuntimeSnapshot; attempts capture once. |
| WS route needs body facts unavailable before 101. | Enable WS only for routes with affinity, explicit connection target, or handshake-independent singleton selection; otherwise reject WS capability only. |
| Stale provider refresh clears a newer action. | Persist monotonic generations and endpoint incarnations; stale results may be history but cannot alter projection. |
| External Codex patch overwrites user edits. | Journal before write, compare fingerprints before restore/reconcile, and require local human action. |
| Remote read token leaks through discovery/redirect. | Pin approved origins, disable automatic redirects, validate targets, and never attach tokens to discovery. |
| Broad deletion removes a still-used helper. | Delete in owning units after focused tests, then use final source audit as verification rather than the deletion mechanism. |

### Sources and Research

- crates/core/src/config.rs, crates/core/src/config_storage.rs, and crates/core/src/config_v4.rs establish the current disk schema and compatibility layers.
- crates/core/src/routing_ir.rs, crates/core/src/proxy/request_routing.rs, crates/core/src/proxy/route_executor_runtime.rs, and crates/core/src/proxy/attempt_target.rs establish the duplicate runtime paths.
- crates/core/src/usage.rs, crates/core/src/pricing.rs, crates/core/src/state.rs, crates/core/src/request_ledger.rs, and crates/core/src/logging.rs establish current accounting/replay behavior.
- crates/core/src/proxy/responses_websocket.rs and repo-ref/codex/codex-rs/core/src/client.rs establish WebSocket timing and Codex fallback behavior.
- crates/core/src/usage_providers.rs, crates/core/src/provider_signals/, crates/core/src/policy_actions/, and crates/core/src/state/ establish observation and policy behavior.
- docs/adr/0001-central-relay-container-runtime.md establishes central relay ownership.
- OpenAI GPT-5.6 guide: https://developers.openai.com/api/docs/guides/latest-model.md
- OpenAI reasoning guide: https://developers.openai.com/api/docs/guides/reasoning.md
- OpenAI prompt caching guide: https://developers.openai.com/api/docs/guides/prompt-caching.md
- OpenAI pricing: https://developers.openai.com/api/docs/pricing
- sub2api PR #3898 is implementation prior art for cache-write evidence and tiered pricing, not the pricing authority: https://github.com/Wei-Shaw/sub2api/pull/3898
- repo-ref/aio-coding-hub was not present locally during planning; public main informed candidate-order and budget/quota separation only.
- docs/plans/2026-07-10-001-refactor-canonical-relay-runtime-plan.md is a rejected draft because it introduces a helper schema/storage version not present in the product.

---

## Implementation Units

| U-ID | Title | Key files | Depends on |
| --- | --- | --- | --- |
| U1 | Characterize current external contracts | proxy integration tests, config tests, usage/pricing tests | None |
| U2 | Canonicalize version 5 config and Codex ownership | config*, codex integration, CLI config/switch | U1 |
| U3 | Normalize usage evidence and cache accounting | usage, pricing, response semantics | U1 |
| U4 | Build provider catalog epochs and request dialects | models compatibility, pricing, request body | U1, U3 |
| U5 | Compile one route runtime and atomic snapshot | routing IR, runtime config, request routing | U1, U2, U4 |
| U6 | Establish helper SQLite authority | runtime store, startup host, state constructors | U1, U2 |
| U7 | Move HTTP/SSE to the durable request lifecycle | attempt modules, state, ledger, streams | U3, U4, U5, U6 |
| U8 | Rebuild Responses WebSocket on the lifecycle | responses WebSocket, headers, WS tests | U4, U5, U7 |
| U9 | Persist ordered provider policy | usage providers, provider signals, policy actions | U5, U6, U7 |
| U10 | Publish canonical economic projections | usage day, dashboard, TUI/desktop mappers | U4, U7, U9 |
| U11 | Publish typed read-only operator truth | dashboard, control plane, CLI/TUI/desktop | U5, U7, U9, U10 |
| U12 | Remove residual legacy surfaces and certify | exact residual files, docs, verification | U1-U11 |

### U1. Characterize current external contracts

- **Goal:** Pin the behavior that must survive breaking internal changes.
- **Requirements:** R1-R13, R15-R22, R24-R30; covers AE1-AE17.
- **Dependencies:** None.
- **Files:** crates/core/src/config/tests/v4_schema.rs, crates/core/src/config/tests/io_bootstrap.rs, crates/core/src/proxy/tests/routing_profiles.rs, crates/core/src/proxy/tests/api_admin/routing_explain.rs, crates/core/src/proxy/tests/failover/response_semantics.rs, crates/core/src/proxy/tests/failover/response_semantics_compact.rs, crates/core/src/proxy/tests/failover/response_semantics_websocket.rs, crates/core/src/usage.rs, crates/core/src/pricing.rs, crates/core/src/request_ledger.rs.
- **Approach:** Add behavior fixtures for version 5 round trips, legal graph targets, current continuity/retry semantics, GPT-5.6 facts and dialects, tri-state cache usage, transformed ETags, route/runtime parity, durable lifecycle failure points, WebSocket handshake/frames, provider observation ordering, and control-plane method restrictions. Do not preserve internal type names or migration behavior.
- **Execution note:** Start from failing or characterization tests. Keep fixtures at observable seams and delete any assertion whose only purpose is preserving V2/V4 helpers.
- **Patterns to follow:** Existing proxy integration harness, route graph fixtures, request-chain contract tests, and repo-ref/codex client behavior.
- **Test scenarios:** Valid version 5 TOML round-trips unchanged; provider.endpoint validates; JSON/Chat/SSE/WS usage converges; Sol/Terra/Luna fixtures differ correctly; Pro/ultra/Chat reasoning remain endpoint-correct; HTTP/WS candidate reasoning agrees where WS has a selector; 426 and pre-101 failures have no side effects; poisoned metadata stays redacted.
- **Verification:** Every later unit has an externally observable failing or characterization fixture, and no new fixture asserts a project schema/storage version not present today.

### U2. Canonicalize version 5 config and Codex ownership

- **Goal:** Preserve the real version 5 file while removing internal config generations, migration/import paths, and unsafe Codex ownership.
- **Requirements:** R1-R5, R30; covers AE1, AE2, and AE17.
- **Dependencies:** U1.
- **Files:** crates/core/src/config.rs, crates/core/src/config_v2.rs, crates/core/src/config_v4.rs, crates/core/src/config_storage.rs, crates/core/src/config_bootstrap.rs, crates/core/src/client_config.rs, crates/core/src/codex_patch_plan.rs, crates/core/src/codex_integration.rs, crates/core/src/config/tests/, src/commands/config.rs, src/commands/config_doc.rs, src/commands/provider.rs, src/commands/routing.rs, src/commands/route_view.rs, src/commands/switch.rs, src/cli_app.rs.
- **Approach:** Rename the current persistence graph to semantic config types and keep serializing version 5 at the current path. Reject config.json and non-current versions without migration. Remove automatic Codex import/bootstrap and all auth/cache/SQLite facade presets. Retain only explicit local switch on/off: record prepared/applied/recovery state under helper state, patch the helper provider stanza in Codex config.toml, and refuse automatic restoration when the external file fingerprint changed. Keep a temporary compiled-runtime adapter only until U5 deletes it.
- **Execution note:** Land public config round-trip fixtures before renames. Remove readers and their tests together; do not translate legacy fixtures.
- **Patterns to follow:** Current route graph serde shape, config backup/write safety, and existing switch three-way config merge behavior.
- **Test scenarios:** Version 5 file path and bytes stay compatible; config.json/version 4 is rejected with a safe unsupported message; startup does not inspect Codex auth; server never patches Codex files; switch changes only the target stanza; interruption/fingerprint change yields recovery-required without touching user edits; invalid environment references never echo secret values.
- **Verification:** No production loader, CLI, or control-plane path constructs ProxyConfigV2/V4, migrates config, imports Codex auth, or modifies Codex cache/SQLite; current version 5 configuration still starts.

### U3. Normalize usage evidence and cache accounting

- **Goal:** Produce source-aware, contradiction-preserving usage and exclusive billable buckets.
- **Requirements:** R10-R12, R16; covers AE5, AE6, and AE9.
- **Dependencies:** U1.
- **Files:** crates/core/src/usage.rs, crates/core/src/pricing.rs, crates/core/src/proxy/response_semantics.rs, crates/core/src/proxy/attempt_response.rs, crates/core/src/proxy/stream.rs, crates/core/src/request_ledger.rs, crates/core/src/usage_day.rs.
- **Approach:** Add tri-state evidence, source provenance, deterministic nested-field precedence, and provider/protocol CacheAccountingConvention. Preserve raw totals and conflicts. Remove nonzero-as-presence checks from SSE accumulation and bridge conversions.
- **Execution note:** Update current wrong expectations first and observe the intended failures before implementation.
- **Patterns to follow:** Existing usage parsers, femto-USD arithmetic, and incremental SSE scanner.
- **Test scenarios:** Official Responses and Chat fields, aliases, explicit zero, duplicate fields, conflict, underflow, chunk splitting, Anthropic TTL writes, OpenAI GPT-5.6 writes, and 1000/100/200 = 700/100/200 all produce explicit evidence and identical canonical buckets.
- **Verification:** Every cost caller consumes canonical buckets and no path can double-bill a cache write as ordinary input.

### U4. Build provider catalog epochs and request dialects

- **Goal:** Make model capability, price, request semantics, and ETag behavior provider-scoped and atomically versioned.
- **Requirements:** R6-R9, R12, R13, R26; covers AE4, AE7-AE9.
- **Dependencies:** U1, U3.
- **Files:** crates/core/src/proxy/models_compat.rs, crates/core/src/pricing.rs, crates/core/src/basellm_metadata.rs, crates/core/src/codex_capability_profile.rs, crates/core/src/model_routing.rs, crates/core/src/proxy/request_body.rs, crates/core/src/proxy/request_preparation.rs, crates/core/src/proxy/selected_upstream_request.rs, crates/core/src/proxy/headers.rs, crates/core/src/proxy/attempt_response.rs, crates/core/src/proxy/codex_relay_capabilities.rs.
- **Approach:** Build complete provider catalog epochs off-path, validate authority/provenance, and atomically publish last-known-good snapshots. Encode independent Sol/Terra/Luna capability and standard/priority price fixtures. Interpret Responses, Chat, compact, Pro, max, and Codex ultra through endpoint/provider dialect rules while preserving future fields. Generate entity-correct relay ETags after any body transformation.
- **Patterns to follow:** Existing metadata conditional refresh, model translation tests, price override validation, and upstream Codex model catalog.
- **Test scenarios:** Sol/Terra/Luna context/default effort/multi-agent/priority/Lite facts; OpenAI vs Bedrock isolation; standard/priority cache-write prices and explicit zero; unsupported tiers unknown; Responses Pro preserved; Chat tools with invalid reasoning rejected or routed according to contract; unknown fields survive; decompression/translation removes stale entity headers and changes ETag.
- **Verification:** Model listing, request policy, routing eligibility, price lookup, and operator provenance consume the same captured provider epoch without request-time disk reads.

### U5. Compile one route runtime and atomic snapshot

- **Goal:** Make the current route graph the only validation, explain, selection, and runtime authority.
- **Requirements:** R2, R3, R6, R20, R21, R26; covers AE3 and the routing portion of AE12-AE14.
- **Dependencies:** U1, U2, U4.
- **Files:** crates/core/src/routing_ir.rs, crates/core/src/model_routing.rs, crates/core/src/proxy/runtime_config.rs, crates/core/src/proxy/request_routing.rs, crates/core/src/proxy/route_executor_runtime.rs, crates/core/src/proxy/attempt_target.rs, crates/core/src/proxy/routing_plan.rs, crates/core/src/proxy/route_provenance.rs, crates/core/src/proxy/selected_upstream_request.rs, crates/core/src/proxy/persisted_registry_api.rs, crates/core/src/proxy/runtime_admin_api.rs, crates/core/src/proxy/tests/routing_profiles.rs, crates/core/src/proxy/tests/api_admin/routing_explain.rs.
- **Approach:** Compile exact/wildcard model rules, route candidates, stable route digest, continuity domains, and WS handshake selectors once per config/catalog generation. Build and publish one immutable RuntimeSnapshot. Migrate all callers, then delete Legacy route selection, Legacy attempt target, compatibility station/upstream keys, V2 projections, and dual runtime locks.
- **Execution note:** Characterize current graph semantics before replacing callers. Migrate validator and explain first, then HTTP selection; delete compatibility branches only after source audit shows no callers.
- **Patterns to follow:** RoutePlanTemplate/Executor, ProviderEndpointKey, ContinuityDomainKey, and current explain provenance.
- **Test scenarios:** provider.endpoint and conditional graph validation; exact/wildcard tie rejection; stable input-order-independent model decision; pin/continuity/retry/concurrency/manual stop behavior; reload readers see all-old or all-new revisions; failed reload preserves old snapshot and remains retryable; WS selector rejects only explicitly WS-enabled ambiguous routes.
- **Verification:** Production request, explain, provider listing, reload, and WS preselection all derive from one graph/compiler/snapshot and no legacy runtime enum remains.

### U6. Establish helper SQLite authority

- **Goal:** Add a durable helper-owned state foundation without changing request behavior yet.
- **Requirements:** R14, R18, R25, R26; supports AE9-AE11 and AE14.
- **Dependencies:** U1, U2.
- **Files:** crates/core/src/runtime_store.rs, crates/core/src/runtime_host.rs, crates/core/src/proxy/service_core.rs, crates/core/src/state.rs, crates/core/src/state/session_route_ledger.rs, crates/core/src/state/policy_action_store.rs, crates/core/src/lib.rs, crates/core/Cargo.toml.
- **Approach:** Open ~/.codex-helper/state/state.sqlite before listeners, validate helper application/schema IDs, configure WAL/full sync/foreign keys/busy timeout, acquire lifecycle writer ownership, and inject the store into runtime state. Provide explicit in-memory/test construction. Implement idempotent recovery primitives and stable UUID identities, but leave request/policy business writes to U7/U9.
- **Execution note:** Prove startup and recovery failpoints before integrating request execution. Do not make all test constructors async.
- **Patterns to follow:** Existing rusqlite use, local state path helpers, and dependency injection through ProxyService/ProxyState constructors.
- **Test scenarios:** clean create/reopen; unknown/corrupt schema; DB busy/IO failure before bind; second writer rejection; WAL/SHM preservation; recovery transaction before/after crash; two consecutive restarts; in-memory store isolation; no Codex SQLite access.
- **Verification:** Runtime cannot bind before a valid store/recovery result, tests can inject a store without filesystem coupling, and no behavior-bearing ledger/policy projection has two durable authorities after its owning unit migrates it.

### U7. Move HTTP and SSE to the durable request lifecycle

- **Goal:** Make SQLite terminal events the sole request-accounting authority and centralize request/attempt transitions.
- **Requirements:** R15-R19; covers AE5, AE9-AE11.
- **Dependencies:** U3, U4, U5, U6.
- **Files:** crates/core/src/proxy/request_context.rs, crates/core/src/proxy/request_observer.rs, crates/core/src/proxy/provider_execution.rs, crates/core/src/proxy/attempt_execution.rs, crates/core/src/proxy/attempt_response.rs, crates/core/src/proxy/attempt_failures.rs, crates/core/src/proxy/attempt_health.rs, crates/core/src/proxy/stream.rs, crates/core/src/proxy/response_finalization.rs, crates/core/src/state.rs, crates/core/src/request_ledger.rs, crates/core/src/logging.rs, crates/core/src/local_log_store.rs, crates/core/src/dashboard_core/.
- **Approach:** Persist logical requests and pending upstream attempts before write, commit each attempt result before retry selection, and commit one conditional logical terminal event containing frozen economic/routing evidence. Update rollups/read projections from committed events only. Replace JSONL replay with SQLite queries; retain JSONL solely as post-commit debug output.
- **Execution note:** Implement one HTTP nonstreaming vertical slice first, then retry/failover, then SSE. Observe each terminal-gate failure before broad migration.
- **Patterns to follow:** Existing attempt modules, RoutePlanExecutor retry boundaries, response semantics, and request-ledger read shape.
- **Test scenarios:** A fails/B succeeds; pre-output failure; partial-output failure; duplicate same terminal; conflicting terminal; current price reload during request; server-reported model/tier conflict; commit failure before output and after partial output; SSE completion withholding; crash/restart at each transition; debug-log failure/rotation/only-errors cannot affect ledger.
- **Verification:** HTTP/SSE accounting, health publication, replay, admin ledger, and rollups read committed SQLite events and success cannot escape before terminal commit.

### U8. Rebuild Responses WebSocket on the lifecycle

- **Goal:** Preserve HTTP fallback, bind one endpoint safely, and account for every logical request on a reusable socket.
- **Requirements:** R20-R22 and R15-R19; covers AE5, AE12, and AE13.
- **Dependencies:** U4, U5, U7.
- **Files:** crates/core/src/proxy/responses_websocket.rs, crates/core/src/proxy/request_preparation.rs, crates/core/src/proxy/request_body.rs, crates/core/src/proxy/response_semantics.rs, crates/core/src/proxy/headers.rs, crates/core/src/proxy/tests/failover/response_semantics_websocket.rs.
- **Approach:** Select and establish upstream WS before downstream upgrade, return upstream HTTP failures safely, and transfer the accepted socket into the upgrade task. Bind endpoint/continuity/route revision and allowlisted metadata. For each serialized response.create, validate binding compatibility, run the U7 lifecycle, preserve unknown future frames, and withhold known success terminal events until commit.
- **Execution note:** Prove real pre-101 426 first; a downstream 101 followed by close does not satisfy the contract.
- **Patterns to follow:** repo-ref/codex WebSocket fallback/client event handling and the existing proxy WS integration harness.
- **Test scenarios:** 426/DNS/TLS/401/429/5xx/downstream-upgrade failure with no side effects; accepted header allowlist; no cookie/auth leakage; warmup without fabricated cost; response.cancel, malformed frame, client close, upstream close, duplicate/overlapping create; previous_response_id; unknown event passthrough; route reload reconnect; compatible price epoch reuse; per-request usage and terminal commit.
- **Verification:** WebSocket and HTTP share route/lifecycle/accounting facts, Codex receives genuine HTTP fallback, and one physical socket can never collapse multiple logical requests into one ledger entry.

### U9. Persist ordered provider observations and eligibility policy

- **Goal:** Move provider balance/quota effects from direct mutable state into ordered durable policy transactions.
- **Requirements:** R23-R26; covers AE14.
- **Dependencies:** U5, U6, U7.
- **Files:** crates/core/src/usage_providers.rs, crates/core/src/provider_signals/model.rs, crates/core/src/provider_signals/mod.rs, crates/core/src/policy_actions/model.rs, crates/core/src/policy_actions/mod.rs, crates/core/src/runtime_identity.rs, crates/core/src/state.rs, crates/core/src/state/policy_action_store.rs, crates/core/src/proxy/route_unavailability.rs, crates/core/src/proxy/route_target_selection.rs, crates/core/src/proxy/providers_api.rs.
- **Approach:** Keep closed credential-safe vendor adapters, normalize observation scope/incarnation/generation, and commit observation/action/projection in one SQLite transaction. Publish a new RuntimeSnapshot only after commit. Remove adapter/direct LB mutations and request-error quota mutations; keep passive health separate.
- **Execution note:** Characterize existing trusted exhaustion, daily reset, sibling endpoint, and manual-precedence behavior before changing persistence.
- **Patterns to follow:** ProviderEndpointKey, existing evidence/action types, endpoint refresh queue, and manual action precedence.
- **Test scenarios:** slow exhausted@1 after recovered@2; old recovered@2 after exhausted@3; endpoint origin/account/config revision change; sibling isolation; persistence failure; manual disable; passive success; HTTP 429/transport/poll parse errors; malicious redirect/URL/header cannot receive credential.
- **Verification:** Route selection consumes one monotonic committed eligibility revision, adapters contain no route mutation, and old JSON policy state has no production reader.

### U10. Publish canonical economic projections

- **Goal:** Derive usage, cost, price coverage, provider balance/quota, budget, and policy views from committed facts without forecast-driven routing.
- **Requirements:** R11, R12, R16, R23-R27, R31; covers AE5, AE6, AE9, AE14, and AE16.
- **Dependencies:** U4, U7, U9.
- **Files:** crates/core/src/usage_forecast.rs, crates/core/src/usage_balance.rs, crates/core/src/usage_day.rs, crates/core/src/usage_providers.rs, crates/core/src/dashboard_core/snapshot.rs, crates/core/src/dashboard_core/types.rs, crates/tui/src/tui/model.rs, crates/tui/src/tui/state.rs, apps/desktop/src/lib/api/mappers.ts.
- **Approach:** Preserve local-day/coverage semantics while replacing data sources with committed terminal events and policy projections. Extract the production quota reset-time helper from usage_forecast, then delete forecast/calibration/pacing and its UI config. Delete the test-only usage_balance module and client-side cache-rate/cost derivation. Keep cost, balance, budget, and eligibility visibly distinct.
- **Execution note:** Characterize local-day, unknown/stale/unlimited/exhausted, and reset behavior before deleting forecast code.
- **Patterns to follow:** Existing usage_day aggregation and dashboard coverage/status types.
- **Test scenarios:** unknown/partial economics; standard/priority breakdown; historical record stable after reload; local-day boundary; reset time and timezone; unknown/stale/unlimited/exhausted distinctions; balance vs budget; empty/disconnected projection; no forecast influences route selection.
- **Verification:** Server projections are replayable from committed facts, clients do not recalculate economics, and no production or test-only forecast/usage_balance surface remains.

### U11. Publish typed read-only operator truth

- **Goal:** Give every operator client one coherent, redacted, query-only runtime model and remove remote mutation/trust leaks.
- **Requirements:** R27-R29; covers AE15 and AE16.
- **Dependencies:** U5, U7, U9, U10.
- **Files:** crates/core/src/dashboard_core/snapshot.rs, crates/core/src/dashboard_core/operator_summary.rs, crates/core/src/dashboard_core/types.rs, crates/core/src/proxy/api_responses.rs, crates/core/src/proxy/runtime_admin_api.rs, crates/core/src/proxy/control_plane_routes/, crates/core/src/proxy/control_plane_service.rs, crates/core/src/control_plane_client.rs, crates/core/src/notify.rs, crates/server/src/config.rs, src/cli_app.rs, src/commands/doctor.rs, src/commands/usage.rs, src/commands/route_view.rs, crates/tui/src/tui/attached.rs, crates/tui/src/tui/model.rs, apps/desktop/src-tauri/src/commands/admin_api.rs, apps/desktop/src/lib/api/admin-read-model.ts, apps/desktop/src/lib/api/admin-types.ts, apps/desktop/src/lib/api/admin-client.ts, apps/desktop/src/lib/tauri/commands.ts, apps/desktop/scripts/desktop-contracts.mjs, apps/desktop/scripts/check-admin-read-model-contract.mjs.
- **Approach:** Capture config/route/catalog/pricing/policy/ledger revisions once and build typed safe sections with freshness/status. Make local JSON CLI, TUI, desktop, and fleet consume that model. Remove all remote POST/PUT/PATCH/DELETE routes and client commands. Pin trusted origins, disable redirect following, and prevent discovery from replacing configured authority or receiving credentials.
- **Execution note:** Add typed contract and no-side-effect fixtures before removing each client-specific assembly path.
- **Patterns to follow:** ControlPlaneClient, require_admin_access, dashboard DTOs, desktop contract generation, attached TUI status state, and ADR-0001.
- **Test scenarios:** coherent revision under reload; ready/stale/disconnected/auth states; no local fallback; CLI/TUI/desktop parity; nested TypeScript drift; safe redaction; only GET/HEAD route inventory; remote mutation absent; redirect/cross-origin/private/link-local/userinfo/query targets receive no token; TUI refresh only re-reads the model.
- **Verification:** Every operator surface uses one typed redacted bundle, remote tokens have no write path, and no discovery/client path can move trust or leak credentials.

### U12. Remove residual legacy surfaces and certify

- **Goal:** Remove only residual superseded paths, update documentation, and prove the repository has one runtime truth.
- **Requirements:** R1-R31; covers AE18 and final coverage for AE1-AE17.
- **Dependencies:** U1-U11.
- **Files:** crates/core/src/config_v2.rs, crates/core/src/config_v4.rs, crates/core/src/codex_models_cache.rs, crates/core/src/usage_forecast.rs, crates/core/src/usage_balance.rs, crates/core/src/proxy/request_routing.rs, crates/core/src/proxy/attempt_target.rs, crates/core/src/proxy/provider_orchestration.rs, crates/core/src/proxy/control_plane_routes/, crates/core/src/codex_patch_plan.rs, apps/desktop/src/lib/api/admin-client.ts, README.md, README_EN.md, docs/CONFIGURATION.md, docs/CONFIGURATION.zh.md, docs/adr/0001-central-relay-container-runtime.md, relevant docs/workstreams/ completion/status documents.
- **Approach:** Delete any residual file only when its replacement unit has removed all callers. Rewrite user/developer docs around the existing version 5 config, provider-scoped GPT-5.6 facts, SQLite runtime state, durable lifecycle, WS binding, observation policy, read-only control plane, offline states, and minimal local switch. Mark superseded historical workstreams as historical instead of rewriting their past decisions.
- **Execution note:** U12 is an audit and documentation unit, not a dumping ground for incomplete deletions from prior units.
- **Patterns to follow:** Existing bilingual docs structure and source-surface CI checks.
- **Test scenarios:** Source scans find no internal V2/V4 config/runtime type, migration reader/command, legacy route selection/target, JSONL accounting reader, JSON policy reader, forecast engine, usage_balance module, auth facade, Codex cache/SQLite mutation, remote write route, duplicate operator mapper, or raw sensitive DTO path. External version 5 and protocol version labels remain.
- **Verification:** All Verification Contract gates pass from a clean checkout and documentation describes only current behavior without migration promises or a new helper schema version.

---

## Verification Contract

| Gate | Command or check | Coverage |
| --- | --- | --- |
| Focused core | cargo nextest run --locked -p codex-helper-core --no-fail-fast | Config, usage, catalog, route, SQLite lifecycle, WS, policy, security, and operator behavior. |
| Rust format | cargo fmt --all -- --check | Workspace formatting. |
| Rust static analysis | cargo clippy --locked --workspace --all-targets -- -D warnings | Type, ownership, dead-code, and unsafe residual checks. |
| TUI | cargo nextest run --locked -p codex-helper-tui --no-fail-fast | Attached read model and explicit offline states. |
| Desktop contracts | pnpm --dir apps/desktop check:contracts | Rust/TypeScript nested schema, enum, and optionality parity. |
| Desktop tests | pnpm --dir apps/desktop test -- --run | Mapper, redaction, state, and interaction behavior. |
| Desktop build | pnpm --dir apps/desktop build | Production TypeScript/Vite build. |
| Full workspace | cargo nextest run --locked --workspace --no-fail-fast | Cross-crate behavior after deletions. |
| Fresh helper smoke | Start from a clean helper home with version 5 config, then exercise HTTP, SSE, WS fallback/reuse, reload, and attached reads. | End-to-end startup and protocol behavior. |
| Crash/failpoint matrix | Exercise store-open, pending insert, upstream result, terminal commit, recovery, and policy transaction failure points. | Exact-once and last-known-good invariants. |
| Method/trust audit | Enumerate control-plane routes and HTTP methods; inspect redirect/token behavior. | Read-only remote surface and origin pinning. |
| Source-surface audit | Repository search for internal historical types/readers/executors and deleted ownership violations while allowlisting external protocol/version contracts. | Precise deletion proof without removing valid version labels. |

---

## Definition of Done

- The existing ~/.codex-helper/config.toml version = 5 contract remains valid; no new project schema/storage version is introduced.
- Rust config and route-domain names are semantic and unversioned; V2/V4 compatibility, migration, and legacy runtime execution paths are gone.
- Sol, Terra, and Luna provider facts, request dialects, cache evidence, standard/priority prices, and relay ETags are provider-scoped and current.
- HTTP, SSE, and WS use one immutable RuntimeSnapshot and one durable logical-request/upstream-attempt lifecycle.
- state/state.sqlite opens and recovers before listener bind and is the request ledger, policy, affinity, and revision authority.
- Terminal success, economics, health, and policy publication happen only from committed events; crash/retry/reload cannot duplicate or drift completed facts.
- Responses WebSocket preserves real pre-upgrade HTTP fallback, binds one endpoint, processes every response.create independently, and never persists transient turn state.
- Provider observations are identity-scoped, ordered, durable, and separate from passive health, cost, budget, and manual control.
- CLI, TUI, desktop, and fleet consume one typed redacted OperatorReadModel; the remote control plane exposes GET/HEAD only.
- Explicit local switch on/off modifies only Codex config.toml and safely handles external edits; Codex auth, model cache, and SQLite are untouched.
- Forecast, test-only usage balance, JSONL/JSON accounting readers, auth facades, cache invalidation, remote mutations, and all replaced compatibility code are deleted.
- All Verification Contract gates pass. Any unavailable platform check is documented rather than assumed green.
- Abandoned experiments and superseded implementation attempts are removed before completion; the final diff contains only the chosen design.

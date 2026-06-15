# Task Ledger

## RSO-010 Contract Baseline

- Status: proposed
- Owner: main
- Scope: core proxy attempt/result types and request ledger tests.
- Goal: define `AttemptOutcome`, `CandidateSkip`, and `RequestObserver`
  without changing route graph policy.
- Validation: non-stream and stream tests prove exactly-once publication.

## RSO-020 Scheduler Runtime Snapshot

- Status: completed
- Owner: main
- Scope: route candidate selection and runtime availability state.
- Goal: expose one availability snapshot containing cooldown, usage
  exhaustion, passive health, configured/effective concurrency limit,
  active count, and saturation.
- Validation: saturated candidates skip without failure penalty and all
  unavailable paths report structured dominant reasons.
- Handoff: `RoutePlanCandidateRuntimeSnapshot` is now the single runtime
  availability source for selection helpers, structured skip reasons, routing
  explain candidate capacity, and routing explain candidate availability.
  `/routing/explain` candidates expose `availability` with available/runtime
  available, hard-unavailable, usage, breaker/cooldown, missing-auth, and
  concurrency active/limit fields plus `dominant_reason`. TUI and GUI routing
  previews render `availability=...` next to capacity and skip reasons.

## RSO-030 Upstream Throttle Outcome Integration

- Status: proposed
- Owner: main
- Scope: response classification, retry policy, cooldown updates, route attempt
  logs.
- Goal: make `upstream_rate_limited` and `upstream_overloaded` flow through the
  same outcome path for stream and non-stream attempts.
- Validation: `429`, `503`, `529`, retry-after headers, and quota/capacity
  bodies produce policy-consistent retry/failover behavior.

## RSO-040 Session Metrics Surface

- Status: completed
- Owner: main
- Scope: session identity cards, active/finished request snapshots, TUI/GUI/API
  session views.
- Goal: expose last and aggregate token usage plus output token-per-second
  metrics from core snapshots.
- Validation: TUI snapshot tests cover session rows and details when metrics
  are present or absent.
- Handoff: `SessionStats` and `SessionIdentityCard` now expose
  `last_output_tokens_per_second` and `avg_output_tokens_per_second`; TUI
  dashboard/session views and GUI session details render those fields from core
  snapshots instead of recomputing them in UI code.

## RSO-050 Operator Capacity Surface

- Status: completed
- Owner: main
- Scope: routing explain, admin provider/endpoint summaries, TUI/GUI provider
  views.
- Goal: show configured limit, effective limit, limit group, active count,
  saturated flag, cooldown reason, and retry-after source.
- Validation: route explain and UI formatting tests render
  `concurrency_saturated(active=N/limit=M)` and configured/effective limit
  fields.
- Handoff: `ProviderCapacity` now backs `/providers`, operator summary provider
  payloads, and routing explain candidates. GUI attached provider runtime shows
  provider/endpoint capacity, while TUI and GUI routing previews show candidate
  active/limit/group/saturation from routing explain. Provider rows only
  aggregate active/limit when there is a single endpoint or all endpoints share
  the same limit group and effective limit; otherwise endpoint rows remain the
  authoritative view. Cooldown reason and retry-after source stay assigned to
  RSO-020/RSO-030 so they flow through the unified runtime/outcome model.

## RSO-060 Cleanup And Documentation

- Status: proposed
- Owner: main
- Scope: duplicate metric/log paths and operator documentation.
- Goal: remove ad hoc stream/non-stream observation duplication after the
  observer becomes authoritative.
- Validation: targeted nextest suites pass, docs describe failover versus
  queue/reject behavior, and historical log compatibility remains readable.

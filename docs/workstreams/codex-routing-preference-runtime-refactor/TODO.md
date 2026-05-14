# TODO: Codex Routing Preference Runtime

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- [x] User route preference must outrank automatic affinity.
- [x] Default affinity should not permanently stick to fallback.
- [x] Explicit manual pins remain stronger than automatic preference.
- [x] Existing v4 configs must keep loading.
- [x] Station is migration-only in the next runtime.
- [x] New pins target routes, providers, or endpoints, not stations.

## Open Questions

- [x] Confirm whether the next persisted schema is `version = 5` or whether
  station retirement ships as a behavior-only breaking change.
- [x] What should the default fallback TTL be for local users with high cache
  sensitivity?
- [x] Should preferred reprobe happen on every request after TTL, or be
  rate-limited per session/provider group?
- [x] Should trusted balance exhaustion skip a preferred group for the whole
  session or only for the current request window?
  - Accepted: only for the current request/refresh window. Trusted exhaustion
    is provider-endpoint runtime state, not a session-level permanent skip.

## WS0 - Baseline And Reproduction

- [x] RPR-000 Add a regression fixture for monthly-first fallback stickiness.
- [x] RPR-001 Capture current request/control trace fields for the regression.
  - Closed by endpoint-first request attempt assertions and control-trace
    parsing for `route_graph_selection_explain`; the old station-first fields
    are not preserved as target behavior.
- [x] RPR-002 Add an explain snapshot showing fallback selected with
  higher-priority monthly providers viable.
  - Closed by the v5 routing explain API contract and the completion audit
    evidence map; fallback selection now reports provider endpoint,
    preference group, and structured skip reasons.
- [x] RPR-003 Document the current behavior as a compatibility baseline.
  - The compatibility baseline is documented as rejected architecture in
    `DESIGN.md`, `FEARLESS_REFACTOR.md`, and `COMPLETION_AUDIT.md`.

## WS1 - Preference Group IR

- [x] RPR-100 Add `preference_group` to route candidates.
- [x] RPR-101 Preserve route node group boundaries through nested
  `ordered-failover`.
- [x] RPR-102 Define `tag-preferred` group derivation for preferred and fallback
  children.
- [x] RPR-103 Define `conditional` group derivation after branch selection.
  - Conditional route expansion selects the matching branch first and then
    preserves that branch's group structure; covered by conditional route IR,
    explain, and proxy tests.
- [x] RPR-104 Add compiler tests for nested monthly pool plus paygo fallback.

## WS2 - Affinity Policy Model

- [x] RPR-200 Add an internal affinity policy type.
- [x] RPR-201 Implement `off`, `preferred-group`, `fallback-sticky`, and `hard`
  modes.
- [x] RPR-202 Add fallback affinity TTL.
  - session route affinity TTL is still enforced on read for global cache
    pruning.
  - route graph configs now support `fallback_ttl_ms`, which bounds how long
    a lower-priority `fallback-sticky` affinity may continue to outrank a
    currently viable higher preference group.
- [x] RPR-203 Add preferred-group reprobe window.
  - route graph configs now support `reprobe_preferred_after_ms`; once the
    fallback affinity's target-change age reaches the window, selection
    returns to the best viable higher preference group.
  - default is unset for compatibility; `preferred-group` already reprobes on
    every request because affinity is scoped inside the best available group.
- [x] RPR-204 Make session affinity identity provider-endpoint based.
- [x] RPR-205 Keep route graph key invalidation for topology changes.

## WS3 - Executor Rewrite

- [x] RPR-300 Remove route selection dependence on compatibility
  `LoadBalancer`.
  - route graph candidate selection now uses provider-endpoint attempt
    state directly and no longer groups unavoided candidates by
    compatibility station.
  - route graph attempt success/failure/penalty/usage-exhaustion now writes
    provider-endpoint runtime health directly, and `routing explain` reads
    the same provider-endpoint state as live routing.
  - route graph candidate execution no longer receives a compatibility
    `LoadBalancer`; attempt health helpers take `LoadBalancer` only as an
    optional legacy sink, while route graph attempts update provider endpoint
    runtime state through `AttemptTarget`.
  - route graph attempt accounting now uses the route plan candidate count
    instead of synthetic compatibility station upstream count.
  - route graph retry avoidance now records and applies route candidate stable
    indices through `AttemptTarget::attempt_avoid_index`; request logs expose
    these as `avoided_candidate_indices` while leaving `avoid_for_station`
    empty for route graph attempts.
  - route graph streaming usage auto-refresh now polls by provider endpoint
    identity first; station/upstream lookup remains only for legacy station
    attempts and compatibility projection updates.
- [x] RPR-301 Select by preference group before affinity.
- [x] RPR-302 Retry within the selected group before moving to fallback groups.
- [x] RPR-303 Preserve existing retry profile semantics.
  - `proxy_v4_route_graph_affinity_is_session_scoped` now asserts the
    fallback request attempts `input -> input1 -> right` and preserves the
    configured provider/upstream max-attempt values.
- [x] RPR-304 Prove manual pin and session/global override precedence.
  - control-plane tests now verify session route target overrides win over
    global route target overrides, both override persisted `manual-sticky`
    routing, and the persisted manual target applies again after runtime
    overrides are cleared.
- [x] RPR-305 Remove the station path from the new executor.
  - route graph request execution now uses a dedicated provider-endpoint
    candidate loop instead of the legacy station upstream loop.
  - `RoutePlanAttemptState` is provider-endpoint keyed for route graph
    selection; selected route attempts now carry `provider_endpoint_key` and
    `preference_group` as first-class metadata.
  - route graph health/cooldown/usage state no longer mutates synthetic
    `routing` `LbState`; covered by
    `proxy_v4_route_graph_health_does_not_write_synthetic_routing_lb_state`.
  - route graph request execution passes `legacy_lb = None` into the shared
    attempt transport/response/streaming pipeline; only the legacy station
    executor passes `Some(lb)`.
  - provider endpoint attempt targets now carry route candidate stable indices,
    so route graph retry exhaustion no longer depends on the compatibility
    station/upstream index even when compatibility metadata is still logged.
  - route graph attempt raw chain entries and control trace summaries now show
    `endpoint` / preference `group` first, with station/upstream retained as
    compatibility context instead of primary routing identity.
  - attempt target APIs now expose station/upstream only through explicit
    `compatibility_*` accessors, and `attempt_select` traces put those fields
    under a `compatibility` object while keeping provider endpoint identity at
    the top level.
  - provider endpoint attempt targets no longer require a synthetic legacy
    `routing` station key; route graph request/attempt logs omit top-level
    station/upstream identity unless explicit compatibility metadata exists.
  - request logs now serialize provider endpoint identity with
    `provider_endpoint_key` / `endpoint_id`, and route graph active/finished
    requests do not write synthetic `station_name = routing`.
  - route graph selection results now carry route candidates and provider
    endpoint keys directly; legacy `SelectedUpstream` projection is explicit
    and limited to station execution plus shadow/parity helpers.
  - route graph retry selection now returns `avoided_candidate_indices`
    instead of an `avoid_for_station` field, leaving station-scoped avoid sets
    only on the legacy station selection result.
  - `/routing/explain` now keys selected/skipped candidates by provider
    endpoint identity and omits `compatibility` for route graph candidates
    that do not have explicit legacy compatibility metadata.
  - `RuntimeUpstreamIdentity` now treats legacy station/upstream identity as
    optional compatibility metadata; v4 route graph candidates keep provider
    endpoint identity without synthesizing `routing/<index>`.
  - route graph runtime signal views no longer read station health,
    compatibility load-balancer state, or station balance snapshots unless the
    candidate has explicit legacy compatibility metadata.
  - remaining UI/API wording cleanup is tracked under RPR-604 and RPR-702; the
    route graph executor path itself no longer uses the station upstream loop.
- [x] RPR-306 Replace station pins with route/provider/endpoint pins.
  - route graph `manual-sticky` targets now resolve route refs, provider refs,
    and provider endpoint refs such as `input.fast`; `routing pin` and
    `routing set --target` validate the same target classes.
  - route graph configs now reject legacy session/global station override
    writes, expose route-target override capabilities, and support
    session/global route target override storage plus API endpoints.
  - covered by the v4 persisted control-plane test asserting
    `backup.default` session override wins over `paygo.default` global
    override and disabled endpoint targets are rejected.

## WS4 - Runtime State Re-Keying

- [x] RPR-400 Key route health/cooldown/usage state by provider endpoint
  identity.
- [x] RPR-401 Migrate old station/upstream runtime state into provider endpoint
  state when identity is unambiguous.
  - compatibility `LbState` now preserves failure/cooldown/usage/last-good
    state across provider endpoint reorder and projects it into provider
    endpoint keyed route runtime state.
  - old base-url signatures are migrated when the new provider endpoint layout
    has unique base URLs, and ambiguous layouts reset instead of guessing.
- [x] RPR-402 Migrate in-memory state across config reload by provider endpoint
  identity.
  - covered by `lb_migrates_state_when_provider_endpoint_order_changes` and
    `route_plan_runtime_state_migrates_reordered_lb_state_to_provider_endpoint_keys`.
- [x] RPR-403 Delete compatibility `last_good_index` from the new selection
  path.
- [x] RPR-404 Add tests for provider endpoint base URL changes and endpoint
  removal.

## WS5 - Config And Migration

- [x] RPR-500 Decide and document the `version = 5` schema boundary.
- [x] RPR-501 Add config loader defaults for affinity policy.
- [x] RPR-502 Extend `config migrate` if schema changes.
- [x] RPR-503 Add migration warnings for old fallback-sticky behavior.
- [x] RPR-504 Update config template and examples.
- [x] RPR-505 Add roundtrip tests for old v4 configs.
  - `old_v4_route_graph_auto_migrates_to_v5_and_preserves_endpoint_identity`
    covers `version = 4` route graph auto-migration to `version = 5`, backup
    creation, endpoint identity preservation, and recompiled runtime order.
- [x] RPR-506 Add migration tests for station-shaped configs into route graph
  config.
  - `station_shaped_v2_config_migrates_to_route_graph_without_profile_station_binding`
    covers v2 station/group migration into route graph routing, cleared profile
    station bindings, endpoint-scope warnings, and provider-endpoint identity in
    the compatibility projection.

## WS6 - Observability

- [x] RPR-600 Add selected preference group to request logs.
- [x] RPR-601 Add affinity source and mode to control trace.
- [x] RPR-602 Record skipped higher-priority groups.
- [x] RPR-603 Extend `routing explain` with preference and affinity sections.
- [x] RPR-604 Update GUI/TUI session and route detail views.
  - TUI session details now expose last route endpoint and route path, and
    label station/upstream as legacy observed fields.
  - TUI now treats `version = 5` as route graph routing, carries
    session/global route target overrides in its snapshot, and maps
    Enter/Backspace/o/O route graph actions to route target overrides instead
    of legacy station overrides.
  - TUI dashboard/header/help posture now separates route graph
    `route_target` controls from legacy station pin/override labels.
  - GUI attached/running snapshots now carry session/global route target
    overrides, route graph controls write the route target APIs instead of
    station override APIs, and session/detail/tray/overview surfaces render
    route target state separately from legacy station pins.
  - `routing explain` candidate payloads now keep provider endpoint identity at
    the top level; station/upstream compatibility data is only emitted under
    the optional `compatibility` object when explicit legacy compatibility
    metadata exists. CLI/TUI/GUI formatting reads that object only when it
    intentionally displays legacy compatibility details.
  - CLI `routing explain`, GUI request details, TUI request details, and proxy
    failure summaries now render route attempts with provider endpoint and
    preference group first; compatibility station/upstream is no longer the
    leading identity in those user-facing surfaces.
  - final public-surface wording classification keeps station wording only for
    legacy station configs, station health pages, compatibility projections,
    and historical log views.

## WS7 - Documentation And Cleanup

- [x] RPR-700 Update `docs/CONFIGURATION.md` session affinity semantics.
- [x] RPR-701 Add a migration note for operators who want old behavior.
- [x] RPR-702 Remove or rewrite outdated compatibility station wording.
  - TUI route graph footer, header, help, and session control posture no
    longer present route target overrides as station pins; remaining wording
    cleanup is mostly GUI and legacy statistics surface review.
  - GUI route graph overview, session controls, runtime summary, tray, and
    control trace summaries now use route target/provider endpoint wording for
    new control-plane state while keeping legacy station wording for station
    compatibility surfaces.
  - request details now show route graph retry avoidance as
    `avoid_candidates`, and control trace attempt selection summaries use
    `endpoint` / preference `group` before compatibility station/upstream.
  - CLI route explain text, GUI/TUI retry attempt lines, and failed proxy
    client messages are covered by endpoint-first regression tests.
  - `/routing/explain` no longer serializes legacy `station_name` and
    `upstream_index` as top-level candidate identity; route graph candidates
    without explicit legacy metadata omit `compatibility` entirely.
  - request ledger display now includes provider endpoint identity and treats
    missing `station_name` as a legacy-compatible absence instead of inventing
    a synthetic route graph station.
- [x] RPR-703 Close the workstream with accepted defaults and test evidence.
  - `COMPLETION_AUDIT.md` now maps requirements to concrete code and test
    evidence.
  - accepted defaults: `preferred-group` is default, `fallback-sticky` is
    explicit opt-in, trusted exhaustion is a provider-endpoint runtime signal
    for the current request/refresh window, and legacy station writes are
    rejected for route graph configs.
  - verification: `cargo fmt --all --check`, `cargo check --workspace
    --all-features`, core nextest, TUI nextest, and GUI nextest all passed after
    the final audit edits.

## Post-Closeout Follow-Ups

- [x] RPR-F01 Bound provider balance refresh HTTP calls and improve diagnostics.
  - balance adapter requests now apply a per-request timeout and include the
    probed origin plus adapter kind in HTTP/status/non-JSON errors.
  - manual and on-request balance refresh now reuse the proxy runtime HTTP
    client, so future outbound proxy support will cover both model requests and
    balance refresh consistently.
- [x] RPR-F02 Cap TUI-triggered graceful shutdown waiting.
  - when the TUI exits first, the proxy/admin server task now gets a short
    graceful shutdown window before remaining server work is aborted.
- [ ] RPR-F03 Design first-class outbound proxy config.
  - preferred shape: global outbound proxy profile plus provider-endpoint
    override; balance adapter override only for dashboard APIs with different
    egress needs.
  - avoid route-scoped proxy as the default abstraction: route graph should
    choose endpoints, while endpoints should own transport policy.
  - migration should preserve current environment-proxy behavior unless the
    user opts into explicit config.

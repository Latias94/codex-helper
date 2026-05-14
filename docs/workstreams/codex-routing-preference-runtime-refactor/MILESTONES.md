# Milestones: Codex Routing Preference Runtime

## P0 - Workstream Shape

- [x] Define the runtime preference problem.
- [x] Separate preference/affinity semantics from the previous route graph and
  runtime IR workstreams.
- [x] Accept that a config breaking change is allowed if it removes ambiguous
  behavior.
- [x] Decide that station is migration-only in the next runtime.
- [x] Define the target documents: design, fearless refactor notes, TODO, and
  milestones.

Acceptance:

- the workstream explains why v4 route graph alone is not enough;
- the target behavior is clear before implementation starts.

## P1 - Regression And Baseline

- [x] Add tests that reproduce fallback stickiness after monthly provider
  recovery.
- [x] Capture request log and control trace expectations for the current
  behavior.
  - closed as a compatibility baseline: the old station-first fields are
    rejected target behavior, while endpoint-first request/control traces are
    asserted by tests.
- [x] Add a failing target test for preferred-group recovery.

Acceptance:

- the current problem is reproducible without live providers;
- the target test fails for the old behavior for the right reason.

## P2 - Preference Group Compiler

- [x] Extend route candidates with preference group metadata.
- [x] Preserve group boundaries through nested routes.
- [x] Add compiler tests for ordered fallback, tag-preferred, manual sticky, and
  conditional branches.

Acceptance:

- the compiled route plan can tell which candidates belong to the preferred
  group;
- existing route order remains deterministic.

## P3 - Affinity Policy

- [x] Add the internal affinity policy model.
- [x] Implement preferred-group affinity.
- [x] Implement fallback TTL and preferred reprobe.
  - `fallback_ttl_ms` and `reprobe_preferred_after_ms` are persisted v5
    route graph fields.
  - unset defaults preserve explicit `fallback-sticky` compatibility, while
    `preferred-group` continues to reprobe on every request.
- [x] Keep old fallback-sticky behavior behind an explicit mode.

Acceptance:

- fallback success no longer becomes the default route when preferred providers
  recover;
- explicit fallback-sticky mode still supports cache-locality-heavy users.

## P4 - V5 Executor Cleanup

- [x] Stop using compatibility `LoadBalancer` state for route selection.
  - route graph selection and request execution no longer receive a
    compatibility `LoadBalancer`; remaining `LoadBalancer` usage is scoped to
    legacy station execution and compatibility projections.
  - streaming usage auto-refresh for route graph attempts now resolves the
    current upstream by provider endpoint before updating the legacy projection.
- [x] Use provider endpoint runtime identity as the primary state key.
- [x] Remove station identity from the new executor.
  - new route graph attempts use provider endpoint targets and state; remaining
    station identity is compatibility metadata in attempt/log/transport
    surfaces.
  - route graph attempt exhaustion now uses candidate stable indices and logs
    them as `avoided_candidate_indices`, leaving `avoid_for_station` to legacy
    station execution.
  - route graph attempt code now reaches station/index through explicit
    compatibility accessors, and `attempt_select` traces keep compatibility
    station/index under a `compatibility` object.
  - route graph provider endpoint targets no longer carry a required synthetic
    `routing` legacy key; active/finished requests and route attempt logs omit
    station/upstream fields when only provider endpoint identity exists.
  - route graph selection results now carry provider endpoint keys directly;
    legacy `SelectedUpstream` projection is explicit and limited to station
    execution plus parity helpers.
  - route graph selections return `avoided_candidate_indices`, leaving
    `avoid_for_station` semantics to the legacy station executor.
  - runtime upstream identity stores legacy station/upstream as optional
    compatibility metadata; ordinary route graph candidates no longer synthesize
    a `routing/<index>` identity for runtime signal lookup.
- [x] Replace station pins with route/provider/endpoint pins.
  - route graph controls reject legacy station override writes and use
    route/provider/endpoint target overrides; tests cover session-over-global
    precedence and fallback to persisted manual routing after runtime overrides
    are cleared.
- [x] Preserve retry profile behavior.
  - route graph integration tests preserve provider/upstream max-attempt
    semantics and candidate-level retry avoidance.

Acceptance:

- request execution does not depend on synthetic station `last_good_index`;
- legacy station configs work only by migrating before runtime execution.

## P5 - Config And Migration Decision

- [x] Decide whether affinity syntax requires `version = 5`.
- [x] If yes, implement deterministic v4-to-v5 migration.
- [x] Implement station-shaped config migration into route graph config.
- [x] Remove or reject station write APIs in the new public surface.
  - route graph configs reject legacy station override/spec writes and expose
    route target override endpoints for provider/endpoint targets.
- [x] Add roundtrip and backup tests for migrated configs.

Acceptance:

- operators can preview the migration;
- existing v4 configs load safely;
- old behavior has an explicit opt-in path if retained.

## P6 - Observability And Operator UX

- [x] Add preference group and affinity mode to logs.
  - route attempt raw entries and control trace attempt summaries now put
    provider endpoint and preference group ahead of compatibility station data.
  - request logs now expose `endpoint_id` and `provider_endpoint_key`, while
    `station_name` is a legacy compatibility field and may be absent for route
    graph requests.
  - `/routing/explain` omits `compatibility` for route graph candidates that
    do not have explicit legacy station metadata.
- [x] Show skipped higher-priority groups in explain output.
- [x] Update CLI, TUI, and GUI route/session views.
  - CLI route explain, GUI/TUI request retry details, and proxy failure
    summaries now render provider endpoint identity before compatibility
    station/upstream fields.
  - Route graph controls use route target wording; station wording remains for
    legacy station configs, station health pages, compatibility projections,
    and historical log views.
- [x] Add troubleshooting guidance for monthly-first routes.
  - `docs/CONFIGURATION.md` explains how to inspect `routing explain`,
    request logs, control trace, affinity mode, route target overrides,
    cooldown, unsupported model, and trusted exhaustion.

Acceptance:

- operators can answer "why did this request use fallback?";
- explain output distinguishes preference, health, cooldown, and affinity.

## Done When

- monthly-first routes return to monthly providers after fallback recovery;
- automatic affinity cannot silently outrank route graph preference;
- explicit pins still work and are visibly different from affinity;
- station state no longer drives route selection;
- migration behavior is documented and tested.

Current closeout status:

- runtime, migration, observability, UI wording, accepted defaults, and
  verification evidence are closed in `COMPLETION_AUDIT.md`.

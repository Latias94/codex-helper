# Phase 1 Implementation Plan: SLICE-001 to SLICE-005

> 中文速览：这份文档把第一阶段切片落到“改哪些模块、先后顺序是什么、兼容性怎么保、测试怎么补”的粒度。目标不是一步到位做完整控制平面，而是在尽量不打断现有 GUI/TUI/attach 模式的前提下，把 session identity、model/fast override、effective route source 和 profile 骨架先建立起来。

## Objective

This phase covers the first five slices from `TODO.md`:

- `SLICE-001` Surface a session identity card using existing observed data.
- `SLICE-002` Add session override for `model`.
- `SLICE-003` Add session override for `service_tier`.
- `SLICE-004` Add explicit source attribution for effective route values.
- `SLICE-005` Add a default profile skeleton without removing legacy station/config compatibility handling.

The goal is to improve control semantics without forcing a full schema rewrite or UI redesign in one step.

## Success Criteria

At the end of Phase 1, the system should let an operator answer:

1. Which session is this?
2. Which station/provider/upstream/model/tier/effort is it effectively using?
3. Which values came from request payload, session override, profile default, station mapping, or runtime fallback?
4. Can I change this session's model or fast mode without mutating future defaults?

## Current Anchors in the Codebase

### Core state

Current request/session observation already records:

- active request fields:
  - `session_id`
  - `cwd`
  - `model`
  - `reasoning_effort`
  - `station_name`
  - `provider_id`
  - `upstream_base_url`
  - `route_decision`
- session stats:
  - `last_model`
  - `last_reasoning_effort`
  - `last_provider_id`
  - `last_station_name`
  - `last_route_decision`

Current override storage is split into two runtime-only maps:

- `session_effort_overrides`
- `session_station_overrides`

Relevant files:

- `crates/core/src/state.rs`
- `crates/core/src/proxy/mod.rs`

### Proxy pipeline

Current proxy logic already:

- extracts `session_id`
- extracts request `reasoning.effort`
- extracts request `model`
- injects an effort override into the request body when present

Current v1 endpoints already include:

- `/__codex_helper/api/v1/snapshot`
- `/__codex_helper/api/v1/status/session-stats`
- `/__codex_helper/api/v1/overrides/session/effort`
- `/__codex_helper/api/v1/overrides/session/station`

### GUI/TUI

Current session presentation is built by merging:

- active requests
- recent finished requests
- session stats
- override maps

This merge currently produces a `SessionRow` with:

- observed last values
- `override_effort`
- `override_station_name`

Relevant files:

- `crates/gui/src/gui/pages/mod.rs`
- `crates/gui/src/gui/proxy_control.rs`
- `crates/tui/src/tui/view/pages/sessions.rs`

## Design Choice for Phase 1

### Recommended approach

Introduce a **structured session control model** internally, while keeping legacy map-based compatibility for existing consumers during this phase.

Implementation note: the shipped design ultimately converged on `SessionBinding` plus
per-dimension manual override storage, instead of a single monolithic override struct.

This is better than adding more parallel maps because Phase 1 already needs:

- `model` override
- `service_tier` override
- provenance/source metadata

If the code keeps growing one map per field, the GUI/TUI merge layer becomes harder to reason about and migration gets worse.

### Minimum new runtime structs

Recommended additions in `crates/core/src/state.rs`:

```rust
pub struct SessionControlOverride {
    pub station_name: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub updated_at_ms: u64,
    pub last_seen_ms: u64,
}

pub struct EffectiveValue {
    pub value: Option<String>,
    pub source: RouteValueSource,
}

pub enum RouteValueSource {
    RequestPayload,
    SessionOverride,
    GlobalOverride,
    ProfileDefault,
    StationMapping,
    RuntimeFallback,
}

pub struct SessionIdentityCard {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub last_model: Option<String>,
    pub last_reasoning_effort: Option<String>,
    pub last_service_tier: Option<String>,
    pub last_station_name: Option<String>,
    pub last_provider_id: Option<String>,
    pub last_upstream_base_url: Option<String>,
    pub binding_profile_name: Option<String>,
    pub binding_continuity_mode: Option<SessionContinuityMode>,
    pub last_route_decision: Option<RouteDecisionProvenance>,
    pub effective_model: Option<EffectiveValue>,
    pub effective_reasoning_effort: Option<EffectiveValue>,
    pub effective_service_tier: Option<EffectiveValue>,
    pub effective_station: Option<EffectiveValue>,
    pub effective_upstream_base_url: Option<EffectiveValue>,
    pub override_station_name: Option<String>,
    pub active_count: u64,
    pub turns_total: u64,
    pub last_status: Option<u16>,
    pub last_seen_ms: u64,
}
```

Notes:

- `EffectiveValue` is intentionally string-based in Phase 1 to keep serde and GUI code simple.
- `service_tier` can stay string-based initially rather than introducing a new public enum immediately.
- Existing `SessionStats` can remain in place for now and feed the identity card builder.

## Compatibility Strategy

Phase 1 should not break:

- current attach-mode consumers
- current GUI/TUI snapshot merge
- current legacy config/station compatibility loader

Therefore:

1. Keep existing API routes alive.
2. Keep current snapshot fields alive:
   - `session_effort_overrides`
   - `session_station_overrides`
3. Derive those legacy fields from the new structured override store.
4. Add new fields/endpoints instead of replacing old ones immediately.

## Implementation Order

## Step A - Introduce structured session control state

### Files

- `crates/core/src/state.rs`

### Changes

- Add `SessionControlOverride`.
- Add `session_control_overrides: RwLock<HashMap<String, SessionControlOverride>>`.
- Keep compatibility getters:
  - `get_session_effort_override()`
  - `get_session_station_override()`
  - `list_session_effort_overrides()`
  - `list_session_station_overrides()`
- Add new getters/setters:
  - `get_session_model_override()`
  - `set_session_model_override()`
  - `clear_session_model_override()`
  - `list_session_model_overrides()`
  - `get_session_service_tier_override()`
  - `set_session_service_tier_override()`
  - `clear_session_service_tier_override()`
  - `list_session_service_tier_overrides()`
- Add a single internal mutation helper to avoid four nearly identical code paths.

### Why first

This removes the biggest source of Phase 1 complexity: override data scattered across unrelated maps.

## Step B - Add model and service tier override injection

### Files

- `crates/core/src/proxy/mod.rs`

### Changes

- Keep `extract_model_from_request_body()` and `apply_model_override()` as the model path.
- Add:
  - `extract_service_tier_from_request_body()`
  - `apply_service_tier_override()`
- Resolve effective values in this order:
  - request payload
  - session override
  - profile default / station mapping / runtime fallback
- Inject overrides into the upstream request body only when a session override exists.
- Record both observed and effective values for later session-card building.

### Important rule

For Phase 1, `model` and `service_tier` overrides must be **session-scoped only**. No silent persistence into config files.

## Step C - Build a first-class session identity card

### Files

- `crates/core/src/state.rs`
- optional new shared helper:
  - `crates/core/src/session_identity.rs`

### Changes

- Build `SessionIdentityCard` from:
  - active requests
  - recent finished requests
  - session stats
  - structured overrides
- Keep `observed_*` and `effective_*` separate.
- Store and expose value sources.

### Recommended shape

Phase 1 should prefer a dedicated builder function rather than continuing to duplicate merge logic in GUI and TUI.

Suggested API:

```rust
pub async fn list_session_identity_cards(&self) -> Vec<SessionIdentityCard>;
```

or a pure helper that takes snapshot inputs and returns cards.

## Step D - Add v1 API surface for session identity and new overrides

### Files

- `crates/core/src/proxy/mod.rs`

### New routes

- `GET /__codex_helper/api/v1/sessions`
  - returns `Vec<SessionIdentityCard>`
- `GET/POST /__codex_helper/api/v1/overrides/session/model`
- `GET/POST /__codex_helper/api/v1/overrides/session/service-tier`

### Capability update

Extend `/__codex_helper/api/v1/capabilities` so GUI attach mode can detect these routes explicitly.

### Compatibility note

Keep `/snapshot` for this phase, but add new fields:

- `session_model_overrides`
- `session_service_tier_overrides`

This allows a gradual GUI migration.

## Step E - Refactor GUI/TUI to consume identity cards

### Files

- `crates/gui/src/gui/pages/mod.rs`
- `crates/gui/src/gui/proxy_control.rs`
- `crates/tui/src/tui/view/pages/sessions.rs`

### GUI plan

- Add `session_identity_cards` to `GuiRuntimeSnapshot`.
- Keep old fields during the migration window.
- Replace the current `build_session_rows(...)` merge path with:
  - prefer `session_identity_cards` when available
  - fallback to legacy merge if attached proxy is older

### TUI plan

- Reuse the same card-based representation.
- Update display lines to prefer:
  - effective model
  - effective service tier
  - effective effort
  - binding station/profile
- Keep old last-seen details as secondary context.

### Why this matters

This is the real semantic upgrade. Without it, model/tier overrides would exist but still be hard to understand in the UI.

## Step F - Add profile skeleton without replacing legacy station/config compatibility

### Files

- `crates/core/src/config.rs`
- `crates/gui/src/gui/config.rs`

### Phase 1 scope

Only add the minimum profile layer needed to support future defaults.

Recommended additions:

```rust
pub struct CodexProfile {
    pub station: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
}

pub struct CodexProfileConfig {
    pub default_profile: Option<String>,
    pub profiles: BTreeMap<String, CodexProfile>,
}
```

### Rules

- Loading profiles must be optional.
- Legacy `configs` input remains readable while station-first output becomes canonical.
- No immediate removal of existing `active` / `active_group` semantics.
- Profiles are used only as a skeleton in Phase 1:
  - define default intent
  - do not yet replace all routing selection logic

### GUI config bridge

Legacy GUI routing presets are no longer a target state. Phase 1 should converge the GUI onto real profiles:

- legacy `RoutingProfile` UI/runtime flow is retired
- GUI control entry points should target real profiles, not just pinned config
- `gui.toml` should stop carrying a separate routing-preset layer

## Detailed Slice Breakdown

## SLICE-001 - Session identity card

### Deliverable

- Session cards shown in GUI/TUI
- New v1 `/sessions` endpoint

### Minimal file touches

- `crates/core/src/state.rs`
- `crates/core/src/proxy/mod.rs`
- `crates/gui/src/gui/proxy_control.rs`
- `crates/gui/src/gui/pages/mod.rs`
- `crates/tui/src/tui/view/pages/sessions.rs`

### Suggested commit boundary

- "feat(core): add session identity card snapshot"

## SLICE-002 - Session model override

### Deliverable

- Runtime storage
- API endpoint
- GUI apply/clear action
- Proxy body injection

### Suggested commit boundary

- "feat(proxy): add session model override"

## SLICE-003 - Session service tier override

### Deliverable

- Runtime storage
- API endpoint
- GUI apply/clear action
- Proxy body injection

### Suggested commit boundary

- "feat(proxy): add session service tier override"

## SLICE-004 - Source attribution

### Deliverable

- `RouteValueSource`
- effective value rendering in GUI/TUI

### Suggested commit boundary

- "feat(ui): show effective route sources"

## SLICE-005 - Profile skeleton

### Deliverable

- Optional profile schema
- default profile loader
- docs and compatibility tests

### Suggested commit boundary

- "feat(config): add codex profile skeleton"

## API Sketch

### Session identity card

```json
{
  "session_id": "abc123",
  "cwd": "G:/codes/rust/codex-helper",
  "last_model": "gpt-5.4",
  "last_reasoning_effort": "medium",
  "last_service_tier": "fast",
  "last_station_name": "right",
  "last_provider_id": "right",
  "last_upstream_base_url": "https://www.right.codes/codex/v1",
  "binding_profile_name": "daily",
  "effective_model": { "value": "gpt-5.4", "source": "session_override" },
  "effective_reasoning_effort": { "value": "low", "source": "session_override" },
  "effective_service_tier": { "value": "fast", "source": "session_override" },
  "effective_station": { "value": "right", "source": "profile_default" },
  "effective_upstream_base_url": {
    "value": "https://www.right.codes/codex/v1",
    "source": "runtime_fallback"
  },
  "active_count": 1,
  "turns_total": 8,
  "last_status": 200,
  "last_seen_ms": 1773210000000
}
```

### Session model override write

```json
{
  "session_id": "abc123",
  "value": "gpt-5.4"
}
```

Clearing is done with:

```json
{
  "session_id": "abc123",
  "value": null
}
```

The same shape should apply to service tier.

## Testing Plan

### Core

- Add unit tests for:
  - structured override store mutation
  - compatibility getters derived from the new store
  - session identity card builder
  - source attribution precedence

### Proxy

- Add tests for:
  - request body model override injection
  - request body service tier override injection
  - no mutation when no override exists

### Config

- Add tests for:
  - optional profile loading
  - legacy `configs` compatibility
  - invalid `default_profile` handling

### GUI attach compatibility

- Add tests or fixture-level checks for:
  - v1 proxy with new endpoints
  - older proxy without `/sessions`
  - fallback to legacy merge logic

## Risks and Guardrails

### Risk: premature full schema rewrite

Avoid rewriting every legacy term at once in Phase 1. Introduce profiles additively while keeping station-first public semantics canonical.

### Risk: UI depending on new API too early

GUI and TUI should support:

- preferred new session-card path
- fallback legacy path

### Risk: cross-field inconsistency

Do not implement model override, service tier override, and source attribution as three separate ad hoc representations. They should share the same structured session-control model.

## Recommended Outcome of Phase 1

If Phase 1 succeeds, the project should have a stable base for the next stage:

- replacing weak routing presets with real profiles
- introducing station runtime state and breaker logic
- preparing LAN/shared-relay-safe UI behavior

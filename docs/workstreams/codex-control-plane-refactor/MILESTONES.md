# Fearless Refactor Milestones: Codex Control Plane

> 中文速览：这些里程碑按“先建立会话语义，再补控制模板，再做站点管理和高可用，最后承接局域网共享与远程 UI”的顺序排列。每个阶段都要求能回答一个更清晰的问题，而不是只堆功能。

## Milestone Strategy

The milestones are ordered by semantic leverage:

1. Make the current system explain itself.
2. Make session control explicit.
3. Make reusable intent first-class.
4. Make station management and HA trustworthy.
5. Make the product LAN-ready without over-promising local-only features.

## Current Read

As of the current refactor state:

- `M1`, `M2`, `M5`, and the current GUI/control-plane scope of `M6` are effectively usable.
- `M3` and `M4` are substantially complete.
- `CP-000` / `CP-001` are now documented explicitly via `VOCABULARY.md`.
- The main remaining semantic gap is now the last `CP-002` / `CP-401` compatibility-only terminology/export cleanup.
- The code-side runtime/UI route is now largely station-first across core / GUI / TUI:
  - remaining `config` wording is mostly compatibility shims, persisted document concepts, tests, or historical design material
- Fast mode / priority-processing observability is no longer a blocker:
  - request logs now distinguish requested / effective / actual `service_tier`
  - recent/session observed values now prefer the actual upstream response when available
- `operator/summary` is now strong enough to act as a real operator home payload, including lightweight runtime health/failover posture

This means the workstream is past "can this direction work" and is now in a structured closeout phase.

## M0 - Vocabulary and Compatibility Baseline

### Goal

Stabilize naming and legacy compatibility before new UI or control surfaces harden the wrong abstractions.

### Deliverables

- Legacy terminology audit
- Target vocabulary:
  - station
  - profile
  - session binding
  - observed session
  - enriched session
- Migration notes for invalid or ambiguous legacy values

### Definition of Done

- The team can explain the difference between:
  - legacy config
  - station
  - profile
- The refactor no longer relies on ambiguous meanings like `active = "true"`.

## M1 - Session Identity and Effective Route

### Goal

Every active or recent session can be mapped to an effective route and a clear source-of-truth chain.

### Deliverables

- Session binding model in core state
- Effective route card in API
- Source attribution for:
  - model
  - service tier
  - reasoning effort
  - station selection
- GUI/TUI session view update

### Definition of Done

- An operator can answer:
  - which session is this
  - what route is it using
  - why is it using that route

## M2 - Session-scoped Control

### Goal

Session-level changes become explicit, complete, and operationally safe.

### Deliverables

- Session override for `model`
- Session override for `service_tier`
- Unified session override handling for `reasoning_effort`
- API/UI to apply and clear overrides
- Scope semantics documented and enforced

### Definition of Done

- Operators can change a session's model/fast/effort without accidentally rewriting global defaults.
- Resume/fork/new-session behavior is documented and implemented consistently.

## M3 - Profile-driven Control

### Goal

Reusable operator intent moves out of ad hoc pinned station choices into a first-class profile layer.

### Deliverables

- Profile schema
- Default profile support
- Session apply-profile action
- Quick switch for default profile
- Profile validation against station capabilities

### Definition of Done

- "Fast mode", "daily", and "deep think" can be represented as named profiles.
- New sessions can reliably inherit a chosen default profile.

## M4 - Station Management and HA

### Goal

Station switching and failover become trustworthy rather than incidental.

### Deliverables

- Station runtime model
- Health scoring and active probes
- Breaker state machine:
  - closed
  - open
  - half-open
- Drain mode
- Capability-aware routing filters
- Cross-station failover guardrails

### Definition of Done

- The operator can see when a station is unhealthy, drained, or breaker-open.
- Unsupported capability mismatches are separated from real health failures.
- Automatic switching is bounded by session continuity rules.

## M5 - LAN-ready Shared Relay

### Goal

The control plane becomes honest and usable for central relay deployment across LAN / Tailscale devices.

### Deliverables

- Capability distinction between observed-session data and local enrichment
- Client/device attribution in observed sessions
- Lightweight access control for non-loopback use
- UI capability gating for remote users

### Definition of Done

- Remote devices can use the shared relay and manage shared routing/session controls.
- Remote users are not misled into expecting host-local history features that do not exist for them.

## M6 - Remote-safe UI Expansion

### Goal

GUI and future WebUI can build on stable control semantics rather than inventing them.

### Deliverables

- Sessions page centered on effective route card
- Profiles/stations management views
- Remote-safe capability badges
- `operator/summary` stable enough to serve as the WebUI/attach home payload
- Optional future WebUI design starting from the same API

### Definition of Done

- GUI is no longer a thin wrapper over legacy fields.
- top-level operator context does not need to be recomposed independently by each client
- A future WebUI can be added without redefining control-plane semantics.

## Endgame Priorities

The remaining work should be driven by closeout priority rather than by the original milestone order.

### P0 - Semantic Closeout

Goal:

- reach a point where the refactor can be called semantically complete, not just usable

Scope:

- complete the remaining `CP-002` application of the documented vocabulary contract
- apply the documented `CP-000` / `CP-001` vocabulary contract consistently across the remaining surfaces
- complete the remaining `CP-002` / `CP-401` compatibility-only runtime/public rename tail
- finish the vocabulary cleanup from legacy `config` language to station-first language
- keep `config` only where it literally means persisted config/document concepts or historical design material
- reduce the remaining compatibility-only tail explicitly:
  - remove dead shims such as `active_config()` once no internal call sites remain
  - compatibility tests/assertions for legacy fields/routes
  - migration/docs examples that intentionally show legacy `configs` input
- treat runtime/operator-facing code paths as effectively closed, with the remaining work focused on docs/examples/export wording and explicit compatibility boundaries
- finish the remaining runtime/admin wording tail after the proxy routing internals closeout:
  - exported runtime/admin type naming
  - finish the last operator/admin wording cleanup on station-first surfaces
- run a stability pass after the rename:
  - attach mode
  - snapshot/recent/session payloads
  - canonical v1 routes/fields
- make config templates/docs/UI wording consistently station/provider/profile-first

Definition of done:

- internal/runtime/public UI naming is station-first
- legacy `config` wording is clearly compatibility-only
- remaining compatibility shims are explicit and intentionally narrow rather than part of the main operator path
- operators can explain station/profile/session concepts without ambiguity
- attach compatibility is still green after the rename cleanup

Why this is `P0`:

- this is the real blocker to declaring the refactor finished
- the remaining risk is semantic drift, not missing core functionality

### P1 - Maintainability and Hardening

Goal:

- reduce implementation drag before the next round of features or UI expansion

Scope:

- split oversized modules:
  - `crates/core/src/proxy/mod.rs`
  - `crates/core/src/state.rs`
    - shared public/internal state types are now split into `state/runtime_types.rs` and `state/session_identity.rs`
  - large focused test files that are becoming navigation bottlenecks
    - `crates/gui/src/gui/proxy_control/tests.rs` has been split into themed `tests/` modules with shared helpers
    - `crates/gui/src/gui/proxy_control/tests/attached_refresh.rs` has been split into themed `attached_refresh/` modules with shared helpers
    - `crates/core/src/proxy/tests/failover.rs` has been split into `failover/mod.rs`, `failover/response_semantics.rs`, and `failover/config_failover.rs`
    - `crates/core/src/proxy/tests/api_admin.rs` has been split into `api_admin/mod.rs`, `api_admin/capabilities.rs`, `api_admin/persisted_crud.rs`, `api_admin/runtime_overrides.rs`, and `api_admin/sessions.rs`
    - `crates/core/src/config.rs` tests have been split into `config/tests/` themed modules with shared helpers
  - `crates/core/src/sessions.rs` now delegates session stats cache support and tests to `sessions/stats_cache.rs` and `sessions/tests.rs`
  - `crates/core/src/sessions.rs` now delegates transcript extraction/search support to `sessions/transcript.rs`
  - `crates/core/src/config.rs` now delegates retry policy types and resolution to `config_retry.rs`
  - `crates/core/src/config.rs` now delegates v2 compile/migrate/compact helpers to `config_v2.rs`
  - `crates/core/src/config.rs` now delegates profile inheritance and station-compatibility validation to `config_profiles.rs`
  - `crates/core/src/config.rs` now delegates routing explanation types/helpers to `config_routing.rs`
  - `crates/core/src/proxy/control_plane_routes.rs` is now split into themed `control_plane_routes/` modules
  - `crates/core/src/logging.rs` now delegates control-trace parsing/write helpers to `logging/control_trace.rs`, with request-log tests moved to `logging/tests.rs`
  - `crates/gui/src/gui/pages/config_v2/editors/stations.rs` is now split into `stations/mod.rs`, `stations/member_editor.rs`, and `stations/section.rs`
  - `crates/gui/src/gui/pages/components/history_sessions.rs` is now split into `history_sessions/mod.rs`, `history_sessions/session_panels.rs`, and `history_sessions/all_by_date.rs`
  - `crates/gui/src/gui/pages/config_v2_header.rs` is now split into `config_v2_header/mod.rs`, `config_v2_header/actions.rs`, `config_v2_header/focus_targets.rs`, `config_v2_header/surface_mode.rs`, and `config_v2_header/runtime_card.rs`
  - `crates/gui/src/gui/proxy_control/attached_refresh.rs` is now split into `attached_refresh/mod.rs`, `attached_refresh/fetch.rs`, and `attached_refresh/state_apply.rs`
- keep the new observability model coherent:
  - request outcome logging
  - session/recent views
  - failover/retry traces
- tighten the operator information architecture in GUI where the semantic model is already stable

Definition of done:

- the main control-plane modules are no longer concentrated in a few oversized files
- request/route/observability behavior is easier to reason about and test
- GUI structure is cleaner without another full redesign being required first

Why this is `P1`:

- not a semantic blocker
- but it strongly affects velocity, code review cost, and future WebUI readiness

### P2 - Productization and Long-horizon Audit

Goal:

- turn the usable local control plane into a more complete operator product

Scope:

- long-horizon route provenance / audit history beyond runtime snapshot
- richer request history and route-outcome inspection
- GUI/WebUI level audit surface if it proves worthwhile
- optional future relay enhancements:
  - per-device access model
  - companion-based remote enrichment
  - advanced failover policy after first output

Definition of done:

- operators can answer not only "what happened now" but also "what happened over time"
- multi-device/shared-relay behavior is clearer and more supportable
- future GUI/WebUI work is building on stable audit/control primitives

Why this is `P2`:

- valuable, but not required to declare the current refactor complete

## Recommended Closeout Sequence

1. Finish `P0` and declare the semantic refactor closed.
2. Use `P1` to reduce codebase drag before larger UI or API changes.
3. Treat `P2` as the productization track rather than as a blocker for refactor completion.

## Exit Criteria for the Workstream

The workstream can be considered complete when:

- session identity is explicit
- session control is complete for `model`, `service_tier`, and `reasoning_effort`
- profiles replace weak routing presets
- stations expose trustworthy management and HA state
- central relay usage across LAN/Tailscale is a supported product shape

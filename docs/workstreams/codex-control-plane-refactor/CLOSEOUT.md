# Fearless Refactor Closeout Assessment: Codex Control Plane

> Historical status (superseded 2026-07-14): this closeout describes the earlier station-first mutable control plane. The current remote control plane is query-only and exposes GET/HEAD routes backed by one typed, redacted operator read model. Remote provider/profile/override/reload/shutdown/probe/refresh mutations and remote migration APIs are no longer part of the product contract. Supported legacy TOML/JSON is handled only by the local one-time startup/CLI converter before canonical version 5 typed loading.

> Quick read: the first usable control-plane refactor milestone and the `CP-002` / `CP-401` station/config semantic closeout are closed. Profile/station control, session override aggregation, session identity APIs, precedence alignment, active/passive health, same-station failover, guarded cross-station failover before first output, hybrid session lifecycle semantics, LAN/Tailscale relay boundaries, the session-card-first Sessions page, the first GUI page/helper split, the first grouped operator information architecture, the initial GUI layout refresh/design primitives, the attach compatibility matrix, and the new `operator/summary` home payload are all in place. `CP-103`, `CP-408`, `CP-601`, `CP-611`, `CP-612`, `CP-613`, and `CP-704` are closed. The vocabulary audit/mapping is explicit via `VOCABULARY.md`; runtime/operator/API/GUI/TUI surfaces are now station-first, with `config` retained only for persisted config documents, explicit legacy/v2 migration compatibility, tests, or historical design material.

## Current Position

The workstream is no longer in the "exploration" stage.

What is already materially in place:

- profile-driven control replaced weak preset semantics
- effective route visibility exists in GUI/TUI
- session overrides now have a unified aggregate API across `model` / `reasoning_effort` / `service_tier` / station
- attach-mode clients now have an operator home payload with runtime target, lightweight health/failover posture, and session/station/profile/provider catalogs
- session identity now has both list and single-session API entry points
- request-time precedence now aligns with session-card/source-attribution semantics
- request/session observability now records fast/service-tier intent vs outcome:
  - request logs capture requested / effective / actual `service_tier`
  - recent/session observed `service_tier` prefers the actual upstream response when it is reported
  - SSE completion paths now preserve the same actual-service-tier signal
- station management has explicit operator-facing APIs and GUI flows
- breaker/open/half-open/drain states exist
- active healthcheck and manual probe are implemented
- passive runtime health now augments the station-first `/status/station-health` surface, and keeps capability-mismatch failures health-neutral
- same-station failover is now explicit: exhaust eligible upstreams inside the current station before considering the next station
- cross-station failover is now guarded: disabled by default, opt-in only before first output, and still suppressed for pinned/bound sessions
- proxy routing internals, upstream selection metadata, SSE finalize bookkeeping, and retry/failover traces are now station-first in core
- runtime state storage, healthcheck execution flow, and request logging helpers are now station-first across core / GUI / TUI
- GUI/TUI runtime snapshot, tray, and page-facing models now propagate station-first `global_station_override` / `station_health` naming across the v1 control plane
- `DashboardSnapshot` now provides station-first accessors and no longer emits legacy dual fields
- core runtime health now exports `StationHealth` as the only public health type name
- public v1 request payloads now use station-first naming (`station_name` / `station_names`), and dashboard-core no longer exports `ConfigOption` / `ConfigCapabilitySummary`
- the GUI attach/runtime control path is now station-first end-to-end, including canonical v1 runtime/status routes and station-first persisted/runtime operation names
- the TUI active-station flow and Stations page module naming are now station-first as well
- session lifecycle semantics now use a hybrid policy: runtime manual overrides expire, while session bindings stay sticky by default
- remote-safe capability gating and LAN-oriented access boundaries exist
- the Sessions page now centers the operator flow on the session identity card, effective route explanation, and last route decision snapshot
- attach consumers now have an explicit compatibility matrix across full v1 snapshots, partial v1 station surfaces, and explicit pre-v1 rejection behavior
- GUI/TUI/tray/request-route summaries now use default/effective/last station wording instead of exposing `active_station`, `legacy`, or `config` labels in operator-facing text
- `operator/summary` regression coverage asserts station-first session-card, link, and capability keys while rejecting legacy `config` aliases

What is not yet true:

- post-output cross-station failover is still intentionally unsupported, and any future advanced policy remains undecided
- long-horizon route provenance audit/history beyond the runtime snapshot is still missing

## Milestone Readiness

### M0 - Vocabulary and Compatibility Baseline

Status: **complete**

Remaining gap:

- none for the CP-002 station/config semantic scope

Impact:

- `config` is no longer public control-plane language except where it literally means a persisted config document or explicit migration/compatibility material
- regression coverage now pins absence of representative legacy home-payload keys and routes

### M1 - Session Identity and Effective Route

Status: **complete for the current control-plane scope**

Remaining gap:

- no major gap inside the current session identity/effective-route scope

Impact:

- operators and external clients can now query both session lists and a single session card
- per-request route decision provenance now rides with recent finished requests
- per-session route decision provenance now rides with the session identity card
- deeper long-term audit history is still intentionally out of scope for this milestone

### M2 - Session-scoped Control

Status: **complete**

Remaining gap:

- none for the current control-surface shape

Impact:

- field coverage and API shape are already closed
- lifecycle policy is now explicit: manual overrides are runtime-scoped, while bindings remain sticky by default

### M3 - Profile-driven Control

Status: **substantially complete**

Remaining gap:

- no major product blocker in this milestone
- only indirect dependencies from M1/M4/M6 remain

Impact:

- profiles are already good enough to serve as the primary reusable intent abstraction

### M4 - Station Management and HA

Status: **complete for the current control-plane scope**

Remaining gap:

- none for the CP-401 station/config naming scope

Impact:

- manual operations are workable now
- the canonical runtime/UI model is now station-first in `dashboard_core` and the GUI control plane
- GUI session/history presentation helpers are now largely station-first and covered by `cargo nextest run -p codex-helper-gui`
- `SessionRow` and the GUI-side test/sample builders are now station-first internally
- shared/core request-session payloads are now station-first across core, GUI, and TUI
- core proxy routing internals and streaming finalize/logging flow are now station-first and covered by `cargo nextest run -p codex-helper-core`
- runtime state storage, healthcheck execution flow, and local GUI/TUI operator calls are now station-first and covered by `cargo nextest run -p codex-helper-core -p codex-helper-gui -p codex-helper-tui`
- compatibility shims and historical examples are explicit; the runtime/operator code path and public attach/API surface expose station-first canonical routes/fields with regression coverage
- retry/failover guardrails are now explicit: same-station first, cross-station opt-in only before first output

### M5 - LAN-ready Shared Relay

Status: **complete enough for the current product shape**

Impact:

- implementation shape is already aligned with LAN/Tailscale usage
- relay-shape ambiguity is closed for the current LAN/Tailscale product target

### M6 - Remote-safe UI Expansion

Status: **complete for the current GUI control-plane scope**

Remaining gap:

- no major product blocker inside the current GUI control-plane shape

Impact:

- current GUI is functional and the Sessions page now follows the session identity card model
- future UI-facing work is external/WebUI stabilization rather than station/config terminology cleanup or a missing operator workflow

## Recommended Closeout Buckets

### Bucket A - First Usable Refactor Closeout

Status: **complete**

The first usable milestone is now fully closed.

### Bucket B - Full Semantic Refactor Closeout

Status: **complete for CP-002 / CP-401 station/config semantics**

Landed closeout items:

- applied the documented vocabulary contract from `VOCABULARY.md` across runtime/operator/API/GUI/TUI surfaces
- removed public helper/export/user-facing tails around `active_config`, `/stations/config-active`, and `station_persisted_config`
- kept historical docs/examples that mention legacy `configs` explicitly scoped to compatibility or migration
- strengthened tests/assertions that verify old field names and old paths are absent from the station-first home payload
- kept attach-side fallback behavior explicit without advertising partial legacy writable surfaces

Why this bucket matters:

- it removes the remaining semantic ambiguity
- it stabilizes the API/UI language for future WebUI or external clients
- it defines the boundary between safe continuity and automated switching

## Suggested Execution Order

With semantic closeout landed, the recommended next sequence is:

1. Keep future clients on the `operator/summary` layering contract.
2. Revisit long-horizon audit/export or post-output failover only as separate product decisions.

See `MILESTONES.md` for the more explicit `P0 / P1 / P2` closeout ladder that turns this recommendation into execution priority.

## Practical Read on Distance to Done

If we split the remaining work by outcome rather than ticket count:

- **Core usable closeout**: closed
- **Full semantic closeout**: closed for the station/config semantic scope

The biggest remaining risks are no longer station/config semantic drift; they are future product decisions:

- long-horizon route provenance/audit completeness
- advanced/post-output failover boundaries
- external/client-facing API stability

In other words: the refactor is past the "can this direction work" and station/config semantic-closeout stages. Future work should be treated as product hardening or new capability design, not as unresolved CP-002 / CP-401 rename debt.

## Parallel Follow-up Track

In parallel with the remaining semantic closeout, there is now a justified maintainability/UI track:

- split the oversized GUI page modules, starting with `pages/mod.rs`
- split large focused control-plane test files once the behavior surface is stable:
  - `crates/gui/src/gui/proxy_control/tests.rs` is now split into themed `tests/` modules with shared helpers
  - `crates/gui/src/gui/proxy_control/tests/attached_refresh.rs` is now split into themed `attached_refresh/` modules with shared helpers
  - `crates/core/src/proxy/tests/failover.rs` is now split into `failover/mod.rs`, `failover/response_semantics.rs`, and `failover/config_failover.rs`
  - `crates/core/src/proxy/tests/api_admin.rs` is now split into `api_admin/mod.rs`, `api_admin/capabilities.rs`, `api_admin/persisted_crud.rs`, `api_admin/runtime_overrides.rs`, and `api_admin/sessions.rs`
  - `crates/core/src/config.rs` tests are now split into `config/tests/` themed modules with shared helpers
- split oversized shared state support types once the exported surface is stable:
  - `crates/core/src/state.rs` now delegates shared runtime/observability types to `state/runtime_types.rs` and `state/session_identity.rs`
  - `crates/core/src/sessions.rs` now delegates session stats-cache support and session tests to `sessions/stats_cache.rs` and `sessions/tests.rs`
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
- establish a cleaner operator information architecture for Sessions / Stations / Config
- allow a more intentional GUI refresh once semantics and terminology are stable enough

This track is not the main semantic blocker, but it is the right place to reduce implementation drag before a future WebUI or larger GUI redesign.

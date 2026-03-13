# Fearless Refactor TODO: Codex Control Plane

> 中文速览：这份清单按“先语义、后界面；先会话、后平台；先可解释、后自动化”的原则拆解。第一阶段不是做炫 UI，而是把 session identity、effective route、scope-aware override 和 profile/station 关系建稳。

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- Product shape: **Codex-first local control plane**
- Shared deployment: **central relay for LAN / Tailscale devices**
- Control scope default: **session-first**
- Resume policy direction: **restore existing session binding**
- Fork policy direction: **inherit existing session binding**
- Control abstraction: **profiles for reusable intent, stations for relay targets**

## Open Questions

- `[ ]` Do we want any cross-station failover after first output, even behind an advanced switch?
- `[ ]` Should remote non-host devices be able to upload optional session enrichment later via a companion mode?
- `[ ]` What is the minimal access-control model for LAN usage:
  - local token only
  - per-device token
  - loopback default + explicit LAN opt-in

See `CLOSEOUT.md` for the recommended closeout buckets and exit-gap assessment.
See `MILESTONES.md` for the current `P0 / P1 / P2` closeout priority ladder.
See `CENTRAL_RELAY.md` for the explicit LAN / Tailscale shared-relay operating model.

## WS0 - Baseline Semantics and Naming

- [ ] CP-000 Audit current terminology:
  - `config`
  - `active`
  - `pinned`
  - `override`
  - `session`
- [ ] CP-001 Define vocabulary mapping from legacy terms to target terms:
  - legacy config -> station/profile/legacy-config bridge
- [~] CP-002 Decide whether `config` remains public API language or becomes compatibility-only wording
  - [x] public attach/API surface is now station-first and canonical-v1-only
  - [~] internal runtime/public UI model is now station-first; a narrow wording/doc/export tail still remains
- [x] CP-003 Reject or migrate invalid values like `active = "true"`
- [x] CP-004 Add migration note for legacy TOML layout

## WS1 - Session Identity and Effective Route

- [x] CP-101 Add a first-class `SessionBinding` model in core state
- [x] CP-102 Add `effective route` resolution output:
  - station
  - upstream
  - model
  - service tier
  - reasoning effort
  - source attribution
- [x] CP-103 Record route decision provenance per request/session
- [x] CP-104 Expose session identity card in API
- [x] CP-105 Update GUI/TUI Sessions view to show effective route rather than only last seen fields
- [x] CP-106 Distinguish `observed session` from `enriched session` in UI and API

## WS2 - Session-scoped Control Surface

- [x] CP-201 Add session override for `model`
- [x] CP-202 Add session override for `service_tier`
- [x] CP-203 Normalize `reasoning_effort` override semantics with the same storage model
- [x] CP-204 Define override source precedence:
  - [x] aggregate session-override API exposes current apply-order contract for request fields vs station resolution
  - [x] session identity/source attribution surfaces align with the same precedence contract
- [x] CP-205 Add clear/apply/list endpoints for all session override dimensions
- [x] CP-206 Add session override expiry policy review:
  - [x] manual session overrides remain runtime-scoped with inactivity TTL
  - [x] session bindings stay sticky by default until explicit clear or proxy restart
  - [x] optional `CODEX_HELPER_SESSION_BINDING_TTL_SECS` allows operator-controlled pruning

## WS3 - Profile System

- [x] CP-301 Introduce `Profile` schema in config
- [x] CP-302 Define default profile semantics for new sessions
- [x] CP-303 Add profile inheritance / `extends`
- [x] CP-304 Add profile CRUD in local API
- [x] CP-305 Replace weak routing preset concept with profile concept in GUI config
- [x] CP-306 Support quick switch:
  - set default profile
  - apply profile to selected session
- [x] CP-307 Add validation for profile-station compatibility

## WS4 - Station Registry and HA

- [x] CP-400 Add runtime station metadata overrides for `enabled` / `level`
- [~] CP-401 Introduce explicit `Station` runtime model
  - [x] add explicit `/api/v1/stations` and `/api/v1/stations/runtime` aliases
  - [x] add persisted station config API for `active_station` / `enabled` / `level`
  - [x] add persisted station structure API for alias / members / create / delete
  - [x] add persisted provider structure API for alias / auth env refs / endpoints / create / delete
  - [x] prefer station API in attach mode with legacy `configs` fallback
  - [x] align operator-facing GUI/tray labels with station terminology where semantics are already station-first
  - [~] rename internal runtime/public UI model from `config` to `station`
    - [x] dashboard_core canonical option/capability types are now station-first
    - [x] GUI runtime snapshot / tray / Stations page now use station-first runtime model names
    - [x] GUI session/history presentation helpers now consume station-first accessors and pass `cargo nextest run -p codex-helper-gui`
    - [x] `SessionRow` internal fields and GUI test builders are now station-first
    - [x] shared/core request-session snapshot payloads are now station-first across core / GUI / TUI
    - [x] proxy routing internals, `SelectedUpstream`, SSE finalize path, and retry/failover traces are now station-first and pass `cargo nextest run -p codex-helper-core`
    - [x] runtime state storage, healthcheck execution flow, and request logging helpers are now station-first across core / GUI / TUI and pass `cargo nextest run -p codex-helper-core -p codex-helper-gui -p codex-helper-tui`
    - [~] exported type naming and the remaining wording/doc cleanup still need final cleanup
      - [x] public attach/API surface now exposes station-first snapshot fields and v1 route aliases:
        - `global_station_override`
        - `station_health`
        - `/__codex_helper/api/v1/status/station-health`
        - `/__codex_helper/api/v1/runtime/status`
        - `/__codex_helper/api/v1/runtime/reload`
        - `/__codex_helper/api/v1/overrides/global-station`
      - [x] built-in GUI attach now targets station-first v1 routes only and explicitly rejects pre-v1 attach surfaces
      - [x] GUI/TUI runtime snapshot, tray, and page-facing models now propagate `global_station_override` / `station_health` / `StationHealth`
      - [x] compatibility tests now cover the station-first v1 surface and explicit pre-v1 attach rejection
      - [~] exported type naming is largely done; the remaining internal/public cleanup tail is now narrow
      - [x] core health type now exports station-first `StationHealth`, with legacy `ConfigHealth` removed
      - [x] `DashboardSnapshot` now exposes station-first accessors and no longer carries legacy dual fields
      - [x] public v1 request payloads and dashboard-core type aliases now use station-first naming (`station_name` / `station_names`, `StationOption`, `StationCapabilitySummary`)
      - [x] routing explanation output and retry-trace observability now emit station-first field names (`active_station`, `selected_station`, `eligible_stations`)
      - [~] remaining closeout tail is now limited to wording/doc/export cleanup on a few operator/admin surfaces
- [x] CP-402 Add capability summary per station:
  - [x] supported models
  - [x] fast/service tier support
  - [x] reasoning support
- [x] CP-403 Add station states:
  - [x] enabled
  - [x] disabled
  - [x] draining
  - [x] breaker-open
  - [x] half-open
- [x] CP-404 Implement passive health scoring
- [x] CP-405 Add active healthcheck API and UI
- [x] CP-406 Add circuit breaker thresholds and cooldowns
  - [x] expose persisted retry/cooldown config API via `/__codex_helper/api/v1/retry/config`
  - [x] add GUI retry/failover operator panel with remote-safe write-back gating
  - [x] breaker threshold / cooldown transition behavior
- [x] CP-407 Add same-station upstream failover rules
- [x] CP-408 Add cross-station failover rules before first output
- [x] CP-409 Ensure unsupported model/capability mismatch does not poison health state

## WS5 - LAN-shared Product Shape

- [x] CP-501 Add explicit control-plane mode docs for central relay deployment
- [x] CP-502 Mark local-only features in API capability response
- [x] CP-503 Separate "host-local history available" from global session observability
- [x] CP-504 Add lightweight access control for non-loopback use
- [x] CP-505 Add device/client identity field in observed session records
- [x] CP-506 Add operator-facing warning when a requested local-only feature is unavailable remotely

## WS6 - GUI / Web-readiness

- [x] CP-601 Redesign Sessions page around session identity card
- [x] CP-602 Add Profiles page or Profiles section under provider management
- [x] CP-610 Add profile linked preview for station/provider resolution and fast/reasoning capability hints
- [x] CP-611 Split oversized GUI page modules:
  - [x] extract Overview page render from `pages/mod.rs`
  - [x] extract Setup page render from `pages/mod.rs`
  - [x] extract Doctor page render from `pages/mod.rs`
  - [x] extract Sessions page from `pages/mod.rs`
  - [x] extract Requests page from `pages/mod.rs`
  - [x] extract Settings page from `pages/mod.rs`
  - [x] extract Stats page render from `pages/mod.rs`
  - [x] extract Stations page render from `pages/mod.rs`
  - [x] extract Stations profile/retry operator panels from `pages/mod.rs`
  - [x] extract legacy Config form renderer from `pages/mod.rs`
  - [x] extract Config v2 main renderer from `pages/mod.rs`
  - [x] extract Config v2 station/provider/profile helper panels from `pages/mod.rs`
  - [x] extract Config raw editor from `pages/mod.rs`
  - [x] extract shared route/session presentation helpers into focused modules
  - [x] extract runtime station health/capability helpers into `pages/runtime_station.rs`
  - [x] extract profile route preview builders/catalog helpers into `pages/profile_preview.rs`
  - [x] extract retry editor helpers into `pages/retry_editor.rs`
  - [x] extract config parse/save/sync glue into `pages/config_document.rs`
  - [x] extract remote attach/admin/token helpers into `pages/remote_attach.rs`
  - [x] extract shared history/workdir/WT helpers into `pages/history_tools.rs`
  - [x] extract shared time/string/usage formatting helpers into `pages/formatting.rs`
  - [x] extract navigation shell into `pages/navigation.rs`
  - [x] extract Config page shell into `pages/config_shell.rs`
  - [x] extract remaining page/view state definitions into `pages/view_state.rs`
- [x] CP-612 Define a cleaner operator information architecture for future GUI/WebUI:
  - [x] session console
  - [x] station/health console
  - [x] config/editor workspace
  - [x] remote-safe capability surface
- [x] CP-613 Refresh GUI layout/design where needed after semantic closeout:
  - [x] reduce dense single-column operator panels
  - [x] make current binding/effective route/last decision visually distinct
  - [x] establish reusable layout/style primitives for future WebUI
- [x] CP-603 Add Stations page with:
  - [x] expose station quick switch and common station metadata in Overview / Config forms
  - [x] dedicated station-focused page for health / drain / breaker / quick switch
  - [x] Config v2 common station fields use control-plane directly when selected service matches the running/attached proxy
  - [x] Stations page persisted station controls are remote-first when the current proxy exposes station config APIs
- [x] CP-608 Add Config v2 station structure editor with remote-safe attach fallback
- [x] CP-609 Add Config v2 provider structure editor with remote-safe attach fallback
- [x] CP-604 Add "effective route source" explanation UI
- [x] CP-605 Add remote-safe capability gating in GUI
- [x] CP-606 Keep transcript/history UI usable even when only observed-session data exists
- [x] CP-607 Add retry/failover operator controls with configured-vs-resolved visibility

## WS7 - Tests, Migration, and Docs

- [x] CP-701 Add config migration tests for legacy -> v2 shape
  - [x] legacy -> v2 compile/load coverage
  - [x] v2 save preserves station/provider schema and legacy alias loading
  - [x] operator-facing migration guide/examples
- [x] CP-702 Add session binding resolution tests
- [~] CP-703 Add breaker/failover behavior tests
  - [x] runtime station `draining` / `breaker-open` routing behavior
  - [x] breaker threshold / cooldown transition coverage
- [x] CP-704 Add API compatibility tests for existing attach mode consumers
  - [x] GUI attach exercises canonical v1 station/runtime surfaces and explicitly rejects pre-v1 control planes
  - [x] broader client compatibility matrix beyond built-in GUI
- [x] CP-705 Update README docs after the first usable milestone lands
- [x] CP-706 Add operator migration guide for existing `config.toml`
- [x] CP-707 Upgrade request observability for fast/service-tier control
  - [x] request logs record requested / effective / actual `service_tier`
  - [x] recent/session observed `service_tier` prefers actual upstream response when available
  - [x] SSE completion path captures actual `service_tier` before request finalization

## Suggested First Slice

- See `PHASE1_IMPLEMENTATION.md` for the concrete module-level execution plan for the items below.
- [x] SLICE-001 Surface current session identity card using existing observed data
- [x] SLICE-002 Add session override for `model`
- [x] SLICE-003 Add session override for `service_tier`
- [x] SLICE-004 Add explicit source attribution for effective route values
- [x] SLICE-005 Add default profile skeleton without removing legacy config handling

## Definition of Ready for Implementation

- [ ] The team agrees on:
  - station vs profile naming
  - resume/fork inheritance rules
  - default failover policy
- [ ] The first API payload sketches are stable enough for GUI consumption
- [ ] Legacy compatibility plan is written down before schema churn begins

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

## WS0 - Baseline Semantics and Naming

- [ ] CP-000 Audit current terminology:
  - `config`
  - `active`
  - `pinned`
  - `override`
  - `session`
- [ ] CP-001 Define vocabulary mapping from legacy terms to target terms:
  - legacy config -> station/profile/legacy-config bridge
- [ ] CP-002 Decide whether `config` remains public API language or becomes compatibility-only wording
- [ ] CP-003 Reject or migrate invalid values like `active = "true"`
- [ ] CP-004 Add migration note for legacy TOML layout

## WS1 - Session Identity and Effective Route

- [x] CP-101 Add a first-class `SessionBinding` model in core state
- [x] CP-102 Add `effective route` resolution output:
  - station
  - upstream
  - model
  - service tier
  - reasoning effort
  - source attribution
- [ ] CP-103 Record route decision provenance per request/session
- [ ] CP-104 Expose session identity card in API
- [x] CP-105 Update GUI/TUI Sessions view to show effective route rather than only last seen fields
- [ ] CP-106 Distinguish `observed session` from `enriched session` in UI and API

## WS2 - Session-scoped Control Surface

- [x] CP-201 Add session override for `model`
- [x] CP-202 Add session override for `service_tier`
- [ ] CP-203 Normalize `reasoning_effort` override semantics with the same storage model
- [ ] CP-204 Define override source precedence:
  - request payload
  - session override
  - profile default
  - station mapping
- [ ] CP-205 Add clear/apply/list endpoints for all session override dimensions
- [ ] CP-206 Add session override expiry policy review:
  - keep TTL
  - persist binding
  - hybrid approach

## WS3 - Profile System

- [x] CP-301 Introduce `Profile` schema in config
- [x] CP-302 Define default profile semantics for new sessions
- [ ] CP-303 Add profile inheritance / `extends`
- [ ] CP-304 Add profile CRUD in local API
- [ ] CP-305 Replace weak routing preset concept with profile concept in GUI config
- [ ] CP-306 Support quick switch:
  - set default profile
  - apply profile to selected session
- [ ] CP-307 Add validation for profile-station compatibility

## WS4 - Station Registry and HA

- [ ] CP-401 Introduce explicit `Station` runtime model
- [ ] CP-402 Add capability summary per station:
  - supported models
  - fast/service tier support
  - reasoning support
- [ ] CP-403 Add station states:
  - enabled
  - disabled
  - draining
  - breaker-open
  - half-open
- [ ] CP-404 Implement passive health scoring
- [ ] CP-405 Add active healthcheck API and UI
- [ ] CP-406 Add circuit breaker thresholds and cooldowns
- [ ] CP-407 Add same-station upstream failover rules
- [ ] CP-408 Add cross-station failover rules before first output
- [ ] CP-409 Ensure unsupported model/capability mismatch does not poison health state

## WS5 - LAN-shared Product Shape

- [ ] CP-501 Add explicit control-plane mode docs for central relay deployment
- [ ] CP-502 Mark local-only features in API capability response
- [ ] CP-503 Separate "host-local history available" from global session observability
- [ ] CP-504 Add lightweight access control for non-loopback use
- [ ] CP-505 Add device/client identity field in observed session records
- [ ] CP-506 Add operator-facing warning when a requested local-only feature is unavailable remotely

## WS6 - GUI / Web-readiness

- [ ] CP-601 Redesign Sessions page around session identity card
- [ ] CP-602 Add Profiles page or Profiles section under provider management
- [ ] CP-603 Add Stations page with:
  - health
  - drain
  - breaker
  - quick switch
- [ ] CP-604 Add "effective route source" explanation UI
- [ ] CP-605 Add remote-safe capability gating in GUI
- [ ] CP-606 Keep transcript/history UI usable even when only observed-session data exists

## WS7 - Tests, Migration, and Docs

- [ ] CP-701 Add config migration tests for legacy -> v2 shape
- [ ] CP-702 Add session binding resolution tests
- [ ] CP-703 Add breaker/failover behavior tests
- [ ] CP-704 Add API compatibility tests for existing attach mode consumers
- [ ] CP-705 Update README docs after the first usable milestone lands
- [ ] CP-706 Add operator migration guide for existing `config.toml`

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

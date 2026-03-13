# Fearless Refactor Closeout Assessment: Codex Control Plane

> Quick read: the first usable control-plane refactor milestone is closed. Profile/station control, session override aggregation, session identity APIs, precedence alignment, active/passive health, same-station failover, guarded cross-station failover before first output, hybrid session lifecycle semantics, LAN/Tailscale relay boundaries, the session-card-first Sessions page, the first GUI page/helper split, the first grouped operator information architecture, the initial GUI layout refresh/design primitives, and the attach compatibility matrix are all in place. `CP-103`, `CP-408`, `CP-601`, `CP-611`, `CP-612`, `CP-613`, and `CP-704` are now closed. Remaining work is now concentrated in the last compatibility-only terminology/runtime cleanup around `CP-401`, plus explicit vocabulary audit/docs closeout.

## Current Position

The workstream is no longer in the "exploration" stage.

What is already materially in place:

- profile-driven control replaced weak preset semantics
- effective route visibility exists in GUI/TUI
- session overrides now have a unified aggregate API across `model` / `reasoning_effort` / `service_tier` / station
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

What is not yet true:

- post-output cross-station failover is still intentionally unsupported, and any future advanced policy remains undecided
- long-horizon route provenance audit/history beyond the runtime snapshot is still missing

## Milestone Readiness

### M0 - Vocabulary and Compatibility Baseline

Status: **partial**

Remaining gap:

- CP-000 terminology audit
- CP-001 legacy-to-target vocabulary mapping
- CP-002 complete rename away from `config` in internal/runtime/UI surfaces

Impact:

- not a blocker for a usable beta
- still a blocker for claiming the refactor is semantically finished

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

Status: **substantially complete**

Remaining gap:

- CP-401 final internal/runtime rename from `config` to `station`

Impact:

- manual operations are workable now
- the canonical runtime/UI model is now station-first in `dashboard_core` and the GUI control plane
- GUI session/history presentation helpers are now largely station-first and covered by `cargo nextest run -p codex-helper-gui`
- `SessionRow` and the GUI-side test/sample builders are now station-first internally
- shared/core request-session payloads are now station-first across core, GUI, and TUI
- core proxy routing internals and streaming finalize/logging flow are now station-first and covered by `cargo nextest run -p codex-helper-core`
- runtime state storage, healthcheck execution flow, and local GUI/TUI operator calls are now station-first and covered by `cargo nextest run -p codex-helper-core -p codex-helper-gui -p codex-helper-tui`
- the remaining rename work is now mostly compatibility shims, exported type aliases, and a smaller wording/doc cleanup tail; the public attach/API surface already exposes station-first canonical routes/fields with regression coverage
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
- the remaining UI-facing work is mostly terminology cleanup and future external/WebUI stabilization rather than a missing operator workflow

## Recommended Closeout Buckets

### Bucket A - First Usable Refactor Closeout

Status: **complete**

The first usable milestone is now fully closed.

### Bucket B - Full Semantic Refactor Closeout

These are the items that should land before declaring the control-plane refactor fully complete rather than merely "usable".

- CP-000 / CP-001 / CP-002 vocabulary cleanup completion
- CP-401 final rename from `config` runtime language to `station`
- explicit closeout of the remaining compatibility-only tail:
  - narrow helper aliases such as `active_config()`
  - historical docs/examples that intentionally still mention legacy `configs`
  - tests/assertions that verify old field names are absent or compatibility-only

Why this bucket matters:

- it removes the remaining semantic ambiguity
- it stabilizes the API/UI language for future WebUI or external clients
- it defines the boundary between safe continuity and automated switching

## Suggested Execution Order

If the goal is momentum with minimal rework, the recommended next sequence is:

1. Close the remaining semantic cleanup:
   - terminology cleanup
   - CP-401

See `MILESTONES.md` for the more explicit `P0 / P1 / P2` closeout ladder that turns this recommendation into execution priority.

## Practical Read on Distance to Done

If we split the remaining work by outcome rather than ticket count:

- **Core usable closeout**: closed
- **Full semantic closeout**: still a meaningful final phase remains

The biggest remaining risks are not raw implementation volume, but semantic coherence:

- long-horizon route provenance/audit completeness
- advanced/post-output failover boundaries
- external/client-facing API stability
- final vocabulary consistency across runtime, API, UI, and historical docs/examples

In other words: the refactor is already past the "can this direction work" stage, but it is not yet at the "semantics are fully closed and externally stable" stage.

## Parallel Follow-up Track

In parallel with the remaining semantic closeout, there is now a justified maintainability/UI track:

- split the oversized GUI page modules, starting with `pages/mod.rs`
- establish a cleaner operator information architecture for Sessions / Stations / Config
- allow a more intentional GUI refresh once semantics and terminology are stable enough

This track is not the main semantic blocker, but it is the right place to reduce implementation drag before a future WebUI or larger GUI redesign.

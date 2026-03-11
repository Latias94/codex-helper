# Fearless Refactor Milestones: Codex Control Plane

> 中文速览：这些里程碑按“先建立会话语义，再补控制模板，再做站点管理和高可用，最后承接局域网共享与远程 UI”的顺序排列。每个阶段都要求能回答一个更清晰的问题，而不是只堆功能。

## Milestone Strategy

The milestones are ordered by semantic leverage:

1. Make the current system explain itself.
2. Make session control explicit.
3. Make reusable intent first-class.
4. Make station management and HA trustworthy.
5. Make the product LAN-ready without over-promising local-only features.

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
  - station/config selection
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

Reusable operator intent moves out of ad hoc pinned config into a first-class profile layer.

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
- Optional future WebUI design starting from the same API

### Definition of Done

- GUI is no longer a thin wrapper over legacy fields.
- A future WebUI can be added without redefining control-plane semantics.

## Exit Criteria for the Workstream

The workstream can be considered complete when:

- session identity is explicit
- session control is complete for `model`, `service_tier`, and `reasoning_effort`
- profiles replace weak routing presets
- stations expose trustworthy management and HA state
- central relay usage across LAN/Tailscale is a supported product shape

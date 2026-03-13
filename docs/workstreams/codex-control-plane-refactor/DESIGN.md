# Fearless Refactor Design: Codex-first Control Plane

> 中文速览：这份设计文档定义的是“中心中转 + 会话控制平面”的产品形态，而不是另一个全生态代理平台。设计重点是把 `session -> binding -> station/profile -> effective route` 这条链路做成可见、可控、可解释的系统，并明确局域网共享场景下哪些能力可以天然共享，哪些只能在本机或未来 companion 模式下提供。

## Problem Statement

`codex-helper` already has a strong local proxy core:

- provider/config selection
- upstream retry and routing
- model mapping
- runtime session/request observation
- GUI/TUI surfaces for recent sessions and requests

However, the current product shape still behaves more like a local router plus observability panel than a real control plane.

The main gaps are semantic rather than mechanical:

1. The user cannot reliably answer: "Which station/provider/config is this Codex session actually using?"
2. Session overrides exist, but only partially:
   - `config`
   - `reasoning effort`
3. Override scope is runtime-only and not clearly modeled as product behavior.
4. Presets are too weak to represent a real control template:
   - no session model override
   - no fast mode / service tier
   - no explicit default vs session inheritance rules
5. LAN-shared usage is plausible, but the product shape does not yet distinguish:
   - proxy-observed session data
   - host-local transcript/session-file enrichment

## Product Definition

The target product is:

**A Codex-first local control plane for personal / small-team / LAN usage.**

It is not:

- a full `CLIProxyAPI` replacement
- a multi-account OAuth platform first
- a generic "supports everything equally" proxy

It should optimize for:

- session identity
- session continuity
- explicit operator control
- quick station switching
- transparent failover behavior
- simple local or LAN deployment

## Goals

- Make every Codex session traceable to an explicit binding and effective route.
- Allow session-scoped control for:
  - `model`
  - `service_tier` / fast mode
  - `reasoning_effort`
  - optional future `verbosity`
- Introduce structured control templates (`profiles`) rather than relying on ad hoc pinned config.
- Add provider/station management with health, drain, breaker, and recovery semantics.
- Support LAN-shared central relay usage without over-promising host-local history features to remote devices.

## Non-goals

- Rebuilding every management feature from `CLIProxyAPI`.
- Large-scale auth/account inventory management in the first iteration.
- Cross-station failover after session continuity is already established unless explicitly allowed.
- Pretending remote devices can automatically access host-local transcript/session files.

## Key Product Constraints

### 1. Session Continuity Matters

Codex traffic is not stateless in the operator's mental model, even when transport allows retries. Once a session is effectively bound to a station/provider path, routing should be sticky by default.

### 2. Scope Must Be Explicit

The control plane should not silently treat "change current session behavior" as "change future global defaults".

Default policy:

- `scope = session` for immediate overrides
- `scope = default` or `scope = profile` only when explicitly requested

### 3. Multi-device Visibility Is Uneven by Nature

A central relay can observe traffic from many devices, but only the host machine naturally sees host-local files such as `~/.codex/sessions`.

Therefore the product must distinguish:

- network-observed truth
- local enrichment

## Core Object Model

### Station

A `station` is the operator-facing abstraction for a relay target or provider entry.

Fields:

- identity (`name`)
- enablement
- upstream list
- auth source
- capability summary
- health state
- drain state
- breaker state

This is the unit used for operator switching and HA policy.

### Profile

A `profile` is a reusable control template.

Fields:

- target station
- target model
- target service tier / fast mode
- target reasoning effort
- optional verbosity
- fallback chain
- continuity/failover policy hooks

Profiles are what users should switch most often.

### Session Binding

A `session binding` represents the current control-plane attachment of a session.

Fields:

- `session_id`
- bound profile
- bound station
- effective overrides
- continuity mode
- timestamps

This is the semantic anchor for understanding and controlling a session.

### HA Policy

HA policy defines how routing behaves when a station or upstream becomes unhealthy.

Fields:

- same-station failover policy
- cross-station failover policy
- breaker thresholds
- recovery probes
- drain behavior

### Observed Session

Data obtained from proxy traffic only.

Examples:

- `session_id`
- client/device identifier
- last config/provider/upstream
- last model / effort / service tier
- request counts
- last error / last success timestamps

This must work for all connected devices.

### Enriched Session

Optional data obtained from host-local files or a future companion/agent.

Examples:

- `cwd`
- transcript path
- transcript preview
- local history metadata

This must be presented as optional enrichment, not guaranteed truth.

## Control Semantics

## Session Identity Card

For each session, the control plane should expose a compact but complete "effective route card":

- `session_id`
- client/device
- current binding
- effective station
- effective upstream
- effective model
- effective service tier
- effective reasoning effort
- continuity mode
- last route decision time
- source of each value:
  - request payload
  - session override
  - profile default
  - station mapping
  - runtime fallback

This card is the answer to: "What exactly am I controlling?"

Current implementation note:

- the GUI Sessions page is now session-card-first rather than request-row-first
- the operator view is split into:
  - session identity list
  - session identity card
  - effective route and source explanation
  - last route decision snapshot with current-vs-decided drift visibility
  - session control editor for bindings and manual overrides

## Operator Information Architecture

The GUI/WebUI should stop behaving like a flat strip of unrelated pages.

The operator-facing structure should be grouped into consoles/workspaces:

- entry:
  - overview
  - setup
- session console:
  - sessions
  - requests
  - history
  - stats
- station/health console:
  - stations
  - doctor
- config/editor workspace:
  - config
  - settings

Remote-safe capability surface is a first-class concern rather than a page-local afterthought.

Recommended rule:

- top-level navigation should expose when the current client is remote-attached
- shared control-plane surfaces stay visible:
  - session observation/control
  - station health and failover management
  - shared observed/config surfaces
- host-local surfaces stay explicitly gated:
  - transcript file access
  - local `cwd` opening
  - direct `~/.codex/sessions` browsing

Current implementation note:

- GUI top navigation is now grouped by these consoles/workspaces instead of one flat row
- remote attach state also shows a global remote-safe banner in the top navigation area
- page-level buttons still keep their own host-local disable reasons and tooltips
- GUI now has reusable console layout primitives for section cards / kv grids / muted notes, intended to survive into a future WebUI design system
- the Sessions details pane now uses these primitives to split identity, route snapshot, source explanation, quick actions, and route decision into clearer surfaces instead of one dense linear panel

### Scope Rules

Immediate changes should be modeled separately from persistent defaults.

Recommended rule set:

- Manual session override action:
  - applies now
  - runtime-scoped
  - expires after inactivity TTL
- Session profile/binding action:
  - applies now
  - stored in session binding
  - remains sticky until explicit clear or proxy restart
  - optional operator-configured binding TTL may prune dormant bindings
- Profile/default action:
  - changes future new sessions
  - does not rewrite existing session bindings unless explicitly requested
- Resume:
  - restore existing session binding
- Fork:
  - inherit binding by default
- New session:
  - use current default profile

## Station Management

The station control plane must support:

- enable / disable
- drain
- manual switch
- healthcheck
- circuit breaker
- half-open recovery
- capability-aware filtering

### Health Classification

Suggested routing semantics:

- transport timeout / connect errors:
  - health-negative
  - breaker candidate
- upstream `5xx`:
  - health-negative
  - breaker candidate
- `429`:
  - health-negative, shorter breaker window
- `401` / invalid auth:
  - health-negative, operator-visible hard fault
- unsupported model / unsupported fast mode:
  - routing mismatch, not health damage

### Failover Rules

Default expectations:

- before first output:
  - allow same-station upstream retry
  - allow cross-station failover only if explicitly permitted by profile/HA policy
- after continuity is established:
  - keep session sticky by default
  - cross-station failover disabled unless operator overrides policy

Current implementation note:

- `retry.allow_cross_station_before_first_output` is the explicit HA switch for unpinned requests
- default behavior keeps cross-station failover disabled
- curated retry profiles may opt in when their intent is explicitly failover-oriented

## LAN / Tailscale Topology

The intended shared topology is:

- one central `codex-helper` instance acts as relay + control plane
- multiple LAN / Tailscale devices send Codex traffic through it

This implies two capability tiers.

### Universally Shareable

- station/profile management
- session identity and observed route data
- health / breaker visibility
- recent requests and routing history
- session-scoped overrides

### Host-local or Companion-only

- transcript file browsing
- direct access to `~/.codex/sessions`
- local path opening
- automatic `cwd` enrichment from host-local session files

Future expansion may add a companion process on remote devices, but the base product must not depend on that.

## Configuration Shape (v2 Direction)

Current config structure mixes provider definition, active selection, and runtime intent too closely.

The target shape should separate:

- stations
- profiles
- session inheritance policy
- HA policy

Example:

```toml
version = 2

[codex]
default_profile = "daily"

[codex.stations.right]
enabled = true
base_url = "https://www.right.codes/codex/v1"
auth_token_env = "RIGHTCODE_API_KEY"

[codex.stations.vibe]
enabled = true
base_url = "https://api-vip.codex-for.me/v1"
auth_token_env = "VIBE_API_KEY"

[codex.profiles.daily]
station = "right"
model = "gpt-5.4"
reasoning_effort = "medium"
service_tier = "auto"
fallback = ["vibe"]

[codex.profiles.fast]
extends = "daily"
service_tier = "fast"
reasoning_effort = "low"

[codex.session]
new_session_profile = "daily"
resume_policy = "restore_binding"
fork_policy = "inherit_binding"
sticky_station = true

[codex.ha]
same_station_failover = true
cross_station_failover_before_first_output = true
breaker_cooldown_secs = 60
half_open_probe_requests = 1
```

## API Direction

The existing local API should evolve into control-plane-oriented endpoints.

Suggested endpoint groups:

- `/api/v1/sessions`
  - list sessions
  - get session identity card
  - set session overrides
  - inspect session binding
- `/api/v1/profiles`
  - list/create/update/delete profiles
  - set default profile
- `/api/v1/stations`
  - list/create/update/delete stations
  - health state
  - drain/open/close
  - manual probe
- `/api/v1/routing`
  - effective route decisions
  - station capability summaries
  - routing warnings
- `/api/v1/history`
  - observed request history
  - local enrichment only when available

## Migration Strategy

The refactor should be incremental.

Recommended order:

1. Make current semantics visible.
2. Add missing session override dimensions.
3. Introduce profiles without breaking legacy station/config compatibility.
4. Add station HA behavior and control surfaces.
5. Add LAN-ready presentation and lightweight access control.

## Known Risks

- Confusing legacy `config` terminology with future `station` and `profile`.
- Over-eager cross-station failover breaking session continuity.
- GUI shipping before the semantics are stable.
- Remote users expecting transcript/history features that only exist on the host.

## Immediate Cleanup Candidate

Legacy values like `active = "true"` are dangerous because they look like booleans but semantically behave like station selectors. The refactor should either reject such values explicitly or migrate them to a valid station/profile identifier.

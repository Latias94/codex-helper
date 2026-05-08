# Milestones: Codex Operator Experience

> 中文速览：优先级按“先修信任，再建事实，再做体验”的顺序排。P0 修 TUI 稳定性和观测语义；P1 做价格、余额和 GUI/TUI 可见体验；P2 再做高级策略、长周期审计和更多产品化能力。

## Milestone Strategy

The work should proceed in this order:

1. Remove UI artifacts that make the operator distrust the tool.
2. Stabilize the request/usage/trace schema.
3. Add cost and balance facts.
4. Upgrade GUI/TUI surfaces.
5. Add richer automatic switching policy controls.

## P0 - Trust and Observability Foundation

Goal:

- Make the existing product trustworthy and make future cost/policy work build on canonical facts.

### P0.1 TUI Render Hygiene

Scope:

- Add explicit full-clear invalidation for:
  - terminal resize
  - page switch
- Align Stations table highlight spacing with other stateful tables.
- Start width-aware header/footer compaction for the top status bar.
- Reduce render-time state mutation where it causes stale table behavior.

Primary files:

- `crates/tui/src/tui/mod.rs`
- `crates/tui/src/tui/view/chrome.rs`
- `crates/tui/src/tui/view/pages/stations.rs`

Acceptance:

- Moving up/down in Tab 2 Stations no longer shows repeated or stale rows.
- Resizing the terminal does not leave stale cells.
- Narrow terminals do not produce misleading top status layout.
- TUI still builds and passes package tests.

Suggested verification:

- `cargo fmt`
- `cargo nextest run -p codex-helper-tui`
- manual TUI smoke test:
  - open Tab 2
  - move selection repeatedly
  - resize terminal narrower/wider
  - switch pages repeatedly

Current implementation slice:

- Full-clear invalidation is used on page switches and terminal resize.
- Stations now synchronizes table selection and viewport offset before rendering, using the actual visible row count so up/down navigation cannot leave the selected row outside the stateful table window.
- Stateful table viewport offsets are clamped with selection state and reset on page switch/resize to avoid stale rows after data or size changes.
- Header status lines now have final display-width fitting, including CJK width, and page tabs compact to numeric tabs while preserving the selected page label on narrow terminals.

### P0.2 Usage Metrics v2

Scope:

- Extend normalized usage to include:
  - cached input tokens
  - reasoning output tokens
  - cache read input tokens
  - cache creation input tokens
  - cache creation 5m input tokens
  - cache creation 1h input tokens
- Preserve old `UsageMetrics` compatibility.
- Parse Codex/OpenAI-compatible and Anthropic-style fields.

Primary files:

- `crates/core/src/usage.rs`
- `crates/core/src/state/runtime_types.rs`
- `crates/core/src/state/session_identity.rs`
- `crates/gui/src/gui/pages/formatting.rs`
- `crates/tui/src/tui/model.rs`

Acceptance:

- Old request logs still deserialize/replay.
- New response shapes expose cache/reasoning fields.
- TUI/GUI can display old and new usage records.

Suggested verification:

- `cargo nextest run -p codex-helper-core`
- `cargo nextest run -p codex-helper-gui -p codex-helper-tui`

### P0.3 Request Trace Contract v2

Scope:

- Normalize request completion into one internal event.
- Add `trace_id` as the primary join key where missing.
- Capture attempt-level route decisions.
- Keep requested/effective/actual model and service tier separate.
- Ensure streaming and non-streaming paths produce equivalent observability.

Primary files:

- `crates/core/src/logging.rs`
- `crates/core/src/logging/control_trace.rs`
- `crates/core/src/proxy/stream.rs`
- `crates/core/src/proxy/response_finalization.rs`
- `crates/core/src/proxy/provider_execution.rs`
- `crates/core/src/state.rs`

Acceptance:

- A request detail can explain final route and intermediate attempts.
- Failover/retry traces are visible without reading raw logs.
- Streaming and non-streaming completion tests cover service tier, usage, timing, and route attempts.

Current implementation slice:

- Added structured route attempt records to request retry info so GUI/TUI do not need to infer route decisions from raw chain strings.
- Added top-level `trace_id` to control-trace entries and backfilled it from legacy `service` + `request_id` records on read.
- Updated the GUI control-trace panel to display/search by `trace_id` as the primary join key while retaining numeric request IDs as fallback.

### P0.4 Operator API DTO Alignment

Scope:

- Expose request observability v2 through the API used by GUI/TUI/attach clients.
- Keep canonical station/profile/session vocabulary.
- Make unknown/missing fields explicit.

Primary files:

- `crates/core/src/dashboard_core/*`
- `crates/core/src/proxy/control_plane_routes/*`
- `crates/gui/src/gui/proxy_control/attached_refresh/*`
- `crates/tui/src/tui/model.rs`

Acceptance:

- GUI/TUI do not need private JSONL parsing for request detail fields.
- Attach-mode clients receive the same semantic fields as local mode.
- Compatibility tests cover old and new payload shapes.

Current implementation slice:

- Added a core `RequestObservability` DTO on finished requests with canonical timing, output speed, attempt count, route-attempt count, retry/failover flags, fast-mode state, streaming state, and `trace_id`.
- Materialized this DTO when requests finish, while legacy payloads without `observability` still deserialize and derive the same view from old fields.
- Moved GUI/TUI request lists, summaries, and details to read generation time, output token speed, attempt counts, fast mode, and retry/failover flags through core request methods instead of local duplicate calculations.

## P1 - Cost, Balance, and Operator UI

Goal:

- Make the product visibly better than a basic relay by showing cost, balance, cache, speed, and route decisions clearly.

### P1.1 Pricing Engine

Scope:

- Add model price catalog.
- Add bundled seed prices and local overrides.
- Add optional sync adapter with cache metadata.
- Calculate cache-aware request costs.
- Support service-tier/provider multipliers.
- Remove duplicated UI cost math.

Primary files/modules:

- new `crates/core/src/pricing/*`
- `crates/core/src/logging.rs`
- `crates/core/src/state/runtime_types.rs`
- `crates/gui/src/gui/pages/stats_summary.rs`
- `crates/tui/src/tui/view/stats.rs`

Acceptance:

- Request detail shows cost breakdown and confidence.
- Stats rollups show cost totals when price confidence allows it.
- Unknown price is shown as unknown, not zero.
- Costs are calculated in core, not UI.

Current implementation slice:

- Core owns a bundled cache-aware model price catalog and calculates request cost with confidence labels.
- Added a read-only operator API surface at `/__codex_helper/api/v1/pricing/catalog` so GUI/TUI/attach clients can inspect the price rows, source, confidence, and cache price fields used by core cost estimates.
- Added `~/.codex-helper/pricing_overrides.toml` local model price overrides; request cost calculation and GUI/TUI pricing catalog views now use the merged operator catalog.
- Added `codex-helper pricing path/list/set/remove/sync` so local overrides can be managed through a typed CLI, including pulling `ModelPriceCatalogSnapshot` JSON from a remote operator catalog.
- Added a GUI local pricing override editor under Stats for local-running mode; attached mode remains read-only against the remote pricing catalog.
- GUI pricing catalog rows can be saved directly as local overrides while the proxy is running locally, reusing the same override validation and refresh path as the editor.
- Added an observed-unpriced model strip in the GUI pricing editor so relay aliases seen in recent requests can be turned into local override rows without retyping the model id.

### P1.2 Balance Adapter Model

Scope:

- Promote `usage_providers.rs` into first-class balance/quota adapters.
- Add balance snapshots to station/upstream runtime state.
- Keep legacy `usage_providers.json` as compatibility input.
- Separate balance exhaustion from health failure.

Primary files/modules:

- `crates/core/src/usage_providers.rs`
- new `crates/core/src/balance/*`
- `crates/core/src/lb.rs`
- `crates/core/src/state/runtime_types.rs`
- `crates/gui/src/gui/pages/stations_*`
- `crates/tui/src/tui/view/pages/stations.rs`

Acceptance:

- Stations show balance/quota/exhaustion/stale/error states.
- Route eligibility can skip exhausted upstreams without poisoning health.
- Provider balance fetch failures are visible but not treated as transport failures.

Current implementation slice:

- Added a core balance snapshot DTO with `ok`, `exhausted`, `stale`, `error`, and `unknown` states.
- Projected provider balance snapshots through the dashboard API, local GUI runtime state, attach refresh, and TUI snapshot.
- Converted PackyCode budget and YesCode profile polling into balance snapshots while keeping quota exhaustion as an LB eligibility flag, not a health failure.
- Added balance snapshot summaries to the shared station routing posture DTO so GUI/TUI auto-switch previews can mark `ok`, `exhausted`, `stale`, and `error` balance states in candidate order.
- Added shared GUI station balance summaries to the Stations list, identity summary, and balance detail section so exhausted/stale/error quota state is visible before drilling into rows.

### P1.3 GUI Request Observatory

Scope:

- Upgrade request list/detail around v2 DTO:
  - timing
  - speed
  - token usage
  - cache
  - cost
  - service tier / fast
  - route chain
  - raw sanitized trace
- Keep `codex-helper usage tail` as a compact CLI request observer over JSONL.

Primary files:

- `crates/gui/src/gui/pages/requests.rs`
- `crates/gui/src/gui/pages/components/request_details.rs`
- `crates/gui/src/gui/pages/stats_summary.rs`

Acceptance:

- A user can inspect one request and understand what happened without reading logs.
- Route chain shows skipped/failed/final providers.
- Cost and cache fields are visible when known and gracefully absent when unknown.

Current implementation slice:

- CLI `usage tail` now shows station/provider/model, service tier/fast, duration, TTFB, output speed, cache-aware token parts, and cost estimates when model/usage are available.
- CLI `usage summary` now includes cache read/create and reasoning token totals by station instead of only input/output/total tokens.
- GUI Requests list rows now surface fast mode, token/cache totals, output speed, cost confidence, retry/failover state, and route-attempt counts before opening the detail pane.

### P1.4 TUI Parity Pass

Scope:

- Show compact cache/cost/fast/balance summaries in TUI.
- Keep dense terminal UX; do not copy GUI layout literally.

Primary files:

- `crates/tui/src/tui/model.rs`
- `crates/tui/src/tui/view/pages/requests.rs`
- `crates/tui/src/tui/view/pages/sessions.rs`
- `crates/tui/src/tui/view/pages/stations.rs`
- `crates/tui/src/tui/view/pages/dashboard.rs`
- `crates/tui/src/tui/view/stats.rs`

Acceptance:

- TUI can answer the core operator questions at a glance:
  - current route
  - fast/tier
  - key token usage
  - cost when known
  - station balance/eligibility

Current implementation slice:

- TUI request details now show compact fast/tier, cache-aware usage, cost parts, generation timing, output token speed, and structured route attempts.
- TUI station details now show an explicit station routing preview with automatic candidate order, pinned target order, skipped stations, and runtime enabled/level overrides.

### P1.5 Policy Preview UX

Scope:

- Make station switching and automatic switching policies more visual and explainable.
- Add policy preview before applying risky changes.
- Show fallback order and after-first-token behavior.

Primary files/modules:

- `crates/core/src/config_retry.rs`
- `crates/core/src/config_profiles.rs`
- `crates/core/src/config_routing.rs`
- `crates/gui/src/gui/pages/retry_editor.rs`
- `crates/gui/src/gui/pages/stations_retry_panel.rs`
- `crates/tui/src/tui/view/modals.rs`

Acceptance:

- Operators can see what a policy will do before enabling it.
- Cross-station failover boundaries are explicit.
- Cost-primary and fast-first policies are understandable without reading docs.

Current implementation slice:

- Added a GUI retry policy preview that shows the resolved retry profile, upstream/provider strategy and attempt count, plus the cross-station-before-first-output boundary.
- GUI retry editing now shows a draft resolved policy before writeback and highlights whether the draft enables cross-station failover before first output.
- Added a GUI station routing preview that explains the current source (`global pin` or automatic active/level routing), candidate station order, skipped station reasons, session pin caveats, and after-first-output failover behavior.
- Added a compact TUI Settings retry policy preview that splits upstream policy, provider policy, cross-station boundary, guardrails, and cooldown behavior into readable lines.

## P2 - Productization and Long-horizon Control

Goal:

- Turn the polished local operator console into a durable product surface.

### P2.1 Request Ledger Storage

Scope:

- Evaluate SQLite after v2 request schema stabilizes.
- Add retention/export.
- Add indexed search by session/model/station/status/cost.

Acceptance:

- Long-horizon request history is fast and queryable.
- JSONL remains export/debug friendly.

### P2.2 Advanced Route Policy Engine

Scope:

- Weighted policy engine:
  - cost
  - latency
  - health
  - quota
  - capability
  - fast support
- Policy simulation.
- Optional advanced after-first-token behavior behind explicit warning.

Acceptance:

- Operators can reason about policy outcomes.
- Dangerous failover modes cannot be enabled accidentally.

### P2.3 Provider Presets and Onboarding

Scope:

- Curated provider/station templates.
- Balance adapter presets.
- Pricing multiplier presets.
- Import/export with secret redaction.

Acceptance:

- New users can configure common relay providers quickly.
- Existing users can audit exactly what a preset changes.

### P2.4 GUI Maturity

Scope:

- Dedicated pricing/balance workspace.
- Better charts after ledger schema is stable.
- Tray/quick switch integration if it fits the existing desktop model.

Acceptance:

- GUI is a daily operator console, not only a configuration editor.

### P2.5 WebUI / LAN Attach Expansion

Scope:

- Build on existing remote-safe control-plane boundaries.
- Keep host-local transcript/history features explicitly gated.
- Consider companion enrichment only after core surfaces are stable.

Acceptance:

- Remote users get honest shared control-plane capabilities.
- No UI implies remote access to host-local files unless a companion provides it.

## Recommended First Execution Slice

1. Fix TUI render hygiene:
   - resize full clear
   - Stations highlight spacing
   - top status compaction sketch
2. Add usage v2 fields and tests.
3. Add route attempt DTO and log/API compatibility.
4. Move cost calculation into core pricing engine.
5. Promote balances into first-class station status.
6. Upgrade GUI request detail and TUI request/station summaries.

## Workstream Exit Criteria

This workstream can be considered complete when:

- TUI no longer shows known stale-cell/repeated-row behavior.
- Request detail can explain route, usage, cache, timing, service tier, and cost.
- Station detail separates health, balance, quota, capability, and policy eligibility.
- Automatic switching policy is visible and safe by default.
- GUI and TUI consume the same core observability DTOs.
- Cost and balance data are confidence-labeled rather than guessed.

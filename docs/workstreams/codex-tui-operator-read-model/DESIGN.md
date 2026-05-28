# Codex TUI Operator Read Model Refactor

Status: Active
Last updated: 2026-05-28

## Intent

Make `codex-helper-core::dashboard_core` the semantic owner for operator
provider/station read models. TUI code may adapt those rows for terminal
rendering, but it should not re-derive provider identity, enablement,
active station, upstream metadata, model metadata, or continuity-domain facts
from raw config while other operator surfaces use core DTOs.

## Current Shape

- `crates/core/src/dashboard_core/station_options.rs` owns station options,
  profile options, model options, and provider endpoint options.
- `crates/tui/src/tui/model.rs` still defines its own `ProviderOption` and
  `UpstreamSummary`, then builds them directly from `ProxyConfig`.
- TUI call sites use that mirror for route menus, station pages, settings
  modals, provider tag summaries, and health/balance presentation.

## Target Shape

- Core owns the operator row facts:
  - station identity, alias, enabled state, level, active flag;
  - upstream base URL, provider id, continuity domain, auth mode;
  - sorted tags, supported models, and model mappings.
- TUI owns presentation-only concerns:
  - terminal labels, colors, truncation, and selected-row rendering;
  - any compatibility adapter needed while the existing TUI view code still
    uses the legacy `ProviderOption` name.
- Attached TUI, integrated TUI, GUI, and desktop/admin payloads should be able
  to converge on the same core-owned contract over time.

## First Slice

`ORM-100` moves TUI provider option construction onto a new core
`dashboard_core` builder for runtime `ServiceConfigManager`. The slice keeps
the public TUI field names stable, but the semantic derivation moves to core.

## Non-Goals

- Rename every TUI `ProviderOption` call site to `StationOption` in one pass.
- Redesign the runtime mutation paths.
- Change routing behavior, health checks, or persisted config formats.

## Risks

- The current TUI name `ProviderOption` really represents a legacy station.
  The first slice keeps that compatibility name to avoid a broad UI churn.
- Core DTO expansion can affect JSON payloads. Additive serde fields are
  acceptable, but removals or meaning changes are out of scope.
- Runtime config may be legacy manager-backed while newer API surfaces are
  provider-catalog-backed. Keep builders explicit about their input model.

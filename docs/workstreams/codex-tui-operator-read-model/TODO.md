# TODO: Codex TUI Operator Read Model Refactor

## Status Legend

- `[ ]` TODO
- `[~]` In progress
- `[x]` Done
- `[!]` Blocked / needs decision

## Locked Decisions

- [x] Core owns operator read-model semantics.
- [x] TUI owns terminal presentation and transitional adapters only.
- [x] Keep the first slice small enough to prove with package-level Rust gates.

## Open Questions

- [ ] When should the transitional TUI `ProviderOption` name be renamed to
  `StationOption`?
- [ ] Should attached TUI fetch the same operator summary payload as GUI once
  the in-process adapter is shrunk?

## ORM-100 - Provider/Station Option Builder

- [x] Add core-owned runtime station/provider option rows that include the
  upstream metadata currently derived in TUI.
- [x] Change TUI `build_provider_options` to delegate to the core builder and
  keep only compatibility adaptation.
- [x] Add/adjust tests so auth labels, sorted tags, model metadata, active
  station, and level sorting are proven at the core boundary.
- [x] Run and record Rust gates.

## ORM-200 - Snapshot / Operator Summary Convergence

- [~] Identify integrated TUI fields still read from raw `ProxyConfig` after
  `ORM-100`.
- [ ] Prefer `dashboard_core` operator snapshot/summary builders for those
  fields.
- [ ] Delete TUI-only derivations that duplicate core contract fields.

## ORM-300 - Naming And Deletion Pass

- [ ] Rename transitional TUI `ProviderOption` to a station-oriented type once
  downstream call sites are ready.
- [ ] Remove obsolete compatibility aliases.
- [ ] Update handoff docs and close the lane.

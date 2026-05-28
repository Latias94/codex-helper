# Codex TUI Operator Read Model Refactor - Milestones

Status: Active
Last updated: 2026-05-28

## M0 - Lane Opened

- [x] Architecture review completed for core/TUI operator surfaces.
- [x] Workstream opened with explicit core-owned read-model boundary.

## M1 - Runtime Provider/Station Rows In Core

- [x] Core dashboard builder returns the station/provider facts needed by TUI.
- [x] TUI no longer derives upstream auth/tags/model metadata directly from raw
  config.
- [x] Focused tests cover the moved semantics.

## M2 - Broader Operator Summary Convergence

- [~] Remaining TUI raw-config read-model derivations are inventoried.
- [ ] Attached/integrated TUI convergence plan is documented.

## M3 - Closeout

- [ ] Automated gates pass.
- [ ] Review findings are addressed or recorded.
- [ ] Workstream is either closed or split into follow-up lanes.

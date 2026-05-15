# Milestones: Codex TUI Operator Polish

> 中文速览：执行顺序应先补测试和不变量，再修 Usage/Routing 的真实痛点，最后整理 footer/help 和 view model。这样每一步都能降低“用户看到错误信息并做错操作”的风险。

## Milestone Strategy

Work should proceed in this order:

1. define the TUI invariants that must not regress;
2. fix the highest-risk Usage / Balance truncation and detail issues;
3. fix Routing page route-target and long-chain readability;
4. clean footer/help so page actions stay discoverable;
5. consolidate page state and view models where duplication remains.

## P0 - Baseline And Guardrails

Goal:

- Make the known TUI risks testable before doing broad polish.

Scope:

- collect normal-width and narrow-width behavior notes;
- add fixtures for long provider names, CJK labels, and balance states;
- define critical-field visibility invariants;
- identify render helpers that derive semantic facts locally.

Acceptance:

- There is a repeatable fixture for the previously reported truncation cases.
- Tests can assert that key fields remain visible or move to detail.
- The implementation plan names the render helpers that need cleanup.

Suggested verification:

- `cargo fmt`
- `cargo nextest run -p codex-helper-tui`

## P1 - Usage / Balance Daily Operator Loop

Goal:

- Make the Usage page reliable for deciding whether a provider is usable,
  exhausted, stale, or worth refreshing.

Scope:

- attention filters;
- endpoint/detail scrolling;
- atomic balance amount rendering;
- better refresh status summaries;
- narrow layout behavior for provider/balance/route impact.

Acceptance:

- Long balance texts are complete in table or detail, never misleadingly cut.
- A selected provider's endpoint rows can be inspected without leaving the page.
- Error/exhausted/stale/unknown rows can be found quickly.
- Refresh failure remains visible and non-blocking.

Suggested verification:

- `cargo nextest run -p codex-helper-tui`
- manual TUI smoke test with narrow width and failing balance refresh

## P2 - Routing Page Readability

Goal:

- Make route target and candidate-order information understandable when there
  are many providers or long aliases.

Scope:

- compact route target labels;
- folded candidate chains;
- full details for selected route/candidate context;
- immediate route preview invalidation after override changes.

Acceptance:

- The active route target is visible at narrow widths.
- Long candidate chains show a count or folded summary plus a path to detail.
- Global/session override changes update visible route preview without stale
  balance or provider text.

Suggested verification:

- `cargo nextest run -p codex-helper-tui`
- manual smoke test toggling global and session overrides

## P3 - Footer And Help

Goal:

- Keep shortcuts discoverable without crowding the bottom line.

Scope:

- page-critical footer action list;
- page-aware help overlay;
- width-aware footer segment compaction;
- help tests for hidden actions.

Acceptance:

- Footer keeps navigation and page-critical actions first.
- Secondary actions are available in help.
- Narrow terminal footers do not hide the existence of help.

Suggested verification:

- `cargo nextest run -p codex-helper-tui`

## P4 - View Model And State Cleanup

Goal:

- Reduce future TUI bugs by keeping semantic derivation out of render-only code.

Scope:

- page view models for Usage and Routing;
- selection/detail/viewport synchronization;
- duplicated row derivation cleanup;
- tests for filtering, refresh, resize, and page switch alignment.

Acceptance:

- The selected row and detail pane always refer to the same provider/route item.
- Filtering and refresh do not leave stale detail text.
- Export/report paths do not depend on terminal-truncated strings.

Suggested verification:

- `cargo nextest run --locked --workspace --features gui --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --features gui -- -D warnings`

## Exit Criteria

This workstream can be considered complete when:

- reported balance and route-target truncation classes have tests;
- Usage and Routing pages remain useful at the agreed minimum terminal width;
- footer/help behavior is page-aware and width-aware;
- selection/detail state survives refresh, resize, page switch, and filters;
- no page derives balance or route semantics from display strings.

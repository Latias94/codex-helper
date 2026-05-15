# Codex TUI Operator Polish Smoke Checklist

This checklist closes the manual portion of `TUI-504`. It must be run from a
real interactive terminal because the dashboard intentionally starts only when
both stdin and stdout are TTYs.

## Preconditions

- Build or run the current workspace revision.
- Use a config with route graph routing enabled.
- Include at least three providers:
  - one long ASCII provider name;
  - one CJK provider label;
  - one provider with an exhausted, stale, unknown, or refresh-error balance
    state.
- Keep one session available for testing session-level route target overrides.

## Command

Run the dashboard in a real terminal:

```powershell
cargo run --bin codex-helper -- --tui
```

If the local CLI uses a different service or config flag, keep the invocation
equivalent: it must enter the full-screen dashboard, not a redirected or
non-interactive run.

## Normal Width

Use a terminal at least 110 columns wide.

- Open Usage and confirm provider identity, balance/quota state, refresh
  summary, and provider detail are visible.
- Open Routing and confirm the active route target, source, candidate order,
  selected row count, balance state, and route graph detail are visible.
- Open Stations and Settings and confirm page-critical footer actions remain
  visible.
- Press `?` on Usage and Routing and confirm page-specific actions that are not
  in the footer are discoverable in help.

## Narrow Width

Resize the same terminal to roughly 76 columns, or the smallest width the
terminal profile allows while staying usable.

- Switch repeatedly between Usage, Routing, Stations, and Settings.
- Confirm CJK provider labels still identify the provider, even if surrounding
  text is compacted.
- Confirm balance amounts are either complete, such as `$0/$300.00`, or replaced
  by an explicit state. No partial currency value should appear.
- Confirm the Routing page keeps active target/source and selected provider
  visible before long candidate-chain detail.
- Confirm the footer still exposes navigation and `? help`; hidden secondary
  actions must remain available in help.

## Interaction Checks

- Trigger a balance refresh that succeeds for at least one provider.
- Trigger or retain one balance refresh failure and confirm the latest provider
  error is shown without blocking other provider states.
- Scroll provider and endpoint details on Usage.
- Toggle global route target and confirm the route preview refreshes.
- Toggle session route target and confirm the preview updates without showing a
  stale previous target.
- Reorder a route graph provider if the config is writable, then confirm the
  selected row and detail pane still describe the same provider.

## Completion Record

After running the checklist, record:

- terminal profile and size used for normal and narrow widths;
- config source or sanitized fixture name;
- pass/fail result for each section;
- any follow-up issue ID or commit hash.

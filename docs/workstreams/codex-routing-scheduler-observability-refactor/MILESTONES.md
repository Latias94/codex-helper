# Milestones

## M1 - Contracts

- `AttemptOutcome`, `CandidateSkip`, and `RequestObserver` exist in core.
- Stream and non-stream paths can publish the same outcome shape.
- Existing request log fields remain readable.

Acceptance:

- outcome publication is exactly once for success, failure, and no-usage stream
  completion.

## M2 - Scheduler State

- Candidate availability is represented by one scheduler runtime snapshot.
- Local saturation, cooldown, trusted usage exhaustion, unsupported model, and
  disabled endpoint are explicit skip reasons.
- Local saturation never mutates failure/cooldown state.

Acceptance:

- tests cover failover from saturated preferred endpoint and all-saturated
  route unavailable summaries.

## M3 - Throttle And Overload Integration

- `429`, `503`, `529`, provider quota/rate-limit bodies, overload/capacity
  bodies, and retry-after/reset hints flow through `AttemptOutcome`.
- Retry policy consumes outcome classes instead of re-parsing responses.

Acceptance:

- one-relay and two-relay edge cases terminate under configured max attempts and
  expose the final dominant reason.

## M4 - Operator Metrics

- Provider/endpoint summaries show configured limit, effective limit, limit
  group, active/limit, and saturation.
- Session summaries show token totals and output tokens per second.
- TUI, GUI, CLI, and admin APIs read metrics from core snapshots.

Acceptance:

- TUI/GUI snapshot tests cover session token/s and provider active/limit
  rendering.

## M5 - Cleanup

- Duplicate attempt logging and UI-side metric derivation are removed.
- Configuration docs describe local saturation, upstream throttling, failover,
  and optional future overflow policies.

Acceptance:

- focused nextest gates pass and the workstream evidence file is updated with
  exact commands.

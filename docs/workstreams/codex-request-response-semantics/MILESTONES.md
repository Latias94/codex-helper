# Codex Request Response Semantics - Milestones

Status: Complete
Last updated: 2026-05-22

## M0 - Scope Frozen

Exit criteria:

- Workstream docs agree on target state, non-goals, and validation gates.
- The lane explicitly preserves the rule that helper must not overwrite user-requested model,
  reasoning effort, or service tier without an explicit override.

Status: Complete.

## M1 - P1 Recovery Shipped

Exit criteria:

- Stale `previous_response_id` errors retry once on the same selected upstream.
- Session completion fills only missing fields from existing request evidence.
- Both behaviors have focused regression tests.

Status: Complete.

## M2 - P2 Observability And Repair Shipped

Exit criteria:

- Logs and request ledger can represent requested/effective/actual service tier.
- Response repair handles known gzip encoding defects and leaves normal responses untouched.
- Focused tests cover repaired body bytes and forwarded headers.

Status: Complete.

## M3 - Verified Closeout

Exit criteria:

- `cargo fmt --package codex-helper-core` passes.
- Focused nextest filters pass.
- `cargo nextest run -p codex-helper-core` passes or any failure is documented as unrelated.
- README/README_EN/CHANGELOG reflect shipped behavior.

Status: Complete.

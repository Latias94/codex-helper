# Codex Continuity Decision Refactor - Milestones

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Evidence Freeze

Exit criteria:

- Workstream docs agree on target state and non-goals.
- Domain-name equality is explicitly documented as a hint, not proof.
- First executable task is bounded and independently validatable.

Status: Done.

## M1 - Shared Continuity Decision

Exit criteria:

- HTTP request preparation and Responses WebSocket preparation use the same continuity classifier.
- Compact v1, compact v2, `compaction_trigger`, `previous_response_id`, and ordinary turn classification have focused tests.
- WebSocket compact no longer bypasses state-bound affinity policy.

Status: Done for CDC-020 and CDC-030.

## M2 - Soft Affinity Escape And Domain Policy

Exit criteria:

- Ordinary conversation turns use soft session affinity and can route to another healthy endpoint when the pinned endpoint is unavailable.
- State-bound compact follows the active affinity policy: fallback-sticky can
  bootstrap through the configured route graph, while hard remains fail-closed
  outside one known continuity domain.
- Missing affinity bootstrap is explicit policy behavior, not an accidental
  route fallback.
- Explicit `continuity_domain` is represented in config/runtime identity or the scope is split with a documented deferral.

Status: Done for CDC-040 and CDC-050.

## M3 - Official OpenAI And Operator Diagnostics

Exit criteria:

- Official OpenAI direct support is described separately from relay facade support.
- Diagnostics explain why fallback was blocked by state continuity.
- Capability/profile output does not imply WebSocket or v2 compact safety from route advertisement alone.

Status: Done for CDC-060.

## M4 - Verification And Closeout

Exit criteria:

- Targeted nextest gates pass.
- `cargo fmt --all --check` passes.
- Workstream evidence is fresh.
- Follow-ons are split or recorded in HANDOFF.md.

Status: Done for CDC-070.

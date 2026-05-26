# Codex Continuity Follow-Up Hardening - Milestones

Status: Complete
Last updated: 2026-05-26

## M0 - Scope And Release Boundary

Exit criteria:

- Workstream docs exist.
- cargo-dist release planning excludes desktop artifacts.
- The generated release workflow is in sync with cargo-dist metadata.
- Desktop local/CI builds keep their sidecar preparation path.

## M1 - Continuity Topology And Regression Shape

Exit criteria:

- Endpoint/domain counting has one owner.
- HTTP, WebSocket, and diagnostics use the same topology helper.
- Response semantics tests are split into maintainable modules.
- Targeted continuity regression tests still pass.

## M2 - Operator Configuration Surfaces

Exit criteria:

- Provider and endpoint persisted DTOs preserve `continuity_domain`.
- TUI and desktop surfaces show the current continuity domain.
- Desktop editing can submit the field without dropping existing endpoint data.
- API/UI tests cover preservation where practical.

## M3 - Diagnostics And Official OpenAI Stance

Exit criteria:

- Capability diagnostics display expected and selected continuity data.
- Older/partial diagnostics responses remain readable.
- Official OpenAI direct behavior is conservative and documented.
- Relay endpoints are never grouped by host/base URL alone.

## M4 - Final Verification

Exit criteria:

- Focused gates pass for release planning, topology, tests, API/UI, and diagnostics.
- Broad Rust gates pass.
- Frontend tests touched by this lane pass.
- Evidence and handoff are updated.
- A conventional commit is prepared after user confirmation or committed if already confirmed.

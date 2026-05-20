# Desktop Lifecycle Owner — Milestones

Status: Complete
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

Exit criteria:

- Workstream docs exist and agree on target state.
- Task ledger is decomposed into independently validatable slices.
- Non-goals explicitly exclude full Tauri/service shipping.

## M1 — Lifecycle Domain And Owner Metadata

Exit criteria:

- Lifecycle mode/owner terms are first-class core types.
- Owner marker path, serialization, stale/invalid handling, and cleanup semantics are tested.
- Manual resident and supervisor-owned resident processes can be distinguished.

## M2 — Manager Seam And Adapter Convergence

Exit criteria:

- GUI uses centralized lifecycle policy for owned-vs-attached stop behavior.
- Attached observer exit detaches only; explicit Stop can still call shutdown API.
- CLI/TUI copy and status reflect owner metadata where available.

## M3 — Desktop-Managed Sidecar Preparation

Exit criteria:

- There is an explicit DesktopOwned owner kind / managed child mode suitable for a tray/Tauri backend.
- The mode is not default and cannot silently surprise simple users.
- `daemon status` can make the running owner visible.

## M4 — Closeout

Exit criteria:

- Targeted tests and checks are recorded in EVIDENCE_AND_GATES.md.
- README and configuration docs describe the shipped behavior.
- Follow-ons are explicitly deferred or split.

Status: complete on 2026-05-20. Full Tauri/tray shell, OS service install, and live owner-marker process orchestration tests are deferred follow-ons.

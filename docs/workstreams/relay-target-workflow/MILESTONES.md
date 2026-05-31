# Relay Target Workflow - Milestones

Status: Complete
Last updated: 2026-05-31

## M0 - Scope Freeze

Status: complete.

Exit criteria:

- Durable workstream docs exist.
- Task ledger is split by vertical relay target slices.
- Evidence plan names focused Rust gates.

## M1 - Shared Target Model

Status: complete.

Exit criteria:

- Relay target config and resolution have tests.
- Control-plane admin request/auth behavior is shared.
- Existing local defaults remain unchanged.

## M2 - Daily CLI Target Flow

Status: complete.

Exit criteria:

- `ch relay local` works as an explicit local target flow.
- `ch relay <name>` supports switch-and-attach for named targets.
- `--no-tui` and `--attach-only` are parsed and tested.
- `relay add/list/status/off` produce useful operator output.

## M3 - Remote TUI Attach

Status: complete.

Exit criteria:

- Attached TUI accepts a resolved admin base URL.
- Remote admin token policy is honored.
- Remote attached mode is visibly distinct from local running mode.

## M4 - Closeout

Status: complete.

Exit criteria:

- Docs describe local, NAS, and attach-only workflows.
- Fresh validation evidence is recorded.
- Remaining risks are documented or split into follow-ons.
- Changes are committed.

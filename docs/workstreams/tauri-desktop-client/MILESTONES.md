# Tauri Desktop Client — Milestones

Status: Draft
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

Exit criteria:

- Replacement intent is explicit: Tauri is the long-term GUI replacement, not a permanent parallel UI.
- React + Tailwind + shadcn/ui and component-prototype workflow are documented.
- The simplified MVP sitemap is frozen.
- shadcn/ui prototype prompt is captured in handoff for the user.

Status: Complete.

## M1 — shadcn/ui Product Prototype

Exit criteria:

- User generates or assembles a React + Tailwind + shadcn/ui prototype from the prompt.
- The returned artifact is one of:
  - source export;
  - images;
  - preview URL;
  - enough visual detail for critique.
- The direction is accepted, revised, or rejected with reasons.

Gate:

- `impeccable critique`-style review against product UI bans and codex-helper requirements.

## M2 — Tauri Shell And Static App

Exit criteria:

- A Tauri + React + Tailwind shell exists in the repo.
- Static/mock simplified MVP can run locally.
- Existing Rust workspace checks are not broken.

Expected gates:

- Frontend install/build/typecheck command chosen for the created workspace.
- `cargo check` for any changed Rust package.
- Visual smoke by browser/Tauri capture if available.

## M3 — Read-Only Admin API Wiring

Exit criteria:

- Dashboard, API Keys, Usage, Providers, and Settings consume real admin API data when a local proxy is available.
- Disconnected and auth-token-required states are first-class.
- Mock mode still works for design iteration.

Expected gates:

- Frontend API client tests.
- Manual smoke against `codex-helper serve --resident` or a desktop-owned sidecar.
- Targeted Rust checks if admin API contracts change.

## M4 — Safe Mutations And Desktop Lifecycle

Exit criteria:

- Safe mutations work from the desktop UI.
- Tray behavior uses owner semantics.
- Attached runtime normal exit detaches without remote shutdown.
- Explicit Stop Proxy remains deliberate and visible.

Expected gates:

- Lifecycle action matrix in `EVIDENCE_AND_GATES.md`.
- Targeted frontend tests for action enablement/confirmation.
- Rust/Tauri command tests where possible.

## M5 — Replacement Readiness

Exit criteria:

- Tauri client is documented as the replacement path.
- Remaining egui gaps are named.
- Follow-on work is split for installer/signing/autoupdate/egui removal if not done.

Expected gates:

- Final targeted frontend and Rust gates.
- Workstream closeout docs updated.

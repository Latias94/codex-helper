# Tauri Desktop Client — TODO

Status: Draft
Last updated: 2026-05-20

## M0 — Scope And Evidence Freeze

- [x] TDC-010 [owner=planner] [deps=none] [scope=docs/workstreams/tauri-desktop-client]
  Goal: Freeze replacement intent, React + Tailwind + shadcn/ui direction, simplified MVP sitemap, component-prototype workflow, and evidence anchors.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Evidence: docs/workstreams/tauri-desktop-client/DESIGN.md
  Handoff: DONE — workstream opened from user-confirmed constraints on 2026-05-20.

## M1 — shadcn/ui Product Prototype

- [ ] TDC-020 [owner=user] [deps=TDC-010] [scope=external-shadcn-prototype,source-or-preview]
  Goal: Generate or assemble an initial simplified React + Tailwind + shadcn/ui product prototype with the revised workstream prompt and return either source export, images, or a preview URL.
  Validation: User confirms the generated direction is close enough to import or asks for a revised prompt.
  Review: Use impeccable critique before implementation if the generated UI looks generic, SaaS-heavy, too neon, or unclear.
  Evidence: prototype source/image/URL plus notes in EVIDENCE_AND_GATES.md.
  Handoff: Final status must be DONE, DONE_WITH_CONCERNS, BLOCKED, or NEEDS_CONTEXT.

- [ ] TDC-030 [owner=planner] [deps=TDC-020] [scope=docs/workstreams/tauri-desktop-client,frontend-design-brief]
  Goal: Convert the accepted prototype into an implementation brief: component inventory, page states, data contract map, visual tokens, and import plan.
  Validation: DESIGN.md or a new UI_BRIEF.md captures accepted decisions and rejected directions.
  Review: Confirm with user before coding.
  Evidence: UI brief and selected images or preview notes.
  Handoff: Blocks implementation until accepted.

## M2 — Tauri Shell And Static App

- [ ] TDC-040 [owner=main-or-worker] [deps=TDC-030] [scope=Cargo.toml,apps/desktop-or-crates/desktop,package.json,tauri-config]
  Goal: Add a runnable Tauri + React + Tailwind shell without wiring live proxy state yet.
  Validation: desktop dev command starts; frontend typecheck/build passes.
  Review: Ensure repo layout does not break existing Rust workspace or release flow.
  Evidence: command output in EVIDENCE_AND_GATES.md.
  Handoff: Static mock UI should be visible in Tauri.

- [ ] TDC-050 [owner=main-or-worker] [deps=TDC-040] [scope=desktop-frontend/src]
  Goal: Import and harden the accepted shadcn/ui prototype into maintainable React components with mock data for all simplified MVP pages.
  Validation: frontend lint/typecheck/build; optional visual smoke.
  Review: Use impeccable product bans: no gradient text, no decorative glass, no identical metric grid, no unsafe modal-first flows.
  Evidence: visual smoke output and frontend checks.
  Handoff: UI must remain mockable before API integration.

## M3 — Read-Only Admin API Wiring

- [ ] TDC-060 [owner=main-or-worker] [deps=TDC-050] [scope=desktop-frontend/src/api,desktop-frontend/src/pages]
  Goal: Wire read-only live data for Dashboard, API Keys, Usage, Providers, and Settings via admin API links.
  Validation: unit tests for API client mapping; Tauri/manual smoke against a running local proxy.
  Review: Prefer `/operator/summary` links and capability flags over hard-coded assumptions.
  Evidence: API client tests and smoke notes.
  Handoff: Advanced sessions/routing/diagnostics remain collapsed, disabled, or mocked until simple surfaces work.

- [ ] TDC-070 [owner=main-or-worker] [deps=TDC-060] [scope=desktop-frontend/src/state,desktop-frontend/src/components]
  Goal: Add loading, empty, disconnected, auth-token-required, and stale-runtime states.
  Validation: component/story/visual checks for key states.
  Review: Empty states must teach the next action.
  Evidence: state images or tests.
  Handoff: Must not hide attached/resident lifecycle uncertainty.

## M4 — Safe Mutations And Desktop Lifecycle

- [ ] TDC-080 [owner=main-or-worker] [deps=TDC-070] [scope=desktop-frontend/src,src/cli_app.rs-or-tauri-commands]
  Goal: Implement safe actions: attach, start desktop-owned proxy, stop owned proxy, explicit remote stop, switch on/off, reload runtime, probe station, refresh balances, set/clear overrides.
  Validation: targeted frontend tests plus Rust/Tauri command tests where practical.
  Review: Explicit Stop Proxy and ordinary Quit must remain visibly different.
  Evidence: tests and manual action matrix.
  Handoff: Dangerous actions require inline confirmation.

- [ ] TDC-090 [owner=main-or-worker] [deps=TDC-080] [scope=desktop-tauri,tray,lifecycle]
  Goal: Implement tray and desktop owner semantics using the hidden desktop-managed sidecar path.
  Validation: lifecycle smoke: close window, quit app, attach existing resident runtime, explicit stop.
  Review: No normal GUI exit may remote-stop an attached runtime.
  Evidence: lifecycle matrix in EVIDENCE_AND_GATES.md.
  Handoff: Split OS autostart/signing if scope grows.

## M5 — Replacement Readiness

- [ ] TDC-100 [owner=planner] [deps=TDC-090] [scope=docs,README,release-notes]
  Goal: Document Tauri client as replacement path for egui GUI and state remaining parity gaps.
  Validation: docs review plus targeted checks.
  Review: Do not remove egui until parity and packaging gates pass.
  Evidence: docs diff and final gate log.
  Handoff: Split follow-on work for egui removal, installer/signing, or auto-update.

- [ ] TDC-110 [owner=planner] [deps=TDC-100] [scope=docs/workstreams/tauri-desktop-client]
  Goal: Close this lane or split remaining work.
  Validation: verify-rust-workstream records fresh final evidence.
  Review: review-workstream has no blocking findings.
  Evidence: EVIDENCE_AND_GATES.md, WORKSTREAM.json
  Handoff: Summarize risks in HANDOFF.md.

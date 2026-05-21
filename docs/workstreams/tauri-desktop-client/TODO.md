# Tauri Desktop Client — TODO

Status: Draft
Last updated: 2026-05-21

## M0 — Scope And Evidence Freeze

- [x] TDC-010 [owner=planner] [deps=none] [scope=docs/workstreams/tauri-desktop-client]
  Goal: Freeze replacement intent, React + Tailwind + shadcn/ui direction, simplified MVP sitemap, component-prototype workflow, and evidence anchors.
  Validation: DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Evidence: docs/workstreams/tauri-desktop-client/DESIGN.md
  Handoff: DONE — workstream opened from user-confirmed constraints on 2026-05-20.

## M1 — Image Concept And shadcn/ui Product Prototype

- [x] TDC-015 [owner=planner] [deps=TDC-010] [scope=docs/workstreams/tauri-desktop-client/UI_BRIEF.md]
  Goal: Build a pre-imagegen UX/design brief from the user-provided reference screenshot, `repo-ref/awesome-design-md`, local product capabilities, admin API contracts, and existing GUI/TUI surfaces.
  Validation: `UI_BRIEF.md` captures visual direction, page requirements, states, data contract map, copy rules, and an imagegen prompt pack.
  Evidence: `docs/workstreams/tauri-desktop-client/UI_BRIEF.md`; `EVIDENCE_AND_GATES.md` 2026-05-21 pre-imagegen entry.
  Handoff: DONE — next visual step is image concept generation and critique before shadcn/ui prototype import.

- [x] TDC-018 [owner=planner-or-user] [deps=TDC-015] [scope=imagegen-concepts,docs/workstreams/tauri-desktop-client/UI_BRIEF.md]
  Goal: Generate first image concepts from `UI_BRIEF.md`, critique them against the acceptance checklist, and revise the brief if the direction is too generic, SaaS-heavy, too blue, too dense, unclear, or incorrectly treats API keys as a standalone top-level page.
  Progress: Dashboard, Providers, revised Usage, and revised Settings concepts are accepted. The first Settings concept was rejected as too complex; the accepted Settings baseline is `settings-approved-v1.png`.
  Validation: User accepted the final Settings concept direction on 2026-05-21.
  Review: Confirm Dashboard answers proxy/Codex/provider/usage/health/action clearly, Providers owns credentials/auth fields, Usage keeps cost as estimates, and Settings is a simple adaptive settings grid before moving to component code.
  Evidence: generated concept image(s), critique notes, and any prompt/brief updates in `EVIDENCE_AND_GATES.md`.
  Handoff: DONE — page-level image direction is accepted; next step is TDC-020 shadcn/ui prototype generation/assembly.

- [x] TDC-020 [owner=user-or-planner] [deps=TDC-018] [scope=external-shadcn-prototype,source-or-preview]
  Goal: Generate or assemble an initial simplified React 19 + Tailwind CSS 4 + shadcn/ui-style + TanStack product prototype with the revised workstream prompt and return either source export, images, or a preview URL.
  Progress: Throwaway prototype generated under `docs/workstreams/tauri-desktop-client/prototype/`; after user review, the shell was adjusted for a client-style fixed sidebar and root viewport, with scrolling moved into the main content region.
  Validation: User confirmed the prototype is good enough on 2026-05-21, with layout guidance that the desktop client must not behave like fully scrolling browser pages.
  Review: DONE_WITH_CONCERNS — direction accepted, but production implementation must harden fixed app-shell layout, internal table/panel scrolling, and sticky desktop regions.
  Evidence: prototype source/image/URL plus notes in EVIDENCE_AND_GATES.md.
  Handoff: DONE_WITH_CONCERNS — proceed to TDC-030 implementation brief; do not scaffold production Tauri until the stack/layout/repo-structure brief is accepted.

- [x] TDC-030 [owner=planner] [deps=TDC-020] [scope=docs/workstreams/tauri-desktop-client,frontend-design-brief]
  Goal: Convert the accepted prototype into an implementation brief: component inventory, page states, data contract map, visual tokens, technology choices, and repository layout plan.
  Progress: `IMPLEMENTATION_BRIEF.md` drafted with React 19, Tailwind CSS 4, shadcn/ui, TanStack Router/Query/Table, React Hook Form + Zod, Recharts, optional Zustand, Tauri command/API boundaries, fixed client-shell layout rules, and recommended `apps/desktop` repository structure.
  Validation: User accepted the implementation brief and authorized production frontend work on 2026-05-21.
  Review: DONE — selected third-party libraries are intentionally limited and the `apps/desktop/src-tauri` layout keeps the existing egui replacement fallback intact.
  Evidence: `docs/workstreams/tauri-desktop-client/IMPLEMENTATION_BRIEF.md`; official-source research notes and local structure audit in `EVIDENCE_AND_GATES.md`.
  Handoff: DONE — proceed to TDC-040/TDC-050 production shell and static/mock UI.

## M2 — Tauri Shell And Static App

- [x] TDC-040 [owner=main-or-worker] [deps=TDC-030] [scope=Cargo.toml,apps/desktop,apps/desktop/src-tauri,package.json,tauri-config]
  Goal: Add a runnable Tauri + React + Tailwind shell without wiring live proxy state yet.
  Validation: `pnpm build`, `cargo fmt --check`, and `cargo check -p codex-helper-desktop` pass.
  Review: DONE — `apps/desktop/src-tauri` is a new workspace member and the existing `crates/gui` fallback remains untouched.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-21 TDC-040/TDC-050 scaffold entry.
  Handoff: DONE — static shell exists and is ready for live data wiring.

- [x] TDC-050 [owner=main-or-worker] [deps=TDC-040] [scope=apps/desktop/src]
  Goal: Import and harden the accepted shadcn/ui prototype into maintainable React components with mock data for all simplified MVP pages.
  Validation: `pnpm test` and `pnpm build` pass in `apps/desktop`.
  Review: DONE_WITH_CONCERNS — four-page mock UI is production-structured and avoids banned visual patterns; visual browser smoke is still recommended before live API wiring polish.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-21 TDC-040/TDC-050 scaffold entry.
  Handoff: DONE_WITH_CONCERNS — UI remains mockable; next step is TDC-060 read-only admin API wiring plus richer loading/empty/error states.

## M3 — Read-Only Admin API Wiring

- [x] TDC-060 [owner=main-or-worker] [deps=TDC-050] [scope=desktop-frontend/src/api,desktop-frontend/src/pages]
  Goal: Wire read-only live data for Dashboard, Providers, Usage, and Settings via admin API links.
  Validation: unit tests for API client mapping; Tauri/manual smoke against a running local proxy.
  Review: DONE_WITH_CONCERNS — the desktop frontend now reads a Tauri-proxied admin read model built from `/operator/summary` links, `/runtime/status`, `/providers`, `/request-ledger/recent`, and `/request-ledger/summary`; mock fallback remains visible when admin data is unavailable. Full Tauri window smoke and auth-token-required UX remain for TDC-070/TDC-080.
  Evidence: `EVIDENCE_AND_GATES.md` 2026-05-21 TDC-060 entry.
  Handoff: DONE_WITH_CONCERNS — advanced sessions/routing/diagnostics remain collapsed, disabled, or mocked until simple surfaces work.

- [x] TDC-070 [owner=main-or-worker] [deps=TDC-060] [scope=desktop-frontend/src/state,desktop-frontend/src/components]
  Goal: Add loading, empty, disconnected, auth-token-required, and stale-runtime states.
  Validation: component/story/visual checks for key states.
  Review: DONE_WITH_CONCERNS — state taxonomy is now explicit and empty/fallback banners teach the next action. Visual QA is covered by component and route tests plus production build, but a full interactive Tauri window smoke remains pending.
  Evidence: `apps/desktop/src/lib/api/data-state.test.ts`, `apps/desktop/src/components/page/DataStateBanner.test.tsx`, `apps/desktop/src/app/App.test.tsx`; `EVIDENCE_AND_GATES.md` 2026-05-21 TDC-070 entry.
  Handoff: DONE_WITH_CONCERNS — attached/resident lifecycle uncertainty is now visible as owner-pending copy; actual owner semantics and tray behavior remain TDC-090.

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

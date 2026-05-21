# Tauri Desktop Client — Evidence And Gates

Status: Draft
Last updated: 2026-05-21

## Required Gates By Phase

Planning:

- Workstream docs agree.
- shadcn/ui prototype prompt reviewed for product scope and lifecycle correctness.

Design import:

- shadcn/ui prototype reviewed against `impeccable` product UI rules.
- Accepted direction documented before implementation.

Frontend:

- Package manager command selected after project scaffold exists.
- Typecheck/build/lint commands recorded.
- Component or visual smoke for key states.

Rust/Tauri:

- `cargo fmt --check`
- `cargo check` for affected Rust packages.
- Targeted tests for changed lifecycle or admin API code.

Lifecycle:

- Matrix covers:
  - desktop-owned start;
  - window close;
  - tray hide/show;
  - tray quit;
  - attach existing resident;
  - explicit Stop Proxy;
  - runtime unavailable;
  - admin token required.

## Evidence Log

### 2026-05-20 — Workstream opened

Evidence:

- User confirmed:
  - Tauri client should replace existing egui GUI;
  - frontend stack is React + Tailwind;
  - initial UI should be a shadcn/ui component prototype;
  - initial MVP pages were Dashboard, Stations, Sessions, Requests, Diagnostics, Settings before simplification.
- User then clarified the first GUI should be much simpler, closer to a light account/API-key/usage dashboard. The MVP top-level navigation was revised to Dashboard, API Keys, Usage, Providers, Settings, with sessions/routing/diagnostics as advanced collapsed areas.
- `impeccable` context loader found no `PRODUCT.md` or `DESIGN.md`, so this workstream captures project-specific product context instead of relying on generic UI assumptions.
- Local references inspected:
  - `README.md`
  - `README_EN.md`
  - `repo-ref/sub2api/README_CN.md`
  - `repo-ref/sub2api/frontend/src/router/index.ts`
  - `crates/core/src/proxy/control_plane_manifest.rs`
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/gui/src/gui/pages/`

Result:

- PASS — M0 scope and simplified product direction documented.

### 2026-05-21 — Local Tauri client reference reviewed

Evidence:

- Reviewed `repo-ref/aio-coding-hub` as a local Tauri + React + Tailwind gateway-client reference.
- Inspected:
  - `repo-ref/aio-coding-hub/src/layout/AppLayout.tsx`
  - `repo-ref/aio-coding-hub/src/ui/Sidebar.tsx`
  - `repo-ref/aio-coding-hub/src/ui/PageHeader.tsx`
  - `repo-ref/aio-coding-hub/src/pages/HomePage.tsx`
  - `repo-ref/aio-coding-hub/src/components/home/HomeOverviewPanel.tsx`
  - `repo-ref/aio-coding-hub/src/pages/providers/ProvidersView.tsx`
  - `repo-ref/aio-coding-hub/src/pages/UsagePage.tsx`
  - `repo-ref/aio-coding-hub/src/pages/settings/SettingsPage.tsx`
- Accepted structural references:
  - fixed Tauri shell with sidebar and drag-safe content;
  - sidebar footer runtime status;
  - page headers with actions/tabs;
  - dashboard work-status and recent-request panels;
  - provider cards plus right-side active order;
  - usage filter strip plus tabbed data panel;
  - settings main column plus secondary sidebar.
- Rejected for the first GUI:
  - broad navigation scope;
  - console/logs/workspaces/prompts/MCP/skills as top-level pages;
  - model validation as a first-class initial page;
  - exposing circuit/session/quota internals on the first screen.

Result:

- PASS — `HANDOFF.md` prompt and `DESIGN.md` product direction now include the useful client UI patterns without increasing the simplified sitemap.

## Deferred / Not Run Yet

- No code has been generated or imported yet.
- No frontend gates exist until the Tauri/React workspace is scaffolded.
- No Rust gates are required for this planning-only slice.

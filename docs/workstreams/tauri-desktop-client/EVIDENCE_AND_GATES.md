# Tauri Desktop Client — Evidence And Gates

Status: Draft
Last updated: 2026-05-22

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
- On 2026-05-21, the user further clarified that API keys are provider credentials in a local proxy product, so API Keys should not remain a standalone top-level page. The simplified sitemap was revised to Dashboard, Providers, Usage, Settings.
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

### 2026-05-21 — Pre-imagegen UI brief added

Evidence:

- Reviewed the user-provided AIO Coding Hub screenshot for desktop shell, sidebar footer, runtime status, dashboard cards, recent records, and page density.
- Reviewed `repo-ref/awesome-design-md` collection and sampled local design-system references:
  - `repo-ref/awesome-design-md/design-md/supabase/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/mintlify/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/linear.app/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/ollama/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/notion/DESIGN.md`
- Rechecked local product capabilities from:
  - `README.md`
  - `docs/CONFIGURATION.zh.md`
  - `crates/core/src/proxy/control_plane_manifest.rs`
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/core/src/dashboard_core/types.rs`
  - `crates/core/src/usage_balance.rs`
  - existing egui page inventory under `crates/gui/src/gui/pages/`
- Added `docs/workstreams/tauri-desktop-client/UI_BRIEF.md` with:
  - reference synthesis;
  - primary user questions;
  - desktop layout and visual tokens;
  - page-by-page requirements;
  - state requirements;
  - data contract map;
  - copy rules;
  - imagegen prompt pack and negative prompt.

Result:

- PASS — visual direction is ready for first imagegen concept generation and critique. No implementation started.

### 2026-05-21 — Dashboard and provider credential concepts accepted

Evidence:

- Accepted Dashboard concept copied to `docs/workstreams/tauri-desktop-client/assets/dashboard-approved-v1.png`.
- Accepted simplified API Keys concept copied to `docs/workstreams/tauri-desktop-client/assets/api-keys-approved-v1.png`, then reclassified as a provider credential list density reference at `docs/workstreams/tauri-desktop-client/assets/provider-credentials-list-reference-v1.png`.
- Rejected the earlier dense API Keys direction because it repeated Dashboard proxy status, used a permanent selected-key panel, and made the credential page feel like a control-console screen.
- Rejected standalone API Keys as an information-architecture concept because codex-helper configures provider API keys as part of provider setup.
- Updated `UI_BRIEF.md` with:
  - accepted image references;
  - Dashboard Codex-focused shell constraints;
  - Providers-as-credentials-owner rules;
  - pagination and table-density rules;
  - the next Providers page imagegen prompt.

Result:

- PASS — Dashboard and provider credential list density are now local workstream references. Next visual step is to generate the Providers page from `UI_BRIEF.md` and critique it before shadcn/ui prototype work.

### 2026-05-21 — Providers concept accepted

Evidence:

- Accepted Providers concept copied to `docs/workstreams/tauri-desktop-client/assets/providers-approved-v1.png`.
- The accepted concept confirms:
  - four-item sidebar navigation: Dashboard, Providers, Usage, Settings;
  - provider cards as the default Providers surface;
  - each provider card shows `Host`, `Auth`, health, latency, balance, usage, and capabilities;
  - right-side default route order panel remains visible and useful;
  - no standalone API Keys page or provider marketplace treatment.
- Added the next Usage page imagegen prompt to `UI_BRIEF.md`.

Result:

- PASS — Providers has an accepted visual baseline. Next visual step is to generate the Usage page from `UI_BRIEF.md`.

### 2026-05-21 — Usage concept needs revision

Evidence:

- Generated a first Usage concept for review.
- User compared it with `repo-ref/sub2api` usage history reference and noted useful patterns:
  - dense full-width request table;
  - practical pagination footer;
  - cost info tooltip;
  - token direction/icons;
  - first-token latency column.
- Rejected the permanent right-side cost panel because codex-helper is a local proxy helper, not the billing authority. Costs should be presented as estimates and detailed through row-level tooltip/popover.
- Updated `UI_BRIEF.md` Usage prompt to require:
  - full-width table;
  - no permanent right-side inspector;
  - “预估费用/预估花费” copy;
  - sub2api-style pagination;
  - cost estimate tooltip with “实际费用以供应商结算为准”.

Result:

- NEEDS_REVISION — regenerate the Usage page from the revised `UI_BRIEF.md` prompt before accepting it.

### 2026-05-21 — Usage concept accepted

Evidence:

- Accepted revised Usage concept copied to `docs/workstreams/tauri-desktop-client/assets/usage-approved-v1.png`.
- Accepted design qualities:
  - full-width request table;
  - four compact summary cards;
  - filters and export actions;
  - estimated-cost language;
  - cost detail as a tooltip/popover, not a permanent panel;
  - token direction/cache indicators;
  - first-token latency and duration columns;
  - practical bottom pagination.

Result:

- PASS — Usage has an accepted visual baseline. Next visual step is Settings.

### 2026-05-21 — Settings concept rejected and prompt revised

Evidence:

- Generated a first Settings concept and saved it as `docs/workstreams/tauri-desktop-client/assets/settings-generated-too-complex-v1.png`.
- User rejected it as too complex for Settings and asked to reference simpler desktop settings layouts instead, specifically the attached examples now saved as:
  - `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-a.png`
  - `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-b.png`
- User direction:
  - Settings should be simpler and more adaptive;
  - two columns are acceptable for responsive layout;
  - avoid the crowded control-console feeling from the generated image.
- Updated `UI_BRIEF.md`, `DESIGN.md`, and `HANDOFF.md` to make Settings a simple responsive two-column settings grid:
  - no permanent right sidebar;
  - no giant full-width control-console form;
  - normal cards for desktop behavior, appearance/language, local proxy, Codex connection, advanced tools, about/paths, and dangerous actions;
  - advanced tools collapsed and visually secondary;
  - dangerous lifecycle actions compact but clearly separated at the bottom.

Result:

- NEEDS_REVISION — regenerate the Settings page from the revised `Next Settings Page Imagegen Prompt` before accepting the page set.

### 2026-05-21 — Settings v2 candidate generated

Evidence:

- Regenerated Settings from the revised simplified two-column prompt.
- Copied the generated image from the Codex default image directory to `docs/workstreams/tauri-desktop-client/assets/settings-candidate-v2.png`.
- Initial review:
  - closer to the desired simple desktop-utility settings page than the rejected v1;
  - uses two-column adaptive setting cards;
  - removes the permanent right-side runtime/about sidebar;
  - keeps advanced tools collapsed/list-like and visually secondary;
  - keeps dangerous lifecycle actions at the bottom with a visually stronger red `Stop Proxy` button.

Result:

- PENDING_USER_REVIEW — user should accept `settings-candidate-v2.png` as the Settings baseline or request another revision before moving from TDC-018 to TDC-020.

### 2026-05-21 — Settings concept accepted and TDC-018 verified

Evidence:

- User accepted the revised Settings concept with: "我觉得可以 够好了".
- Copied `docs/workstreams/tauri-desktop-client/assets/settings-candidate-v2.png` to `docs/workstreams/tauri-desktop-client/assets/settings-approved-v1.png`.
- Accepted page-level baselines now exist:
  - `docs/workstreams/tauri-desktop-client/assets/dashboard-approved-v1.png`
  - `docs/workstreams/tauri-desktop-client/assets/providers-approved-v1.png`
  - `docs/workstreams/tauri-desktop-client/assets/usage-approved-v1.png`
  - `docs/workstreams/tauri-desktop-client/assets/settings-approved-v1.png`
- TDC-018 status updated in `TODO.md` as complete.

Verification:

- Command: `git diff --check -- docs/workstreams/tauri-desktop-client`
- Scope: planning docs and image assets under `docs/workstreams/tauri-desktop-client`
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings.

Skipped broader gates:

- `cargo fmt --check`, `cargo check`, and Rust tests were not run because this was a planning/image-concept documentation slice with no Rust source changes.
- Frontend build/typecheck/lint gates do not exist yet because the Tauri/React workspace has not been scaffolded; they start at TDC-040/TDC-050.

Result:

- PASS — TDC-018 page-level image concept direction is accepted and ready for TDC-020 shadcn/ui prototype generation/assembly.

### 2026-05-21 — TDC-020 prototype scaffold generated

Evidence:

- Added a throwaway prototype under `docs/workstreams/tauri-desktop-client/prototype/`.
- Prototype stack matches the user-requested constraint:
  - React 19.2.6;
  - Tailwind CSS 4.3.0 through `@tailwindcss/vite`;
  - shadcn/ui-style local primitives plus `components.json`;
  - TanStack Router for the four prototype pages;
  - TanStack Table for the Usage table;
  - TanStack Query provider as the future data-fetching boundary while keeping the prototype mock-only.
- Implemented reusable prototype components:
  - `AppShell`;
  - `PageHeader`;
  - `StatusStrip`;
  - `MetricCard`;
  - `ProviderCard`;
  - `UsageTable`;
  - local `ui.tsx` primitives.
- Implemented four static pages from the accepted baselines:
  - Dashboard;
  - Providers;
  - Usage;
  - Settings.

Verification:

- Command: `pnpm install`
- Scope: `docs/workstreams/tauri-desktop-client/prototype`
- Result: PASS — dependencies installed and `pnpm-lock.yaml` generated.
- Notes: pnpm warned about ignored `msw` build scripts from transitive dependencies; this prototype does not use MSW.

- Command: `pnpm build`
- Scope: `docs/workstreams/tauri-desktop-client/prototype`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-*.css`
  - `dist/assets/index-*.js`

Skipped broader gates:

- Rust gates were not run because this task only adds a throwaway front-end prototype under the workstream docs path and does not change Rust code.
- Production Tauri build/lint gates are not applicable yet because TDC-020 is prototype-only; production scaffold starts later after user review.

Result:

- PENDING_USER_REVIEW — prototype is generated and buildable. User should review by running `pnpm dev --host 127.0.0.1` inside `docs/workstreams/tauri-desktop-client/prototype`, then either accept the direction or request revisions before TDC-020 is marked complete.


### 2026-05-21 — TDC-020 prototype accepted with desktop layout concern

Evidence:

- User reviewed the prototype and said the direction is good enough, with a layout concern: this is a desktop client, so the app should not behave like fully scrolling browser pages and the sidebar should remain fixed.
- Adjusted the throwaway prototype shell:
  - `docs/workstreams/tauri-desktop-client/prototype/src/styles.css` now gives `html`, `body`, and `#root` full height and hides root overflow.
  - `docs/workstreams/tauri-desktop-client/prototype/src/components/AppShell.tsx` now uses a fixed-height app shell, fixed sidebar, flex column main region, and main-content scrolling.
  - `docs/workstreams/tauri-desktop-client/prototype/README.md` records the fixed-shell prototype note.

Verification:

- Command: `pnpm build`
- Scope: `docs/workstreams/tauri-desktop-client/prototype`
- Result: PASS — TypeScript and Vite production build completed after the fixed-shell layout adjustment.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-DL9ToUTu.css`
  - `dist/assets/index-BK9oBwIx.js`

Skipped broader gates:

- Rust gates were not run because this update only changes prototype frontend files under the workstream docs path.
- Production Tauri gates remain not applicable because this is still a throwaway prototype, not the production app.

Result:

- DONE_WITH_CONCERNS — TDC-020 direction is accepted, but production implementation must harden desktop-client layout: fixed sidebar, fixed root viewport, and bounded main/table/panel scrolling.

### 2026-05-21 — TDC-030 implementation brief drafted

Evidence:

- Added `docs/workstreams/tauri-desktop-client/IMPLEMENTATION_BRIEF.md`.
- Official-source research was used to select and constrain the stack:
  - React 19 baseline;
  - Tailwind CSS 4 with `@tailwindcss/vite`;
  - Tauri v2 shell/config/commands/tray direction;
  - shadcn/ui Vite/Tailwind v4/component patterns;
  - TanStack Router, Query, and Table;
  - React Hook Form + Zod for forms, with TanStack Form deferred unless form complexity later justifies it;
  - Recharts through shadcn Chart wrappers;
  - Zustand only as an optional tiny persisted shell-preference store.
- Source links are captured in `IMPLEMENTATION_BRIEF.md` under `Research Sources`.
- Local repository structure audit:
  - current Cargo workspace members are `crates/core`, `crates/tui`, and `crates/gui`;
  - existing egui GUI remains under `crates/gui` and should stay until replacement parity;
  - recommended production Tauri app path is `apps/desktop`, with `apps/desktop/src-tauri` joining the Cargo workspace and depending on `codex-helper-core`.
- NPM version snapshot recorded in `IMPLEMENTATION_BRIEF.md` from `npm view` on 2026-05-21 for the proposed packages.

Verification:

- Command: `git diff --check -- docs/workstreams/tauri-desktop-client`
- Scope: planning docs, prototype files, and workstream metadata under `docs/workstreams/tauri-desktop-client`
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited Markdown files.

Skipped broader gates:

- `cargo fmt --check`, `cargo check`, and Rust tests are not required yet because no Rust source or production Tauri crate has been changed.
- Production frontend lint/test gates do not exist yet because TDC-040 scaffold has not started.

Result:

- PASS — user accepted the implementation brief and authorized production frontend work, including fearless refactoring where it improves the architecture.

### 2026-05-21 — TDC-040/TDC-050 production shell and static UI scaffolded

Evidence:

- Added production desktop app under `apps/desktop` with React 19, Tailwind CSS 4, TanStack Router/Query/Table, React Hook Form + Zod scaffolding, and Vitest.
- Added Tauri v2 crate under `apps/desktop/src-tauri` and joined it to the root Cargo workspace.
- Kept existing egui crate intact; no replacement/removal is claimed yet.
- Imported and hardened the accepted four-page mock UI into production-oriented folders:
  - `src/app` for shell, router, and query client;
  - `src/features/dashboard`;
  - `src/features/providers`;
  - `src/features/usage`;
  - `src/features/settings`;
  - `src/lib/api`, `src/lib/tauri`, and `src/mocks`.
- Hardened desktop layout:
  - fixed root viewport;
  - fixed sidebar;
  - drag-safe title strip;
  - bounded main content scrolling;
  - internal provider-list/table scrolling and sticky usage table header.
- Added minimal Tauri commands:
  - `get_app_metadata`;
  - `get_known_paths`.
- Added route smoke tests for Dashboard and Usage.

Verification:

- Command: `pnpm install`
- Scope: `apps/desktop`
- Result: PASS — dependencies installed and `pnpm-lock.yaml` generated.
- Note: pnpm warned about ignored `msw` build scripts from transitive dependencies; the app does not use MSW directly.

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — Vitest route smoke tests passed: 1 file, 2 tests.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-CQ75AXtA.css`
  - `dist/assets/index-D0H-tgUg.js`

- Command: `cargo fmt --check`
- Scope: repository workspace
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: `apps/desktop/src-tauri`
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited text files.

Skipped broader gates:

- Full workspace tests were not run because this slice only adds the production desktop shell and static/mock UI; no core runtime behavior was changed.
- Tauri runtime manual smoke is still pending; the shell compiles, but no live admin API or lifecycle sidecar behavior is wired yet.
- Visual browser smoke is pending for the next iteration because this pass focused on scaffold/build/test gates.

Result:

- PASS — TDC-040 shell and TDC-050 static/mock UI are scaffolded and buildable. Next implementation step is TDC-060 read-only admin API wiring and richer empty/error/loading states.

### 2026-05-21 — TDC-060 read-only admin API wiring

Evidence:

- Added a Tauri-side read-only admin API command:
  - `get_admin_read_model`;
  - reads local admin base from `CODEX_HELPER_DESKTOP_ADMIN_URL` or defaults to proxy `3211` / admin `4211`;
  - fetches `/__codex_helper/api/v1/operator/summary` first, then follows summary links for runtime status, providers, request-ledger recent, and request-ledger summary.
- Added frontend admin API boundaries:
  - DTOs in `src/lib/api/admin-types.ts`;
  - HTTP helper and admin port utilities in `src/lib/api/admin-client.ts`;
  - Tauri read-model adapter in `src/lib/api/admin-read-model.ts`;
  - view-model mappers in `src/lib/api/mappers.ts`;
  - mock fallback data in `src/lib/api/mock-data.ts`.
- Wired Dashboard, Providers, Usage, Settings, shell runtime footer, status strip, and page header badges through TanStack Query hooks.
- Added visible fallback/state components:
  - `DataStateBanner` for loading/refreshing/error/mock fallback;
  - `EmptyState` for empty recent request/provider/table surfaces.
- Mock design mode remains available when the Tauri command fails or no local admin API is reachable.

Verification:

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — Vitest passed: 3 files, 9 tests. Covers route smoke, mock fallback banner, live admin read-model rendering, API client URL/port helpers, and mapper behavior.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-CagLFIkT.css`
  - `dist/assets/index-Biz3x66H.js`

- Command: `cargo fmt --check`
- Scope: repository workspace
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: `apps/desktop/src-tauri`
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited text files.

- Manual admin API smoke:
  - Command: `Test-NetConnection -ComputerName 127.0.0.1 -Port 4211`
  - Result: PASS — loopback admin port was reachable.
  - Command: `Invoke-RestMethod -Uri 'http://127.0.0.1:4211/__codex_helper/api/v1/operator/summary' -TimeoutSec 3`
  - Result: PASS — returned `api_version = 1`, `service_name = codex`, `counts.providers = 18`, `counts.recent_requests = 200`, and advertised request-ledger/providers/runtime links.

Concerns / deferred:

- Full Tauri window smoke is still pending; this pass validates the command crate, frontend build/tests, and live admin endpoint availability but does not run `pnpm tauri:dev`.
- Auth-token-required remote admin UX is still generic error copy; TDC-070 should split auth-required, disconnected, stale, and empty states more explicitly.
- Provider credentials remain read-only/auth-source placeholders because the read-only admin surface does not expose secret material; safe credential/provider mutations belong to TDC-080.

Result:

- DONE_WITH_CONCERNS — Dashboard, Providers, Usage, and Settings now consume real read-only admin data when available and fall back to mock data when unavailable. TDC-070 should refine state taxonomy and visual QA.

### 2026-05-21 — TDC-070 runtime data states and visual QA

Evidence:

- Added a unified frontend runtime data-state taxonomy:
  - `loading`;
  - `live`;
  - `refreshing`;
  - `mock`;
  - `unavailable`;
  - `disconnected`;
  - `auth-required`;
  - `empty`;
  - `stale`.
- Added `apps/desktop/src/lib/api/data-state.ts` to classify Tauri/admin API failures into user-facing states.
- Added `apps/desktop/src/lib/api/use-admin-read-model.ts` so Dashboard, Providers, Usage, Settings, shell runtime footer, and page headers share one read-model state boundary.
- Upgraded `DataStateBanner` from a generic mock/error banner into a state-aware banner with distinct copy, severity, badge, icon, and retry affordance.
- Added explicit next-action copy:
  - disconnected state tells the user to start the local proxy or attach an existing runtime later;
  - auth-required state points at `CODEX_HELPER_ADMIN_TOKEN` and `x-codex-helper-admin-token`;
  - stale state keeps previous live data visible but disables unsafe live actions;
  - empty usage/providers states explain how to produce records or configure providers.
- Added visible lifecycle uncertainty:
  - shell/status/settings now show owner-pending messaging instead of pretending the frontend knows whether the runtime is desktop-owned or attached;
  - no ordinary quit/close path is represented as stopping an attached runtime.

Verification:

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — Vitest passed: 5 files, 20 tests. Covers data-state classification, `DataStateBanner` component states, route-level mock fallback, admin-token-required state, empty usage state, and live read-model rendering.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-ClSUnf_7.css`
  - `dist/assets/index-DG-9RxYV.js`

- Command: `cargo fmt --check`
- Scope: repository workspace
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: `apps/desktop/src-tauri`
- Result: PASS.

- Command: `git diff --check -- .`
- Scope: full repository diff
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited text files.

Concerns / deferred:

- Full Tauri window smoke remains pending; this pass validates browser-rendered React state surfaces through tests/build but does not run `pnpm tauri:dev`.
- Actual start/attach/stop/switch mutations are intentionally still disabled or placeholder-only until TDC-080.
- Runtime owner semantics are only surfaced as uncertainty in the UI; authoritative desktop-owned vs attached behavior remains TDC-090.

Result:

- DONE_WITH_CONCERNS — TDC-070 state taxonomy and visual QA are implemented. Next step is TDC-080 safe control actions.

### 2026-05-21 — TDC-080 safe control actions

Evidence:

- Added typed Tauri control commands under `apps/desktop/src-tauri/src/commands/control.rs`:
  - `get_desktop_control_state`;
  - `attach_existing_proxy`;
  - `start_desktop_proxy`;
  - `stop_proxy`;
  - `switch_codex`;
  - `reload_runtime`;
  - `probe_station`;
  - `refresh_provider_balances`;
  - `apply_provider_runtime_override`;
  - `set_global_route_override`;
  - `apply_session_overrides`;
  - `reset_session_overrides`.
- Reused existing core lifecycle semantics:
  - `RuntimeOwnerMarker`;
  - `RuntimeOwnerKind`;
  - `decide_runtime_stop_action`;
  - `RuntimeStopIntent::ExplicitStop`.
- Added admin-token propagation to the desktop admin client path:
  - reads `CODEX_HELPER_ADMIN_TOKEN`;
  - sends `x-codex-helper-admin-token` on desktop admin requests.
- Added exact confirmation phrases for dangerous local mutations:
  - `STOP OWNED PROXY`;
  - `STOP ATTACHED PROXY`;
  - `SWITCH CODEX`;
  - `SWITCH OFF CODEX`.
- Added frontend action boundary:
  - `apps/desktop/src/features/runtime/actions.ts`;
  - `apps/desktop/src/features/runtime/ActionStatusBanner.tsx`;
  - command wrappers in `apps/desktop/src/lib/tauri/commands.ts`;
  - shared control state types in `apps/desktop/src/lib/api/types.ts`.
- Wired UI surfaces:
  - Dashboard can start/attach/refresh and routes switch confirmation to Settings instead of silently mutating Codex config.
  - Providers can probe, refresh balances, set global route target, disable provider runtime, and clear provider runtime overrides.
  - Settings can reload runtime, switch Codex on/off with confirmation, set/clear global route override, set/reset session route override, and choose owned stop vs remote stop with separate confirmation phrases.

Manual action matrix:

| Action | Behavior | Safety boundary |
| --- | --- | --- |
| Attach Existing | Requires reachable admin API and reports attached state. | Does not stop or take ownership of external runtime. |
| Start Proxy | Spawns `codex-helper serve --codex --host 127.0.0.1 --port <port> --no-tui --desktop-managed` via `CODEX_HELPER_CLI_PATH`/sibling CLI lookup, then polls admin API. | Uses existing desktop owner marker path; tray/sidecar authority remains TDC-090. |
| Stop Owned | Requires `STOP OWNED PROXY`. | Only maps to `StopOwnedRuntime` for desktop-owned state. |
| Remote Stop Attached | Requires `STOP ATTACHED PROXY`. | Separate action/copy; falls back to detach-only if shutdown is unavailable. |
| Codex Switch On | Requires `SWITCH CODEX` in Settings. | Uses non-interactive core switch API; Dashboard quick action is disabled and points to Settings. |
| Codex Switch Off | Requires `SWITCH OFF CODEX` in Settings. | Uses core switch-state restoration path. |
| Reload/probe/balance refresh | Exposed as explicit buttons. | Disabled when live admin state is not usable. |
| Provider/global/session overrides | Exposed in Providers/Settings advanced areas. | Typed payloads through Tauri commands; no secret material is displayed. |

Verification:

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-ConQhKx5.css`
  - `dist/assets/index-6JIEix8J.js`

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — Vitest passed: 5 files, 20 tests.

- Command: `cargo fmt --check`
- Scope: repository workspace
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: `apps/desktop/src-tauri`
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: `apps/desktop/src-tauri`
- Result: PASS — 6 tests. Covers owner classification, explicit confirmation phrases, stop decision separation, CLI sibling lookup, and balance-refresh query encoding.

- Command: `git diff --check -- .`
- Scope: full repository diff
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited text files.

Concerns / deferred:

- Full Tauri window smoke remains pending; this slice validates command/frontend boundaries but does not run `pnpm tauri:dev`.
- `start_desktop_proxy` is implemented by locating/spawning the CLI binary; TDC-090 should make the tray/sidecar path authoritative and package-aware.
- Tray minimize/quit/autostart behavior is still TDC-090.

Result:

- DONE_WITH_CONCERNS — TDC-080 safe mutations are implemented with visible confirmation boundaries. Next step is TDC-090 tray and authoritative desktop owner semantics.

### 2026-05-22 — TDC-090 tray and desktop lifecycle semantics

Evidence:

- Added `apps/desktop/src-tauri/src/lifecycle.rs` as the desktop lifecycle boundary:
  - creates a Tauri tray icon with `Show Window`, `Hide to Tray`, and `Quit App (Proxy Keeps Running)`;
  - intercepts main-window close requests and hides the window to tray unless an explicit app quit has been requested;
  - records explicit app-quit intent through managed `DesktopLifecycleState`;
  - exposes a policy assertion that normal app exit leaves the proxy runtime running.
- Added Tauri window/app commands:
  - `show_main_window`;
  - `hide_main_window`;
  - `minimize_main_window`;
  - `toggle_main_window_maximized`;
  - `quit_app`.
- Added a Desktop Capability Matrix to `IMPLEMENTATION_BRIEF.md` for longer-term desktop support:
  - desktop residency;
  - system tray;
  - auto update;
  - launch at login;
  - single instance;
  - lightweight single-config import/export;
  - open folders/paths;
  - packaged sidecar.
- Wired frontend desktop controls:
  - custom titlebar minimize/maximize/close now call Tauri commands;
  - titlebar close maps to hide-to-tray, not process/runtime shutdown;
  - Settings Dangerous Actions now make Quit App, Detach, and Stop Proxy visibly different;
  - StatusStrip and Settings display desktop-owned vs attached lifecycle owner labels from control state.
- Preserved TDC-080 safety boundary:
  - only `stop_proxy` with exact confirmation phrases can request runtime shutdown;
  - no normal window close, Detach, or Quit App path invokes `stop_proxy`.

Lifecycle matrix:

| Scenario | Current behavior | Runtime stop behavior | Evidence |
| --- | --- | --- | --- |
| Desktop-owned start | `start_desktop_proxy` still spawns `codex-helper serve --codex --host 127.0.0.1 --port <port> --no-tui --desktop-managed` through env/sibling CLI lookup and then polls admin API. | No implicit shutdown is attached to start; explicit Stop Owned remains required. | TDC-080 control command tests plus TDC-090 lifecycle tests. |
| Window close button | Frontend titlebar close calls `hide_main_window`; native close request is intercepted and hidden to tray. | Does not call admin shutdown or `stop_proxy`. | `App.test.tsx` close-button test; `lifecycle::tests::close_request_hides_to_tray_until_safe_quit_is_requested`. |
| Tray hide/show | Tray menu and left-click/double-click can show the main window; tray menu can hide it. | Show/hide are window-only operations. | `setup_tray` and window command compile gate. |
| Tray quit / Settings Quit App | Sets explicit app-quit state and exits the desktop process. Menu label says proxy keeps running. | Leaves runtime running; explicit Stop Proxy remains separate. | `lifecycle::tests::normal_app_exit_never_stops_proxy_runtime`; Settings route test. |
| Attach existing resident runtime | Attached control state is preserved in UI; attach remains non-owning. | Normal close/quit only detaches/hides/exits UI and never remote-stops attached runtime. | Existing TDC-080 attach/owner tests plus new no-`stop_proxy` frontend tests. |
| Explicit Stop Proxy | Settings still requires exact `STOP OWNED PROXY` or `STOP ATTACHED PROXY` and calls `stop_proxy`. | Only this path may request admin runtime shutdown. | TDC-080 stop-decision tests plus TDC-090 Settings separation test. |
| Runtime unavailable | Window/tray commands still compile; data banners remain responsible for disconnected/unavailable admin state. | No runtime stop is attempted. | Existing TDC-070 state tests and TDC-090 build/test gates. |
| Admin token required | Safe action state remains disabled until token/admin access is usable. | No runtime stop is attempted without explicit stop command. | Existing TDC-070 admin-token test and TDC-080 command boundary. |

Verification:

- Command: `pnpm test`
- Scope: `apps/desktop`
- Result: PASS — Vitest passed: 5 files, 22 tests. New coverage verifies titlebar close routes to `hide_main_window` and Settings Quit App/Detach do not invoke `stop_proxy`.

- Command: `pnpm build`
- Scope: `apps/desktop`
- Result: PASS — TypeScript and Vite production build completed.
- Output summary:
  - `dist/index.html`
  - `dist/assets/index-ConQhKx5.css`
  - `dist/assets/index-CfAyaNaZ.js`

- Command: `cargo fmt --check`
- Scope: repository workspace
- Result: PASS.

- Command: `cargo check -p codex-helper-desktop`
- Scope: `apps/desktop/src-tauri`
- Result: PASS.

- Command: `cargo nextest run -p codex-helper-desktop --lib`
- Scope: `apps/desktop/src-tauri`
- Result: PASS — 9 tests. Adds lifecycle close/quit policy tests on top of TDC-080 owner/stop tests.

- Command: `git diff --check -- .`
- Scope: full repository diff
- Result: PASS — no diff whitespace errors reported. Git emitted only Windows LF/CRLF warnings for edited text files.

Concerns / deferred:

- Full interactive OS tray smoke is still pending; this pass validates compile/tests/build and lifecycle policy, but it does not automate `pnpm tauri:dev` window/tray clicks.
- Packaged sidecar declaration/signing/autostart remain out of this slice. Current Start Proxy still uses the hidden desktop-managed CLI path from TDC-080 (`CODEX_HELPER_CLI_PATH` or sibling binary).
- Import/export is intentionally scoped to simple single-config backup/restore, not heavy multi-profile/workspace configuration management.
- No egui removal or replacement-readiness claim is made yet; TDC-100 should document parity gaps before promoting Tauri as the replacement path.

Result:

- DONE_WITH_CONCERNS — TDC-090 tray lifecycle behavior and no-normal-exit-stops-attached-runtime rule are implemented and verified at command/test/build level.

## Deferred / Not Run Yet

- No full interactive Tauri tray/window smoke has been run yet.
- No packaged sidecar/autostart/signing smoke has been run yet.
- No egui removal or replacement-readiness claim is made yet.

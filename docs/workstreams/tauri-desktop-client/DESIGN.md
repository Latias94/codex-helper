# Tauri Desktop Client

Status: Draft
Last updated: 2026-05-21

## Why This Lane Exists

`codex-helper` needs a long-term desktop client that replaces the existing egui GUI. The first version should not expose every internal control-plane concept. It should be built as a clean React + Tailwind + shadcn/ui component prototype first, then adapted into a Tauri app.

The desired first GUI is a simple local proxy dashboard:

- simple left navigation;
- client-style layout with a fixed desktop sidebar, fixed root viewport, and bounded main/panel scrolling;
- account/runtime summary cards;
- provider list with credentials;
- usage table and charts;
- restrained teal accent;
- most advanced controls hidden behind row actions, settings, or detail views.

## Relevant Authority

- Existing docs:
  - `README.md`
  - `README_EN.md`
  - `docs/CONFIGURATION.md`
  - `docs/CONFIGURATION.zh.md`
- Related workstreams:
  - `docs/workstreams/resident-proxy-attach-first/`
  - `docs/workstreams/desktop-lifecycle-owner/`
- Local references:
  - `docs/workstreams/tauri-desktop-client/UI_BRIEF.md`
  - `repo-ref/sub2api/README_CN.md`
  - `repo-ref/sub2api/frontend/src/router/index.ts`
  - `repo-ref/aio-coding-hub/src/layout/AppLayout.tsx`
  - `repo-ref/aio-coding-hub/src/ui/Sidebar.tsx`
  - `repo-ref/aio-coding-hub/src/pages/HomePage.tsx`
  - `repo-ref/aio-coding-hub/src/pages/providers/ProvidersView.tsx`
  - `repo-ref/aio-coding-hub/src/pages/UsagePage.tsx`
  - `repo-ref/aio-coding-hub/src/pages/settings/SettingsPage.tsx`
  - `repo-ref/awesome-design-md/design-md/supabase/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/mintlify/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/linear.app/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/ollama/DESIGN.md`
  - `repo-ref/awesome-design-md/design-md/notion/DESIGN.md`
- Existing local GUI/TUI surfaces:
  - `crates/gui/src/gui/pages/`
  - `crates/tui/src/tui/`
- Existing admin API and control-plane surface:
  - `crates/core/src/proxy/control_plane_manifest.rs`
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/core/src/proxy/control_plane_routes/`
- Current implementation brief:
  - `docs/workstreams/tauri-desktop-client/IMPLEMENTATION_BRIEF.md`

## Problem

The original product direction exposed too much as top-level navigation: stations, sessions, requests, diagnostics, route graph, and detailed observability. That shape is powerful but too complex for an initial GUI replacement.

The first Tauri GUI should reduce cognitive load:

- make normal users comfortable starting and observing the local proxy;
- present the most important usage and relay health information without explaining every route graph detail;
- keep provider credential and usage pages familiar;
- hide advanced station/session/routing/diagnostics controls behind "advanced" areas;
- preserve safe desktop owner semantics without making lifecycle theory a top-level concept.

## Target State

When this workstream closes:

- A simplified Tauri desktop client direction is documented as the replacement path for the current egui GUI.
- The first implementation slice starts from a React + Tailwind + shadcn/ui component prototype.
- The production frontend stack and repository layout are documented before Tauri scaffold work starts.
- The simple MVP sitemap is:
  - Dashboard
  - Providers
  - Usage
  - Settings
- Advanced controls are available but not top-level:
  - sessions;
  - route graph;
  - relay diagnostics;
  - request trace detail;
  - raw TOML editor;
  - desktop lifecycle internals.
- Runtime lifecycle behavior still matches the owner model:
  - ordinary close stops only desktop-owned runtime;
  - attached mode detaches without remote shutdown;
  - explicit Stop Proxy is clearly distinguished from Quit;
  - future tray sidecar maps to the hidden desktop-managed semantics.

## In Scope

- Product information architecture for the simplified Tauri client.
- shadcn/ui component prototype prompt for the initial React + Tailwind UI.
- Tauri replacement strategy for current egui GUI.
- Admin API mapping for simple top-level pages and advanced detail areas.
- Desktop lifecycle and tray behavior requirements, expressed in user-facing copy rather than architecture language.
- Validation strategy for frontend, Tauri shell, and Rust integration.
- Production frontend technology choices and repository layout.

## Out Of Scope

- Implementing the Tauri app in this planning slice.
- Removing the egui GUI immediately.
- Payment, user management, subscription purchase, affiliate, or promo-code features.
- Exposing the full route graph or every session override as top-level navigation in the first version.
- OS-specific signing, auto-update, and installer UX beyond noting them as follow-ons.

## Starting Assumptions

| Assumption | Confidence | Evidence | Consequence if wrong |
| --- | --- | --- | --- |
| The Tauri client is intended to replace the existing egui GUI. | High | User confirmation on 2026-05-20. | Migration plan must change to parallel long-term maintenance instead of replacement. |
| The frontend stack should be React 19 + Tailwind CSS 4 + shadcn/ui-style components + TanStack libraries. | High | User confirmation on 2026-05-20, refined on 2026-05-21. | Prototype and implementation tasks must be rewritten for another component stack. |
| The initial UI should be a component prototype, not a full app with live data. | High | User asked for shadcn components first. | Workstream should move directly into Tauri scaffold and API wiring. |
| The first GUI should be simple and familiar. | High | User feedback after seeing a too-complex prompt. | Reintroduce the full control-plane pages only after core UX is accepted. |
| `repo-ref/aio-coding-hub` is the closest local desktop-client structure reference. | High | It is already a Tauri + React + Tailwind local AI gateway client with app shell, provider, usage, and settings pages. | If its scope is copied too literally, the first codex-helper GUI becomes too broad. Borrow structure, not navigation breadth. |
| Admin API is the right primary integration boundary. | Medium | `control_plane_manifest.rs` exposes most needed surfaces. | Tauri may need direct Rust commands or shared types for missing host-local operations. |
| Desktop owner lifecycle semantics are ready to build on. | High | `desktop-lifecycle-owner` workstream completed and committed. | Need a lifecycle follow-up before real tray/sidecar implementation. |

## Product Direction

Register: product UI. The design serves day-to-day local proxy operation, not an infrastructure operations war room.

Theme scene sentence: a developer opens the client between coding sessions to check balance, request usage, current proxy state, and whether Codex is correctly connected. This favors a light default that feels approachable, with dark mode available for terminal/editor users.

Color strategy: restrained light product UI. Use warm/cool tinted neutrals, a teal primary accent for navigation and main actions, muted semantic colors for warnings/errors/success, and a very light page background wash. Avoid gradient text, neon, decorative glass, heavy dark dashboards, and large equal metric grids that overwhelm the first screen.

Product posture:

- It is a local proxy dashboard, not a full admin console.
- Default navigation should feel familiar: Dashboard, Providers, Usage, Settings.
- API keys and tokens are provider credentials, not a standalone top-level product object.
- Advanced controls should be discoverable via "Advanced", detail panels, or settings sections.
- The first screen should answer: proxy running or not, Codex connected or not, active provider/station, balance/usage health, and next simple action.
- Copy should use user-facing language:
  - "Connected to Codex"
  - "Local proxy running"
  - "Attached to existing proxy"
  - "Quit only detaches"
  - "Stop proxy"
  - "Run diagnosis"

## Reference Client Takeaways

`repo-ref/aio-coding-hub` is a better desktop-client reference than a SaaS admin dashboard because it is already a Tauri local gateway client. Use it for structure and interaction patterns, not for total feature scope.

Borrow:

- Fixed desktop app shell:
  - left sidebar;
  - main content area;
  - Tauri drag-safe top region;
  - startup/runtime status banner when needed.
- Sidebar bottom runtime card:
  - gateway/proxy status;
  - port;
  - update or warning state if relevant.
- Page-level header pattern:
  - title;
  - optional subtitle;
  - page-specific actions;
  - page-local tabs where useful.
- Dashboard composition:
  - combine runtime/work status, compact usage, recent requests, and provider health;
  - prefer a few meaningful panels over a large wall of equal metric cards.
- Providers page pattern:
  - service tabs if multiple client types are visible;
  - tag chips and search;
  - provider cards;
  - right-side active order/default route panel.
- Usage page pattern:
  - filter strip;
  - summary cards;
  - tabbed data panel for usage, cache, or availability views;
  - explicit query error, loading, stale, and desktop-unavailable states.
- Settings pattern:
  - simple responsive settings grid;
  - two adaptive columns at desktop width, stacking on narrower widths;
  - runtime state, app info, and paths as normal setting cards rather than a permanent right sidebar;
  - advanced sections grouped and collapsed.
- Code organization pattern for later implementation:
  - lazy routes;
  - thin page components;
  - page data-model hooks;
  - shared `PageHeader`, `EmptyState`, `QueryErrorCard`, `Skeleton`/`Spinner`, `Card`, and `Tabs` components.

Do not borrow for the first codex-helper GUI:

- broad top-level navigation for workspaces, prompts, MCP, skills, console, logs, and CLI manager;
- model-validation workflows as a first-class page;
- too much circuit/quota/session terminology on the first screen;
- decorative accents that make the shadcn component surface feel less standard.

## UI Brief And Image Concept Direction

`UI_BRIEF.md` is the current pre-imagegen UX/design brief. It synthesizes the user-provided AIO Coding Hub screenshot, `repo-ref/aio-coding-hub`, selected `repo-ref/awesome-design-md` systems, README product capabilities, admin API contracts, and existing egui/TUI surfaces.

Use the brief before image generation or shadcn/ui prototyping. Its current direction is:

- restrained mint/teal local developer dashboard;
- light theme first, dark mode available but not primary in the first concept;
- fixed Tauri sidebar with runtime footer;
- Dashboard answers proxy/Codex/provider/usage/health/action at a glance;
- top-level pages remain Dashboard, Providers, Usage, Settings;
- provider credentials/API keys live inside Providers as auth fields and detail/edit surfaces;
- advanced sessions/routing/diagnostics/traces/raw TOML remain in Advanced drawers or Settings;
- lifecycle copy must clearly distinguish Quit App, Detach, and Stop Proxy.

The imagegen prompt pack in `UI_BRIEF.md` should be treated as the next visual exploration input. If the generated concept is accepted, fold the accepted tokens/components/states back into `UI_BRIEF.md` or this `DESIGN.md` before implementation.

## Simplified MVP Sitemap

### 1. Dashboard

Purpose: calm account/runtime overview.

Primary regions:

- Compact status cards:
  - Local proxy status and port;
  - Codex connection/switch status;
  - Active provider/station;
  - today's requests/tokens/cost;
  - average response time.
- Work status panel:
  - Codex connection row;
  - Claude Code connection row if present;
  - active provider/station;
  - safe Start/Attach/Switch actions;
  - attached-mode note when relevant.
- Recent requests panel:
  - latest request rows;
  - status, model, tokens, cost, duration;
  - click target for detail drawer later.
- Provider breakdown panel:
  - provider name;
  - balance;
  - today's usage;
  - request count;
  - token count;
  - health badge.
- Usage charts:
  - model distribution or provider distribution;
  - token/cost trend.
- Simple quick actions:
  - Start Proxy;
  - Attach Existing;
  - Switch On;
  - Switch Off;
  - Refresh;
  - Run Diagnosis.

### 2. Providers

Purpose: manage relay providers/stations and their credentials without overwhelming the user.

In codex-helper, an API key is the provider auth field. It should appear in provider cards, provider detail, edit sheets, or an optional provider credential list mode, not as a standalone top-level page.

Primary regions:

- Provider cards or a provider credential list mode.
- Fields:
  - provider name;
  - base URL host;
  - auth source, such as env var, masked key, or missing credential;
  - balance;
  - active/default badge;
  - health;
  - latency;
  - capabilities summary;
  - recent requests.
- Actions:
  - set active;
  - probe;
  - refresh balance;
  - edit credentials/config;
  - enable/disable;
  - open advanced.
- Right-side active order/default route panel.

Advanced can be a collapsed section or side panel containing:

- model mapping summary;
- station/upstream list;
- route settings link;
- diagnostics link.

### 3. Usage

Purpose: request and cost history.

Primary regions:

- Summary cards:
  - total requests;
  - total tokens;
  - total estimated cost;
  - average duration.
- Filters:
  - key/provider/station;
  - date range;
  - model;
  - status.
- Usage table:
  - provider/key;
  - model;
  - effort;
  - endpoint;
  - type;
  - tokens;
  - cost;
  - first token time;
  - duration;
  - time.
- Inline detail popover/drawer for cost breakdown, retry chain, and route decision.
- Export CSV action.

### 4. Settings

Purpose: configuration, desktop lifecycle, and advanced tools.

Primary regions:

- Layout:
  - simple responsive two-column settings grid;
  - no permanent right-side status/about sidebar;
  - no giant full-width control-console form;
  - medium-sized cards with a title, one-line description, and compact controls.
- Desktop behavior:
  - launch at login;
  - tray enabled;
  - close behavior;
  - attached mode explanation.
- Appearance and language:
  - language;
  - theme;
  - optional density preference.
- Local proxy:
  - host;
  - port;
  - endpoint;
  - runtime owner;
  - admin token;
  - reload runtime.
- Codex/Claude connection:
  - switch status;
  - preset;
  - responses websocket;
  - backup status.
- About and paths:
  - version;
  - config path;
  - logs path;
  - cache path;
  - update check.
- Advanced:
  - sessions and overrides;
  - route graph;
  - diagnostics;
  - request trace;
  - raw TOML editor;
  - logs.

## Progressive Disclosure Rules

- Dashboard should not expose raw route graph, session override matrix, or full diagnostics.
- Providers should keep credential/common actions visible and advanced actions in row detail.
- Usage should show basic history first, then reveal retry/route details on demand.
- Settings is where advanced sections live, but they should be collapsed by default.
- A future Advanced page can be added only after simple MVP is accepted.

## Client Layout Direction

This is a desktop client, not a normal web page. The production app must use a fixed root shell:

- `html`, `body`, and `#root` should fill the window and avoid root-level browser scrolling.
- The sidebar is fixed inside the Tauri window and must not scroll away.
- The drag-safe top strip is part of the shell.
- The main page region owns overflow deliberately, and complex regions such as tables use internal scrolling, pagination, or sticky headers.
- Settings may scroll as a page region when the grid overflows, but it should remain compact and adaptive.

The TDC-020 prototype was adjusted after user feedback to demonstrate this fixed-shell direction. Treat `IMPLEMENTATION_BRIEF.md` as authoritative for production layout rules.

## Architecture Direction

Preferred integration path:

1. Build a simple React 19 + Tailwind CSS 4 + shadcn/ui-style + TanStack prototype from the prompt captured in this workstream.
2. Import the accepted prototype into a new production workspace under the recommended `apps/desktop` Tauri app after the implementation brief is accepted.
3. Replace mock data with an admin API client that consumes:
   - `/__codex_helper/api/v1/operator/summary`
   - `/__codex_helper/api/v1/runtime/status`
   - `/__codex_helper/api/v1/request-ledger/*`
   - `/__codex_helper/api/v1/stations*`
   - `/__codex_helper/api/v1/providers*`
   - `/__codex_helper/api/v1/codex/relay-capabilities`
   - `/__codex_helper/api/v1/codex/relay-live-smoke`
4. Use Tauri commands for host-local tasks that HTTP admin API should not own, such as spawning/owning the sidecar, opening files/log folders, reading local secure tokens, and tray events.
5. Keep the egui GUI during bring-up, then deprecate it after Tauri has parity for the simplified MVP plus lifecycle safety.

Current stack recommendation is captured in `IMPLEMENTATION_BRIEF.md`:

- Tauri v2;
- React 19 + TypeScript + Vite;
- Tailwind CSS 4 via `@tailwindcss/vite`;
- shadcn/ui-style components;
- TanStack Router, Query, and Table;
- React Hook Form + Zod for forms;
- Recharts through shadcn chart wrappers;
- optional Zustand only for small persisted shell preferences if React local state and router search params are not enough.

## Replacement Strategy

The replacement should be explicit and staged:

- Stage 1: simplified shadcn/ui prototype accepted.
- Stage 2: Tauri shell runs static app with mock data.
- Stage 3: read-only admin API wiring for Dashboard/Providers/Usage/Settings.
- Stage 4: safe mutations for switch, attach, provider/station probe, balance refresh, reload.
- Stage 5: desktop owner sidecar and tray behavior.
- Stage 6: advanced sessions/routing/diagnostics are added behind Settings or detail panels.
- Stage 7: egui GUI deprecation once parity gates pass.

## Closeout Condition

This lane can close when:

- the accepted shadcn/ui prototype has been imported or intentionally superseded;
- the Tauri desktop client can run the simplified MVP pages;
- lifecycle behavior matches owner semantics;
- targeted frontend/Tauri/Rust gates pass;
- docs reflect replacement status and advanced feature placement;
- remaining egui removal or installer/signing work is split into follow-on workstreams.

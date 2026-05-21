# Tauri Desktop Client

Status: Draft
Last updated: 2026-05-21

## Why This Lane Exists

`codex-helper` needs a long-term desktop client that replaces the existing egui GUI. The first version should not expose every internal control-plane concept. It should be built as a clean React + Tailwind + shadcn/ui component prototype first, then adapted into a Tauri app.

The desired first GUI is a simple local proxy dashboard:

- simple left navigation;
- account/runtime summary cards;
- API credential/provider list;
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
  - `repo-ref/sub2api/README_CN.md`
  - `repo-ref/sub2api/frontend/src/router/index.ts`
  - `repo-ref/aio-coding-hub/src/layout/AppLayout.tsx`
  - `repo-ref/aio-coding-hub/src/ui/Sidebar.tsx`
  - `repo-ref/aio-coding-hub/src/pages/HomePage.tsx`
  - `repo-ref/aio-coding-hub/src/pages/providers/ProvidersView.tsx`
  - `repo-ref/aio-coding-hub/src/pages/UsagePage.tsx`
  - `repo-ref/aio-coding-hub/src/pages/settings/SettingsPage.tsx`
- Existing local GUI/TUI surfaces:
  - `crates/gui/src/gui/pages/`
  - `crates/tui/src/tui/`
- Existing admin API and control-plane surface:
  - `crates/core/src/proxy/control_plane_manifest.rs`
  - `crates/core/src/dashboard_core/operator_summary.rs`
  - `crates/core/src/proxy/control_plane_routes/`

## Problem

The original product direction exposed too much as top-level navigation: stations, sessions, requests, diagnostics, route graph, and detailed observability. That shape is powerful but too complex for an initial GUI replacement.

The first Tauri GUI should reduce cognitive load:

- make normal users comfortable starting and observing the local proxy;
- present the most important usage and relay health information without explaining every route graph detail;
- keep API credential/provider and usage pages familiar;
- hide advanced station/session/routing/diagnostics controls behind "advanced" areas;
- preserve safe desktop owner semantics without making lifecycle theory a top-level concept.

## Target State

When this workstream closes:

- A simplified Tauri desktop client direction is documented as the replacement path for the current egui GUI.
- The first implementation slice starts from a React + Tailwind + shadcn/ui component prototype.
- The simple MVP sitemap is:
  - Dashboard
  - API Keys
  - Usage
  - Providers
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
| The frontend stack should be React + Tailwind with shadcn/ui components. | High | User confirmation on 2026-05-20. | Prototype and implementation tasks must be rewritten for another component stack. |
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
- Default navigation should feel familiar: Dashboard, API Keys, Usage, Providers, Settings.
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
  - main settings column;
  - secondary status/about/sidebar column;
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

### 2. API Keys

Purpose: familiar credential and provider entry list.

In codex-helper this page should represent "local credentials/provider entries" rather than a SaaS user's public API keys.

Primary regions:

- Search and filters.
- Table rows:
  - name;
  - masked token/env var;
  - provider/station tag;
  - status;
  - today's usage;
  - last used;
  - actions.
- Row actions:
  - copy local endpoint;
  - test/probe;
  - edit;
  - disable;
  - delete.
- Top actions:
  - Add provider;
  - Import from config;
  - Refresh balances.

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

### 4. Providers

Purpose: manage relay providers/stations without overwhelming the user.

Primary regions:

- Provider cards or a table.
- Fields:
  - provider name;
  - base URL host;
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
  - edit;
  - open advanced.

Advanced can be a collapsed section or side panel containing:

- model mapping summary;
- station/upstream list;
- route settings link;
- diagnostics link.

### 5. Settings

Purpose: configuration, desktop lifecycle, and advanced tools.

Primary regions:

- Desktop behavior:
  - launch at login;
  - tray enabled;
  - close behavior;
  - attached mode explanation.
- Local proxy:
  - service;
  - host;
  - port;
  - admin token;
  - reload runtime.
- Codex/Claude connection:
  - switch status;
  - preset;
  - responses websocket;
  - backup status.
- Advanced:
  - sessions and overrides;
  - route graph;
  - diagnostics;
  - request trace;
  - raw TOML editor;
  - logs.

## Progressive Disclosure Rules

- Dashboard should not expose raw route graph, session override matrix, or full diagnostics.
- API Keys and Providers should keep common actions visible and advanced actions in row detail.
- Usage should show basic history first, then reveal retry/route details on demand.
- Settings is where advanced sections live, but they should be collapsed by default.
- A future Advanced page can be added only after simple MVP is accepted.

## Architecture Direction

Preferred integration path:

1. Build a simple React + Tailwind + shadcn/ui prototype from the prompt captured in this workstream.
2. Import the accepted prototype into a new frontend workspace, likely under a future `apps/desktop` or `crates/desktop` Tauri package after repo layout is decided.
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

## Replacement Strategy

The replacement should be explicit and staged:

- Stage 1: simplified shadcn/ui prototype accepted.
- Stage 2: Tauri shell runs static app with mock data.
- Stage 3: read-only admin API wiring for Dashboard/API Keys/Usage/Providers/Settings.
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

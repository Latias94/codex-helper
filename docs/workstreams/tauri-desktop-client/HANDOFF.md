# Tauri Desktop Client — Handoff

Status: Closed with follow-ups
Last updated: 2026-05-22

## Current State

This workstream has reached replacement-readiness closeout. The Tauri desktop client is the preferred long-term replacement path for the existing egui GUI, but the released egui GUI remains in place until desktop parity and packaging gates pass. See `REPLACEMENT_READINESS.md` for the current decision, parity gaps, and follow-on split.

The user confirmed:

- replacement relationship: Tauri should become the long-term GUI, not merely a parallel experiment;
- frontend stack: React 19 + Tailwind CSS 4 + shadcn/ui-style components + TanStack libraries;
- workflow: first create a simple shadcn/ui component prototype, then have Codex convert/adapt/harden it into Tauri;
- simplified MVP pages: Dashboard, Providers, Usage, Settings;
- API keys/tokens are provider credentials and should live inside Providers, not as a standalone top-level page;
- advanced sessions/routing/diagnostics should be progressively disclosed, not top-level in the first GUI.

`repo-ref/aio-coding-hub` was reviewed as a local Tauri gateway-client reference. The useful patterns are its fixed desktop shell, left sidebar with runtime status footer, page header actions, dashboard work-status panels, provider cards plus right-side route order, and usage filters plus tabbed data panel. Do not copy its broad navigation scope.

Production Tauri code now exists under `apps/desktop`, and the original throwaway frontend prototype remains only as a historical design artifact under the workstream docs path.

On 2026-05-21, `UI_BRIEF.md` was added as the current pre-imagegen UX/design brief. It incorporates the user-provided AIO Coding Hub screenshot, selected `repo-ref/awesome-design-md` references, local product capabilities, admin API contracts, and existing egui surface inventory. It also contains the next imagegen prompt pack.

Later on 2026-05-21, the user accepted two concept images as references:

- `docs/workstreams/tauri-desktop-client/assets/dashboard-approved-v1.png`
- `docs/workstreams/tauri-desktop-client/assets/provider-credentials-list-reference-v1.png`

The accepted Dashboard is the visual shell baseline. The former API Keys concept is retained only as a provider credential list density reference: toolbar + clean table + security note, with no large proxy entrance card, no permanent right detail panel, and no repeated relay switch. `UI_BRIEF.md` now makes Providers the home for credentials and contains pagination/table-density rules plus the next Providers page imagegen prompt.

The user then accepted the Providers and revised Usage concepts:

- `docs/workstreams/tauri-desktop-client/assets/providers-approved-v1.png`
- `docs/workstreams/tauri-desktop-client/assets/usage-approved-v1.png`

For Settings, the first generated concept was rejected as too complex and saved only as an anti-reference:

- `docs/workstreams/tauri-desktop-client/assets/settings-generated-too-complex-v1.png`

The user prefers a simpler adaptive Settings layout, borrowing structure from:

- `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-a.png`
- `docs/workstreams/tauri-desktop-client/assets/settings-simple-reference-b.png`

Settings direction is now: simple responsive two-column setting cards, no permanent right sidebar, no giant full-width control-console form, advanced tools collapsed, and dangerous lifecycle actions compact but clearly separated at the bottom.

A revised Settings candidate was generated and saved as:

- `docs/workstreams/tauri-desktop-client/assets/settings-candidate-v2.png`

The user accepted that revised Settings candidate. It was copied to:

- `docs/workstreams/tauri-desktop-client/assets/settings-approved-v1.png`

TDC-018 is complete: Dashboard, Providers, Usage, and Settings now have accepted image baselines.

## Closeout State

TDC-020 is complete with concerns. A throwaway React 19 + Tailwind CSS 4 + shadcn/ui-style + TanStack prototype exists at:

- `docs/workstreams/tauri-desktop-client/prototype/`

Run it with:

```powershell
cd docs/workstreams/tauri-desktop-client/prototype
pnpm install
pnpm dev --host 127.0.0.1
```

It builds with `pnpm build`. The user reviewed it on 2026-05-21 and said the direction is good, with an important desktop-client layout correction: the sidebar must remain fixed and the app must not behave like a set of full-page scrolling browser pages. The prototype shell was adjusted accordingly: root overflow is hidden, the sidebar is fixed within the app viewport, and scrolling is moved into the main content region.

TDC-030 is complete: the user accepted the implementation brief and authorized production frontend work with fearless refactoring where it improves the architecture.

TDC-040 and TDC-050 are now scaffolded:

- `apps/desktop/` contains the production React 19 + Tailwind CSS 4 + shadcn-style + TanStack frontend.
- `apps/desktop/src-tauri/` contains the Tauri v2 Rust crate and is now a Cargo workspace member.
- The accepted Dashboard, Providers, Usage, and Settings prototype has been imported into production-oriented feature folders.
- The app shell uses a fixed root viewport, fixed sidebar, drag-safe title strip, and bounded main/panel scrolling.
- Existing `crates/gui` egui remains untouched as a fallback until replacement parity.

Run checks with:

```powershell
cd apps/desktop
pnpm install
pnpm test
pnpm build
cd ..\..
cargo fmt --check
cargo check -p codex-helper-desktop
```

TDC-060 is complete with concerns:

- `apps/desktop/src-tauri` now exposes `get_admin_read_model`, a read-only Tauri command that fetches the loopback admin API from `CODEX_HELPER_DESKTOP_ADMIN_URL` or the default `127.0.0.1:4211`.
- The frontend now maps `/operator/summary`, `/runtime/status`, `/providers`, `/request-ledger/recent`, and `/request-ledger/summary` into Dashboard, Providers, Usage, Settings, shell runtime footer, status strip, and page header badges.
- TanStack Query hooks preserve mock fallback for design iteration and display a visible `DataStateBanner` when showing offline sample data or refresh/error states.
- Validation passed on 2026-05-21: `pnpm test`, `pnpm build`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, `git diff --check -- .`, and loopback admin API smoke against `127.0.0.1:4211`.

TDC-070 is complete with concerns:

- `apps/desktop/src/lib/api/data-state.ts` defines the shared frontend state taxonomy for loading, live, refreshing, mock, desktop-runtime-unavailable, disconnected, auth-token-required, empty, and stale states.
- `apps/desktop/src/lib/api/use-admin-read-model.ts` centralizes the TanStack Query read-model boundary so Dashboard, Providers, Usage, Settings, shell footer, and page headers present consistent state.
- `DataStateBanner` now renders state-specific severity, badge, copy, icons, and retry actions instead of a single generic fallback message.
- Empty providers/usage states now teach the next action, and auth/disconnected/stale states explain what to fix before trying control actions.
- Owner semantics are intentionally shown as pending/uncertain in shell/status/settings copy; the frontend no longer pretends it knows whether the runtime is desktop-owned or attached.
- Validation passed on 2026-05-21: `pnpm test` (5 files, 20 tests), `pnpm build`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, and `git diff --check -- .`.

TDC-080 is complete with concerns:

- Tauri safe mutation commands exist for attach, desktop-managed start, owned stop, explicit attached remote stop, Codex switch on/off, runtime reload, station probe, provider balance refresh, provider runtime overrides, global route override, and session overrides.
- Desktop admin API requests now propagate `CODEX_HELPER_ADMIN_TOKEN` as `x-codex-helper-admin-token`.
- Frontend action hooks and status banners are wired into Dashboard, Providers, and Settings.
- Dangerous actions require exact inline phrases:
  - `STOP OWNED PROXY`
  - `STOP ATTACHED PROXY`
  - `SWITCH CODEX`
  - `SWITCH OFF CODEX`
- Validation passed on 2026-05-21: `pnpm build`, `pnpm test`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, `cargo nextest run -p codex-helper-desktop --lib`, and `git diff --check -- .`.

TDC-090 is complete with concerns:

- `apps/desktop/src-tauri/src/lifecycle.rs` now owns tray/window/app lifecycle behavior.
- Native main-window close requests hide to tray unless an explicit app quit was requested.
- The tray menu exposes Show Window, Hide to Tray, and Quit App (Proxy Keeps Running).
- Frontend custom titlebar buttons call Tauri window commands; titlebar close maps to hide-to-tray.
- Settings Dangerous Actions now distinguishes Quit App, Detach, and Stop Proxy; Quit App/Detach do not call `stop_proxy`.
- StatusStrip and Settings now show desktop-owned vs attached owner labels when `get_desktop_control_state` can classify the runtime.
- Validation passed on 2026-05-22: `pnpm test` (5 files, 22 tests), `pnpm build`, `cargo fmt --check`, `cargo check -p codex-helper-desktop`, `cargo nextest run -p codex-helper-desktop --lib` (9 tests), and `git diff --check -- .`.
- Follow-up Windows native close smoke also passed after hardening the listener registration: `PostMessage(WM_CLOSE)` left the desktop process alive, hid the window, and kept the existing `127.0.0.1:4211` proxy reachable before close, after close, and after desktop-process cleanup.

TDC-100/TDC-110 are complete. Do not remove egui yet. The Tauri client is documented as the preferred replacement path, but egui removal is explicitly blocked on parity gaps: full interactive packaged tray/window smoke, packaged sidecar lookup, single instance, launch at login, signing/installer behavior, auto-update policy, lightweight single-config import/export, open path actions, provider edit parity, and any remaining advanced controls.

On 2026-05-22, `IMPLEMENTATION_BRIEF.md` gained a Desktop Capability Matrix requested by the user. It tracks:

- desktop residency;
- system tray;
- auto update;
- launch at login;
- single instance;
- lightweight single-config import/export;
- open folders/paths;
- packaged sidecar.

Use that matrix as the source of truth for TDC-100 follow-up planning. Current support is intentionally marked partial/not implemented for anything beyond TDC-090 tray/window lifecycle behavior.

Important product boundary from the user: import/export should be simple because codex-helper has one primary config. Do not copy the heavier config/profile/workspace management style from `repo-ref/aio-coding-hub` or `repo-ref/cc-switch`; a future implementation should be closer to "export current config / import config with validation and backup".

## Follow-on Work

Open new workstreams or bounded tasks for:

- packaged sidecar and installer behavior;
- single instance;
- launch at login;
- signing and auto-update;
- lightweight single-config import/export with validation, backup, and secret warning;
- config/log/cache open-path actions;
- provider edit parity for common single-endpoint providers;
- full packaged tray/window lifecycle smoke;
- egui deprecation/removal after the above gates pass.

If a future agent resumes desktop work, start from `REPLACEMENT_READINESS.md`, not from the older prototype prompt below.

## shadcn/ui Prototype Prompt

```text
Create a high-fidelity React + Tailwind + shadcn/ui component prototype for a Tauri desktop client named codex-helper.

Important direction:
Keep the first version simple, approachable, and familiar. Build it from standard shadcn/ui primitives: Card, Button, Badge, Tabs, Table, DropdownMenu, Select, Input, Tooltip, Popover, Sheet, Switch, Separator, Skeleton. This should look like a clean local desktop dashboard, not a dense observability cockpit.

Product context:
codex-helper is a local desktop helper for Codex. It starts a local proxy, connects Codex to that proxy, lets the user manage relay providers and each provider's credential/auth source, shows request usage, token usage, cost estimates, balances, and basic relay health. Advanced routing, session overrides, and relay diagnostics exist, but they should be hidden behind Advanced sections in the first UI.

Reference structure:
Use the structure of a mature local Tauri control center:
- fixed left sidebar;
- main content area with page headers;
- sidebar footer showing local proxy status and port;
- page-local tabs when they reduce complexity;
- card panels, tables, popovers, drawers, and grouped settings.
Borrow these structural patterns only. Do not create a broad admin console.

Target user:
A developer who mostly wants to know:
1. Is the local proxy running?
2. Is Codex connected to it?
3. Which provider or station is active?
4. How much did I use today?
5. Are my providers healthy and do they still have balance?

Technology and output:
- React 19 components.
- Tailwind CSS 4.
- shadcn/ui component style.
- TanStack Router for page switching and TanStack Table for the Usage table.
- TanStack Query can be present as the future data-fetching boundary, but this prototype should use mock data only.
- Use realistic mock data.
- Desktop Tauri window around 1280 x 820.
- Prioritize clean static UI and component structure over complex interactions.
- Labels can be English, but layout must support Chinese text later.
- No login, registration, payment, subscription purchase, affiliate, or user-management pages.

Visual style:
- Light theme by default.
- Include a visible Dark Mode toggle in the sidebar, but the mockup can focus on the light theme.
- Restrained product UI.
- Primary accent: teal or cyan-green.
- Background: very light cool mint or blue wash, not plain white.
- Panels: white or near-white, soft shadow, rounded corners.
- Text: strong neutral headings, muted secondary text.
- Semantic colors: green for healthy/success, amber for warning, red for errors, purple/blue only for secondary metrics.
- Avoid neon, cyberpunk, heavy dark dashboards, gradient text, glassmorphism, and over-designed animations.

Navigation:
Use a simple left sidebar with these top-level pages:
1. Dashboard
2. Providers
3. Usage
4. Settings

Sidebar bottom:
- Dark Mode toggle
- Collapse sidebar
- Local proxy status card: Running/Stopped/Attached and port
- App version

Top bar:
- Page title and subtitle
- Notification icon
- Language selector
- Small runtime badge, for example Running or Attached
- Balance pill
- Local profile chip, for example Local User

Page 1: Dashboard
Goal: simple overview like an account/runtime dashboard.
Top region:
- Page header with title, subtitle, Refresh action, and a small runtime badge.
- Compact metric cards, but avoid a wall of identical cards.

Include compact metric cards:
- Local Proxy: Running, port 3211
- Codex Connection: Connected or Not Connected
- Active Provider: relay name
- Today's Requests
- Today's Tokens
- Estimated Cost
- Average Response Time
- Provider Health

Middle region:
- Work Status panel:
  - Codex row: connected, switch status, active provider, primary action.
  - Claude Code row if present: connected or inactive, active provider, primary action.
  - Safe actions: Start Proxy, Attach Existing, Switch On, Switch Off.
  - Attached-mode note if applicable.
- Recent Requests panel:
  - latest 5 requests;
  - model, status, input/output tokens, cost, duration, time;
  - small “View all usage” link.
- Provider breakdown panel: provider name, balance, today's cost, requests, tokens, health.
- Usage controls: time range and refresh button.
- Chart panel:
  - Token Usage Trend line or area chart.
  - Optional compact Model Distribution donut chart only if it does not crowd the page.

Quick actions should be visible but not overwhelming:
- Start Proxy
- Switch On
- Switch Off
- Refresh
- Run Diagnosis

If the app is attached to an existing runtime, show a small calm note: "Attached mode: closing this app only detaches. Use Stop Proxy in Settings to stop the runtime."

Page 2: Providers
Goal: familiar local provider management with credentials included.
In codex-helper, API keys, tokens, and env vars are provider auth fields. Do not make API Keys a standalone page.
Include:
- Search box
- Status filter
- Capability tags
- Provider cards as the default view
- Right-side Active Order or Default Route panel
- Provider card fields:
  - Provider name
  - Base URL host
  - Auth source, such as env CODEX_RELAY_API_KEY or masked key
  - Balance
  - Health
  - Latency
  - Capabilities: responses, compact, imagegen
  - Usage today
  - Last Used
- Actions:
  - Set Active
  - Probe
  - Refresh Balance
  - Edit
  - Disable

Include an optional compact credential list mode inside Providers:
- Provider
- Host
- Key or Env Var, masked
- Usage today
- Last Used
- Status
- Actions

Use one example row with realistic data:
- CodeX Air
- ai.input.im
- env CODEX_RELAY_API_KEY or sk-...da44
- today's cost and 30-day cost
- enabled

Page 3: Usage
Goal: request history and cost visibility.
Include summary cards:
- Total Requests
- Total Tokens
- Total Cost
- Average Duration

Filters:
- API key/provider
- Time range
- Model
- Status
- Buttons: Refresh, Reset, Export CSV

Table columns:
- API Key
- Model
- Reasoning Effort
- Endpoint
- Type
- Billing Mode
- Tokens, input/output/cache shown compactly
- Cost
- First Token
- Duration
- Time

Include a hover/click cost breakdown popover with input cost, output cost, cache read cost, service tier, multiplier, and final billed cost.

Advanced provider detail can be a collapsed section or side panel containing:
- model mapping summary;
- station/upstream list;
- route settings link;
- diagnostics link.

Page 4: Settings
Goal: grouped settings, not a giant form.
Layout:
- simple responsive two-column settings grid at desktop width;
- cards stack to one column on narrower widths;
- no permanent right sidebar for runtime/about;
- no giant full-width control-console form.

Sections:
1. Desktop Behavior
   - Launch at login
   - Tray enabled
   - Close behavior: Exit app / Minimize to tray
   - Startup behavior
2. Appearance And Language
   - Language
   - Theme: Follow system / Light / Dark
   - Optional density preference
3. Local Proxy
   - Host
   - Port
   - Endpoint
   - Runtime owner
   - Admin token status
   - Reload runtime button
4. Codex Connection
   - Switch status
   - Preset: default, chatgpt-bridge, official-relay, official-imagegen
   - Responses websocket toggle
   - Switch On / Switch Off
5. Advanced Tools, collapsed by default
   - Session overrides
   - Routing graph
   - Relay diagnostics
   - Request trace
   - Raw TOML editor
   - Logs folder
6. About And Paths
   - Version
   - Config path
   - Logs path
   - Cache path
   - Check update
7. Dangerous Actions
   - Quit App
   - Detach
   - Stop Proxy

Lifecycle copy requirements:
Use user-friendly terms, not architecture terms.
- "This app started the proxy" means quitting can stop it.
- "Attached to an existing proxy" means quitting only detaches.
- "Stop Proxy" must look more serious than "Quit App".

States to include somewhere in the design:
- running proxy;
- stopped proxy;
- attached mode;
- no providers configured;
- no usage records;
- provider health warning;
- diagnosis recommended;
- loading skeletons.
- desktop runtime unavailable hint;
- stale data or failed refresh state.

Output expectation:
Return a polished multi-page React + Tailwind + shadcn/ui mockup with the four pages above. Keep it spacious, friendly, simple, and not too technical. Advanced codex-helper features should exist as collapsed sections or small entry points, not as top-level pages.

Implementation structure expectation:
- Use reusable components such as AppShell, Sidebar, PageHeader, StatusBadge, MetricCard, ProviderCard, UsageTable, EmptyState, QueryErrorCard, SettingsSection, and DetailDrawer.
- Use mock data arrays and clean component boundaries.
- Avoid one giant component file if possible.
```

## Review Checklist For Returned Prototype

- Does Dashboard feel simple and friendly rather than like an observability platform?
- Are top-level pages limited to Dashboard, Providers, Usage, Settings?
- Are API keys represented as provider credentials instead of a standalone top-level page?
- Are advanced sessions/routing/diagnostics hidden behind Advanced, detail panels, or Settings?
- Are Quit, Detach, and Stop Proxy visually/textually distinct?
- Are tables readable without exposing too many internal fields?
- Does the sidebar footer clearly show proxy status and port?
- Does Dashboard include recent requests or work status, not just metric cards?
- Does Providers use cards plus active order/default route and show credential/auth source clearly?
- Does Settings avoid dumping raw TOML as the default path?
- Does the design avoid banned patterns:
  - gradient text;
  - decorative glass;
  - neon cyberpunk;
  - identical metric-card grids that dominate the page;
  - colored side-stripe cards;
  - modal-first confirmations?
## Implementation Notes

- Do not scaffold Tauri until the user returns and accepts or revises the prototype.
- Prefer admin API for live data and Tauri commands only for host-local desktop concerns.
- Keep egui GUI in place until replacement parity is proven.
- Any future code tasks should update `EVIDENCE_AND_GATES.md` with fresh command evidence.



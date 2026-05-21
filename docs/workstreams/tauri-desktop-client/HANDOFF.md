# Tauri Desktop Client — Handoff

Status: Draft
Last updated: 2026-05-21

## Current State

M0 is complete. This workstream is open to design and implement a Tauri desktop client that replaces the existing egui GUI. The user confirmed:

- replacement relationship: Tauri should become the long-term GUI, not merely a parallel experiment;
- frontend stack: React + Tailwind + shadcn/ui;
- workflow: first create a simple shadcn/ui component prototype, then have Codex convert/adapt/harden it into Tauri;
- simplified MVP pages: Dashboard, API Keys, Usage, Providers, Settings;
- advanced sessions/routing/diagnostics should be progressively disclosed, not top-level in the first GUI.

`repo-ref/aio-coding-hub` was reviewed as a local Tauri gateway-client reference. The useful patterns are its fixed desktop shell, left sidebar with runtime status footer, page header actions, dashboard work-status panels, provider cards plus right-side route order, usage filters plus tabbed data panel, and settings main-column/sidebar structure. Do not copy its broad navigation scope.

No code has been generated or imported yet.

## Next Task

TDC-020: user generates or assembles the initial shadcn/ui prototype using the prompt below, then returns source export, images, or a preview URL.

After that, run a product/UI critique before coding. If accepted, write `UI_BRIEF.md` or update `DESIGN.md` with the component inventory, data contracts, and import plan.

## shadcn/ui Prototype Prompt

```text
Create a high-fidelity React + Tailwind + shadcn/ui component prototype for a Tauri desktop client named codex-helper.

Important direction:
Keep the first version simple, approachable, and familiar. Build it from standard shadcn/ui primitives: Card, Button, Badge, Tabs, Table, DropdownMenu, Select, Input, Tooltip, Popover, Sheet, Switch, Separator, Skeleton. This should look like a clean local desktop dashboard, not a dense observability cockpit.

Product context:
codex-helper is a local desktop helper for Codex and Claude Code. It starts a local proxy, connects Codex to that proxy, lets the user manage relay providers and local API credentials, shows request usage, token usage, cost estimates, balances, and basic relay health. Advanced routing, session overrides, and relay diagnostics exist, but they should be hidden behind Advanced sections in the first UI.

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
- React components.
- Tailwind CSS.
- shadcn/ui component style.
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
2. API Keys
3. Usage
4. Providers
5. Settings

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

Page 2: API Keys
Goal: familiar local credential management.
In codex-helper, API Keys represent local provider credentials or env-token entries.
Include:
- Search box
- Group filter
- Status filter
- Local API endpoint pill: http://127.0.0.1:3211
- Buttons: Refresh, Add Key
- Table columns:
  - Name
  - Key or Env Var, masked
  - Provider/Station
  - Usage today
  - Last Used
  - Status
  - Actions
- Row actions:
  - Copy endpoint
  - Probe
  - Edit
  - Disable
  - Delete

Use one example row with realistic data:
- default
- CODEX_RELAY_API_KEY or sk-...da44
- CodeX Air
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

Page 4: Providers
Goal: simple provider/station health and balance management without exposing route graphs by default.
Include:
- Service tabs if useful: Codex, Claude Code, All.
- Search box and simple tag chips.
- Provider cards as the default presentation.
- Right-side “Active Order” or “Default Route” panel showing enabled providers in priority order.
- Fields:
  - provider name;
  - base URL host;
  - balance;
  - active/default badge;
  - health;
  - latency;
  - capabilities summary: models, responses, compact, imagegen, websocket;
  - recent requests.
- Actions:
  - Set Active
  - Probe
  - Refresh Balance
  - Edit
  - Advanced

Advanced can be a collapsed section or side panel containing:
- model mapping summary;
- station/upstream list;
- route settings link;
- diagnostics link.

Page 5: Settings
Goal: grouped settings, not a giant form.
Layout:
- main settings column for editable settings;
- right sidebar for app/about/runtime summary and quick links.

Sections:
1. Desktop Behavior
   - Launch at login
   - Tray enabled
   - Close behavior: Exit app / Minimize to tray
   - Startup behavior
2. Local Proxy
   - Service: Codex / Claude
   - Host
   - Port
   - Admin token status
   - Reload runtime button
3. Codex Connection
   - Switch status
   - Preset: default, chatgpt-bridge, official-relay, official-imagegen
   - Responses websocket toggle
   - Switch On / Switch Off
4. Advanced Tools, collapsed by default
   - Session overrides
   - Routing graph
   - Relay diagnostics
   - Request trace
   - Raw TOML editor
   - Logs folder

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
Return a polished multi-page React + Tailwind + shadcn/ui mockup with the five pages above. Keep it spacious, friendly, simple, and not too technical. Advanced codex-helper features should exist as collapsed sections or small entry points, not as top-level pages.

Implementation structure expectation:
- Use reusable components such as AppShell, Sidebar, PageHeader, StatusBadge, MetricCard, ProviderCard, UsageTable, EmptyState, QueryErrorCard, SettingsSection, and DetailDrawer.
- Use mock data arrays and clean component boundaries.
- Avoid one giant component file if possible.
```

## Review Checklist For Returned Prototype

- Does Dashboard feel simple and friendly rather than like an observability platform?
- Are top-level pages limited to Dashboard, API Keys, Usage, Providers, Settings?
- Are advanced sessions/routing/diagnostics hidden behind Advanced, detail panels, or Settings?
- Are Quit, Detach, and Stop Proxy visually/textually distinct?
- Are tables readable without exposing too many internal fields?
- Does the sidebar footer clearly show proxy status and port?
- Does Dashboard include recent requests or work status, not just metric cards?
- Does Providers use cards plus active order/default route instead of only a dense table?
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



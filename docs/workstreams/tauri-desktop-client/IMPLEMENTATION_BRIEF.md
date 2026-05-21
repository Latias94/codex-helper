# Tauri Desktop Client — Implementation Brief

Status: Draft
Last updated: 2026-05-21

## Research Sources

Primary sources reviewed for this brief:

- React 19 release notes: https://react.dev/blog/2024/12/05/react-19
- Tailwind CSS with Vite: https://tailwindcss.com/docs/installation/using-vite
- Tauri v2 create project: https://v2.tauri.app/start/create-project/
- Tauri v2 configuration reference: https://v2.tauri.app/reference/config/
- Tauri v2 calling Rust from frontend: https://v2.tauri.app/develop/calling-rust/
- Tauri v2 system tray: https://v2.tauri.app/learn/system-tray/
- shadcn/ui Vite install: https://ui.shadcn.com/docs/installation/vite
- shadcn/ui Data Table: https://ui.shadcn.com/docs/components/data-table
- shadcn/ui Chart: https://ui.shadcn.com/docs/components/chart
- shadcn/ui React Hook Form: https://ui.shadcn.com/docs/forms/react-hook-form
- shadcn/ui TanStack Form: https://ui.shadcn.com/docs/forms/tanstack-form
- shadcn/ui Sidebar: https://ui.shadcn.com/docs/components/sidebar
- TanStack Router overview: https://tanstack.com/router/latest/docs/framework/react/overview
- TanStack Query overview: https://tanstack.com/query/latest/docs/framework/react/overview
- TanStack Table introduction: https://tanstack.com/table/latest/docs/introduction
- Zod docs: https://zod.dev/
- Zustand introduction: https://zustand.docs.pmnd.rs/getting-started/introduction

## Decision Summary

Recommended production stack:

- Desktop shell: Tauri v2.
- Frontend runtime: React 19 + TypeScript + Vite.
- Styling: Tailwind CSS 4 through `@tailwindcss/vite`.
- Component system: shadcn/ui-style components with Radix primitives, `lucide-react`, `class-variance-authority`, `clsx`, and `tailwind-merge`.
- Routing: TanStack Router.
- Server state: TanStack Query.
- Tables: TanStack Table.
- Forms: React Hook Form + Zod + `@hookform/resolvers` as the first production choice.
- Charts: shadcn Chart component pattern backed by Recharts.
- Client-only state: prefer React local state and URL/search params first; add Zustand only for small persisted shell preferences if the app outgrows local state.
- Desktop API boundary: use the existing loopback admin API for runtime/product data; use Tauri commands only for host-local desktop capabilities.

Do not scaffold production Tauri code until this brief is accepted.

## Client Layout Rules

The desktop client should feel like a native utility window, not a browser page.

Hard rules:

- The app root owns the viewport: `html`, `body`, and `#root` should be full-height with root overflow hidden.
- The left sidebar is fixed inside the app shell and must not scroll away.
- The Tauri drag-safe top strip remains fixed with the shell.
- The main content area is the primary scroll container only when the page needs overflow.
- Large regions such as usage tables, provider card grids, and settings sections should use internal scroll/pagination/sticky headers where that is better than scrolling the whole page.
- Page headers should remain visually stable; long pages can scroll below the header or within a bounded panel.
- Avoid "everything is one long web page" behavior.

Recommended shell structure:

```tsx
<div className="flex h-screen overflow-hidden">
  <aside className="h-full shrink-0">...</aside>
  <main className="flex min-h-0 flex-1 flex-col">
    <div className="drag-region h-8 shrink-0" />
    <section className="no-drag min-h-0 flex-1 overflow-hidden">
      <Outlet />
    </section>
  </main>
</div>
```

Recommended page convention:

- `Dashboard`: `overflow-hidden`; cards and recent activity fit the 1280 x 820 target, with secondary panels allowed to scroll internally.
- `Providers`: provider list/card area can scroll; route order panel can stay sticky in the content column.
- `Usage`: table panel owns vertical scroll and sticky table header; pagination footer stays attached to the table panel.
- `Settings`: settings grid can scroll in the main content region, but cards remain compact; no permanent right sidebar.

## Library Decisions

### React 19

Use React 19 as the frontend baseline. Keep React Server Components out of the Tauri client unless a future packaging/runtime decision explicitly justifies them. For this app, React 19 mainly provides the current stable client runtime, improved ref handling, and modern form/action primitives if needed later.

Practical rules:

- Prefer explicit client-side data fetching through TanStack Query.
- Keep components small and page-level data dependencies obvious.
- Use React local state for transient UI state before introducing global stores.

### Tailwind CSS 4

Use Tailwind CSS 4 with the official Vite plugin. This keeps the Tauri frontend close to current Vite best practices and avoids carrying Tailwind v3-era config assumptions unless a plugin specifically needs them.

Practical rules:

- Keep design tokens in `src/styles/globals.css` via Tailwind v4 CSS-first theme variables.
- Keep `components.json` aligned with the chosen shadcn style.
- Prefer utility classes plus small component variants over bespoke CSS files.

### shadcn/ui

Use shadcn/ui as copied source components, not as a black-box component dependency. This matches the product need for a polished but custom local desktop UI.

Initial components:

- Layout and display: `Card`, `Badge`, `Separator`, `ScrollArea`, `Resizable` if needed later.
- Controls: `Button`, `Input`, `Select`, `Switch`, `Tabs`, `DropdownMenu`, `Popover`, `Tooltip`, `Sheet`, `Dialog`.
- Feedback: `Alert`, `Skeleton`, `Sonner`.
- Data: `Table`, `DataTable` pattern with TanStack Table.
- Charts: `ChartContainer` pattern with Recharts.
- Forms: shadcn form pattern with React Hook Form + Zod.
- Sidebar: shadcn Sidebar can be used, but adapt it to fixed Tauri shell semantics rather than accepting page-scroll defaults.

### TanStack Router

Use TanStack Router for typed routes and URL/search-param ownership.

Recommended route shape:

```text
src/routes/
  __root.tsx
  index.tsx
  providers.tsx
  usage.tsx
  settings.tsx
```

Rules:

- Keep the first sitemap fixed: Dashboard, Providers, Usage, Settings.
- Use search params for filters, pagination, and selected provider/detail state when it should be shareable or restorable.
- In Tauri, prefer a history mode that works with bundled static assets; verify whether hash history is needed during the TDC-040 scaffold.

### TanStack Query

Use TanStack Query for all server/admin API state:

- operator summary;
- runtime status;
- provider catalog and balances;
- request ledger/usage rows;
- capability probes;
- mutations that call admin endpoints.

Rules:

- Query keys live in one feature-adjacent module, not scattered strings.
- Use short stale times for runtime status and usage; longer stale times for static config.
- Surface stale/disconnected/auth-token-required states as first-class UI states.
- Tauri commands should return data that can still be cached by TanStack Query when they represent async desktop state.

### Forms

Recommended first choice: React Hook Form + Zod.

Reasoning:

- shadcn has mature form examples around React Hook Form and Zod.
- Zod keeps runtime validation and TypeScript inference close together.
- codex-helper settings/provider forms are mostly schema-driven config forms, not large collaborative document forms.

Where to use:

- provider credential edit sheet;
- local proxy host/port settings;
- desktop behavior settings;
- Codex connection preset form;
- advanced raw config guard forms.

TanStack Form is a valid future option for very complex forms, but adding it now would duplicate form paradigms without a clear payoff.

### Charts

Use Recharts via shadcn chart wrappers for the first dashboard:

- token/cost trend area or line chart;
- provider/model distribution bar or donut;
- simple tooltips with "estimated cost" language.

If future datasets become large or highly interactive, evaluate a lower-level charting library later. Do not optimize prematurely for high-frequency observability.

### State Management

Split state by ownership:

| State kind | Owner |
| --- | --- |
| Runtime/admin API data | TanStack Query |
| Route, selected page, filters, pagination | TanStack Router search params |
| Open/closed drawers, tabs, popovers | React local state |
| Form drafts and validation | React Hook Form + Zod |
| Theme/language/sidebar collapsed/density | Small persisted client store if needed |
| Desktop lifecycle events | Tauri event bridge + Query invalidation |

Recommendation:

- Do not add Redux.
- Do not add Zustand on day one unless persistent shell preferences become awkward with React context/local storage.
- If a store is needed, choose Zustand and restrict it to shell/UI preferences, not server state.

## Desktop Integration Boundary

Use two boundaries deliberately:

1. Admin API for product/runtime data.
2. Tauri commands for host-local desktop capabilities.

Admin API should own:

- Dashboard operator summary.
- Runtime status and attached/running mode.
- Providers, route order, health, balances.
- Usage/request ledger.
- Capability and diagnostic results when already exposed over loopback admin routes.

Tauri commands should own:

- starting a desktop-owned sidecar;
- attaching/detaching a resident runtime;
- tray show/hide/quit behavior;
- open config/log/cache folders;
- read app version and platform metadata;
- OS autostart;
- secure local credential helpers if added later;
- explicit Stop Proxy when it requires owner semantics not represented by a generic HTTP call.

Lifecycle rules:

- Normal window close does not remote-stop an attached runtime.
- Quit App, Detach, and Stop Proxy remain separate UI actions.
- Dangerous actions require inline explanation and stronger visual treatment.

## Recommended Repository Layout

Recommended option: add a new production app under `apps/desktop`, while keeping the current egui crate until replacement parity is proven.

```text
apps/
  desktop/
    package.json
    pnpm-lock.yaml
    components.json
    index.html
    vite.config.ts
    tsconfig.json
    src/
      main.tsx
      styles/
        globals.css
      app/
        App.tsx
        AppShell.tsx
        query-client.ts
        router.tsx
      routes/
        __root.tsx
        index.tsx
        providers.tsx
        usage.tsx
        settings.tsx
      components/
        ui/
        shell/
        page/
        data-table/
        charts/
      features/
        dashboard/
          components/
          hooks/
          types.ts
        providers/
          components/
          hooks/
          schemas.ts
          types.ts
        usage/
          components/
          hooks/
          types.ts
        settings/
          components/
          hooks/
          schemas.ts
          types.ts
      lib/
        api/
          client.ts
          query-keys.ts
          types.ts
        tauri/
          commands.ts
          events.ts
        format/
        utils.ts
      mocks/
    src-tauri/
      Cargo.toml
      tauri.conf.json
      build.rs
      src/
        main.rs
        lib.rs
        commands/
          lifecycle.rs
          paths.rs
          runtime.rs
          settings.rs
```

Root workspace changes for TDC-040:

- Add `apps/desktop/src-tauri` as a Cargo workspace member.
- Keep `crates/gui` and the existing `codex-helper-gui` binary unchanged until replacement readiness.
- Make `apps/desktop/src-tauri` depend on `codex-helper-core`.
- Keep `docs/workstreams/tauri-desktop-client/prototype` throwaway and do not make it part of production tooling.

Why `apps/desktop` over `crates/desktop`:

- Tauri apps naturally combine a JavaScript package with a Rust `src-tauri` crate.
- Keeping the production desktop app under `apps/` separates product packaging from reusable Rust crates.
- It avoids mixing Node artifacts into `crates/`, while still letting `src-tauri` join the Cargo workspace.
- It makes a future `apps/web` or documentation preview possible without disturbing Rust crate naming.

Rejected alternatives:

- Replace `crates/gui` in place: too risky before parity and makes rollback harder.
- Put React under `crates/gui`: conflates egui and Tauri implementations.
- Keep production app under `docs/workstreams`: docs/prototype assets should remain disposable, not release artifacts.

## Production Scaffold Package Set

Baseline npm dependencies for TDC-040/TDC-050:

```text
react
react-dom
@tauri-apps/api
@tanstack/react-router
@tanstack/react-query
@tanstack/react-table
react-hook-form
zod
@hookform/resolvers
recharts
lucide-react
class-variance-authority
clsx
tailwind-merge
sonner
```

Baseline dev dependencies:

```text
@tauri-apps/cli
vite
@vitejs/plugin-react
typescript
tailwindcss
@tailwindcss/vite
tw-animate-css
vitest
@testing-library/react
@testing-library/user-event
jsdom
```

Pin exact versions in `package.json` during scaffold to keep desktop builds reproducible.

Versions observed on 2026-05-21 during planning:

| Package | Observed version |
| --- | --- |
| `react` | `19.2.6` |
| `tailwindcss` | `4.3.0` |
| `@tailwindcss/vite` | `4.3.0` |
| `vite` | `8.0.14` |
| `@vitejs/plugin-react` | `6.0.2` |
| `@tauri-apps/cli` | `2.11.2` |
| `@tauri-apps/api` | `2.11.0` |
| `shadcn` CLI | `4.7.0` |
| `@tanstack/react-router` | `1.170.6` |
| `@tanstack/react-query` | `5.100.11` |
| `@tanstack/react-table` | `8.21.3` |
| `@tanstack/react-form` | `1.32.0` |
| `react-hook-form` | `7.76.0` |
| `zod` | `4.4.3` |
| `@hookform/resolvers` | `5.2.2` |
| `recharts` | `3.8.1` |
| `zustand` | `5.0.13` |
| `lucide-react` | `1.16.0` |

## Type And Contract Strategy

Short term:

- Define frontend DTOs in `src/lib/api/types.ts`.
- Keep DTO names close to Rust/admin API concepts but with UI-safe naming.
- Use Zod schemas for form input and any local persistence payloads.
- Treat API responses as unknown at the boundary, validate only high-risk config mutations first to avoid heavy boilerplate.

Medium term:

- Evaluate Rust-to-TypeScript type generation after the first production shell is stable.
- Candidate tools can be reviewed later if needed; do not block TDC-040 on type generation.

## Testing And Validation

TDC-040 shell gates:

- `pnpm install`
- `pnpm build`
- `pnpm test` once Vitest exists
- `cargo fmt --check`
- `cargo check -p codex-helper-desktop` or equivalent Tauri crate package name

TDC-050 UI import gates:

- Frontend build/typecheck.
- Component tests for route rendering and core empty/error states.
- Browser visual smoke at 1280 x 820 for Dashboard, Providers, Usage, Settings.

TDC-060+ integration gates:

- API client mapping tests.
- Manual smoke against resident local proxy.
- Lifecycle matrix before any replacement claim.

## Open Decisions Before TDC-040

- Confirm final production app path: recommended `apps/desktop`.
- Confirm package manager: recommended `pnpm`.
- Confirm whether Tauri router should use hash history for packaged static assets.
- Confirm app identifier and window defaults.
- Decide whether production scaffold should import the current prototype directly or recreate components with official shadcn generated files first.

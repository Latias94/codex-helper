# TDC-020 Prototype — Throwaway

This is a **throwaway UI prototype** for the `tauri-desktop-client` workstream.

Question answered by this prototype:

> Can the accepted Dashboard / Providers / Usage / Settings image baselines be translated into a simple, maintainable React 19 + Tailwind CSS 4 + shadcn/ui-style + TanStack component prototype before production Tauri scaffold work starts?

Tech stack:

- React 19
- Vite
- Tailwind CSS 4 via `@tailwindcss/vite`
- shadcn/ui-style local primitives
- TanStack Router
- TanStack Table
- TanStack Query boundary with mock data only

Run:

```powershell
pnpm install
pnpm build
pnpm dev --host 127.0.0.1
```

Notes:

- This is not the production Tauri app.
- No live admin API calls are made.
- Mock data is intentionally local and disposable.
- The shell is intentionally client-style: fixed sidebar, fixed root viewport, and scroll only in the main content region.
- Delete or absorb this prototype after the UI direction is accepted.

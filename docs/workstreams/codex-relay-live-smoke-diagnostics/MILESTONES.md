# Codex Relay Live Smoke Diagnostics — Milestones

Status: Active
Last updated: 2026-05-19

## M0 — Safety Contract

Exit criteria:

- Live smoke is explicitly separated from capability diagnostics.
- Opt-in acknowledgement and one-upstream/no-retry constraints are written down.
- Image generation cost risk is explicitly named.

Primary evidence:

- `DESIGN.md`
- `TODO.md`

## M1 — Core Contract

Exit criteria:

- Core DTOs represent requested cases, opt-in state, target, per-case result, and warnings.
- Request builders produce Codex-shaped remote compaction and hosted image generation requests.
- Classifiers detect compact output and image-generation call output without retaining large payloads.
- Unit/integration tests prove one request per selected case.

Primary gates:

- `cargo nextest run -p codex-helper-core codex_relay_live_smoke`

## M2 — Operator API

Exit criteria:

- Admin API exposes a live-smoke endpoint.
- Capabilities and operator summary advertise the link.
- Missing acknowledgement returns a client error before upstream IO.

Primary gates:

- `cargo nextest run -p codex-helper-core codex_live_smoke_api`

## M3 — TUI Flow

Exit criteria:

- Settings page exposes live smoke as a deliberate confirmed action.
- Results render separately from validation-only diagnostics.
- Stale async results are ignored.

Primary gates:

- `cargo nextest run -p codex-helper-tui codex_relay_live_smoke`

## M4 — Closeout

Exit criteria:

- Formatting and targeted tests pass.
- Docs and changelog describe manual cost-bearing behavior.
- Remaining live relay smoke evidence is recorded or explicitly deferred.


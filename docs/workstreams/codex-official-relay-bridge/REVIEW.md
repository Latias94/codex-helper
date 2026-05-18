# Codex Official Relay Bridge — Review

Date: 2026-05-18
Scope: CORB-020, CORB-030, CORB-040, readiness for CORB-050 closeout.

## Workstream Compliance

- Blocking: none found.
- Important: none found.
- Minor: none found.

The implementation satisfies the first-stage target: Codex can be patched into an official-looking
HTTP Responses provider for remote compaction v1, helper keeps relay credentials in its routing
layer, `/responses/compact` is routed and logged, and unsupported compact relays have a documented
fallback path.

## Code Quality

- Blocking: none found.
- Important: none found.
- Minor: none found.

The change keeps the new behavior behind an explicit `official-relay-bridge` mode, reuses existing
bridge credential stripping, keeps WebSocket disabled, and adds request-ledger path filtering without
changing the proxy forwarding path.

## Missing Gates

- `cargo nextest run --workspace` has not been run yet. Current evidence includes `cargo fmt --check`,
  full `codex-helper-core` nextest, targeted GUI request-ledger tests, and CLI/TUI/GUI package
  checks.

## Residual Risk

- `name = "OpenAI"` may enable future Codex provider branches beyond remote compaction v1 if upstream
  Codex changes semantics. Current mitigation is explicit opt-in plus `supports_websockets = false`.
- Remote compaction v2 and WebSocket remain follow-ons because helper has no upgrade proxy and Codex
  v2 semantics are distinct from `/responses/compact`.
- No live relay was exercised; tests use local upstreams to prove helper routing and diagnostics.

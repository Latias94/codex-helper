# Codex Session Route Continuity - Handoff

Status: Complete
Last updated: 2026-05-25

Update, 2026-05-27: later route-continuity work made route-graph fallback behavior
policy-sensitive. `fallback-sticky` may bootstrap missing state-bound compact affinity by trying
the configured route and recording the successful endpoint; `hard` multi-endpoint route graphs
still fail closed when state-bound compact affinity is missing.

## Current Task

None. Workstream closed.

## Context

The target failure involved a Codex session whose compact requests stayed on
`codex/input7/default` until helper restart. After restart, session route
affinity was missing and route selection chose `codex/input2/default`.

The fix should not assume the provider is backed by OpenAI, sub2api, new-api,
or any specific relay implementation. The proxy should persist and restore only
provider-opaque facts it owns.

Compact failure fallback is a separate CSRC-030 concern. Non-state-bound compact
can use normal provider fallback. State-bound compact now follows the active
affinity policy: `fallback-sticky` can continue through the route graph and
update affinity, while `hard` stays on the affinity endpoint unless an explicit
continuity domain proves multiple endpoints share safe upstream state.

## Next Step

No further work is required for this lane. Ask the user before committing.

## Validation

Final gates:

- `cargo fmt --all --check`
- `cargo nextest run -p codex-helper-core`

## Completed

- CSRC-020: Durable session route affinity ledger.
- CSRC-030: Explicit compact continuity policy.
- CSRC-040: Provider-opaque runtime signals.
- CSRC-050: Documentation and closeout.
- Validation passed:
  - `cargo nextest run -p codex-helper-core session_route_affinity --no-fail-fast`
  - `cargo nextest run -p codex-helper-core proxy_restores_route_affinity_after_restart_for_responses_compact --no-fail-fast`
  - `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`
  - `cargo nextest run -p codex-helper-core route_affinity request_logs --no-fail-fast`
  - `cargo nextest run -p codex-helper-core control_trace route_graph_selection_explain --no-fail-fast`
  - `cargo fmt --all --check`
  - `cargo nextest run -p codex-helper-core` passed with 693 tests.

## Residual Risks And Follow-Ups

- State-bound compact either uses known route affinity, bootstraps through an
  explicitly tryable policy such as `fallback-sticky`, or fails closed. Hard
  cross-provider movement still requires an explicit operator-configured
  continuity domain.
- Balance probes remain routing/runtime hints, not proof that compact state can
  safely move between provider endpoints.

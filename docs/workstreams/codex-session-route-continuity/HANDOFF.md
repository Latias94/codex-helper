# Codex Session Route Continuity - Handoff

Status: Complete
Last updated: 2026-05-25

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
can use normal provider fallback. State-bound compact should fail closed or stay
on the affinity endpoint unless a future explicit continuity-domain feature
proves multiple endpoints share safe upstream state.

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

- State-bound compact only uses the known provider endpoint or fails closed.
  Cross-provider fallback for state-bound compact should be a new workstream
  with an explicit operator-configured continuity domain.
- Balance probes remain routing/runtime hints, not proof that compact state can
  safely move between provider endpoints.
- The route affinity ledger is provider-opaque and can be disabled with
  `CODEX_HELPER_SESSION_ROUTE_AFFINITY_LEDGER=off`, but disabling it reopens
  the original restart-continuity risk.

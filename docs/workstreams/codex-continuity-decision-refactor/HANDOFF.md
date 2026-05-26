# Codex Continuity Decision Refactor - Handoff

Status: Complete
Last updated: 2026-05-26

## Current State

Workstream opened for the global continuity/affinity refactor.

CDC-020, CDC-030, CDC-040, CDC-050, CDC-060, and CDC-070 are complete. This lane is closed.

## Key Decisions

- Domain-name equality is not sufficient proof for shared encrypted state.
- Relay endpoints remain provider-opaque by default.
- `continuity_domain` should be explicit before state-bound fallback crosses provider endpoints.
- Ordinary conversation affinity should be soft.
- Compact and encrypted-state affinity should be hard unless one continuity domain is proven.
- Configured `Hard` affinity is now interpreted through request continuity: provider-state-bound compact keeps hard/configured selection, while ordinary conversation turns use soft session preference and can escape an unavailable pinned endpoint.
- `continuity_domain` is explicit only. Provider/endpoint config can set it; endpoint values override provider values. When absent, the effective domain is the provider endpoint itself, so same host/base URL/domain never proves encrypted state sharing.
- State-bound provider failover is allowed only after route selection is restricted to an explicit shared `continuity_domain`.
- Runtime upstream identity migration treats continuity-domain changes like base URL changes and resets retained state.
- Capability/profile diagnostics now state that OpenAI identity selects the compact protocol path but does not prove upstream encrypted-state sharing.
- Relay capability diagnostics report the selected continuity domain, whether it is explicit, how many configured endpoints share it, and operator warnings/recommendations.

## First Files To Inspect

- `crates/core/src/proxy/request_continuity.rs`
- `crates/core/src/routing_ir.rs`
- `crates/core/src/proxy/request_preparation.rs`
- `crates/core/src/proxy/request_body.rs`
- `crates/core/src/proxy/provider_execution.rs`
- `crates/core/src/proxy/responses_websocket.rs`
- `crates/core/src/proxy/route_affinity.rs`
- `crates/core/src/proxy/tests/failover/response_semantics.rs`

## Closeout

Completed CDC-070:

1. Final targeted regression set passed.
2. `cargo nextest run -p codex-helper-core --no-fail-fast` passed: 721 tests.
3. `cargo check -p codex-helper` passed.
4. `cargo fmt --all --check` passed.
5. `git diff --check` passed with only line-ending normalization warnings.

## Follow-Ons

- Consider an official-OpenAI-only continuity heuristic later, but only if it can prove canonical `api.openai.com`, credential source, and org/project identity. Relay endpoints must remain explicit-only.
- Consider surfacing `continuity_domain` editing in TUI/GUI provider editors if operator UX needs it.
- Consider including continuity-domain fields in request ledger summaries if operators need post-incident filtering beyond control trace and relay diagnostics.

## Risks

- Public capability diagnostics now include new `expected.continuity` and top-level `continuity` fields. JSON clients should tolerate additive fields.
- State-bound failover inside explicit `continuity_domain` trusts operator configuration; wrong domains can still move encrypted state to an incompatible relay account.
- Multiple provider endpoints without prior affinity still fail closed for state-bound compact. A future bootstrap-over-one-explicit-domain behavior would need its own design and tests.

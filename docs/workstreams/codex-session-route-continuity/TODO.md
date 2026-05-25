# Codex Session Route Continuity - TODO

Status: Complete
Last updated: 2026-05-25

## M0 - Scope And Evidence Freeze

- [x] CSRC-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-session-route-continuity]
  Goal: Freeze the problem, target state, non-goals, and evidence anchors for
  provider-opaque Codex session route continuity.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Evidence: docs/workstreams/codex-session-route-continuity/DESIGN.md
  Handoff: Ready for the first executable slice.

## M1 - Durable Route Ledger Proof

- [x] CSRC-020 [owner=codex] [deps=CSRC-010] [scope=crates/core/src/state.rs,crates/core/src/state/session_route_ledger.rs,crates/core/src/proxy/tests]
  Goal: Persist and restore session route affinity by provider endpoint identity
  so a helper restart does not make a Codex session silently choose a new
  provider endpoint.
  Validation: cargo nextest run -p codex-helper-core session_route_affinity --no-fail-fast
  Review: Verify persistence is provider-opaque, pruned, and disabled only by explicit configuration or invalid topology.
  Evidence: `cargo nextest run -p codex-helper-core session_route_affinity --no-fail-fast`; `cargo nextest run -p codex-helper-core proxy_restores_route_affinity_after_restart_for_responses_compact --no-fail-fast`.
  Handoff: DONE. Session route affinity now persists in a provider-opaque ledger and restores across proxy state recreation.

## M2 - Continuity Policy And Fail-Closed Compact

- [x] CSRC-030 [owner=codex] [deps=CSRC-020] [scope=crates/core/src/proxy/provider_execution.rs,crates/core/src/proxy/tests]
  Goal: Replace compact/failover boolean composition with an explicit
  continuity decision for stateless, session-preferred, and provider-state-bound
  requests, including what happens when the affinity provider endpoint fails.
  Validation: cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast
  Review: Non-state-bound compact may fallback like a normal session request; state-bound compact must not silently choose a new provider endpoint when durable affinity is missing or the affinity endpoint fails.
  Evidence: `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`.
  Handoff: DONE. Provider-state-bound compact now fails closed when affinity is missing, while non-state-bound compact fallback remains intact.

## M3 - Provider-Opaque Runtime Signals

- [x] CSRC-040 [owner=codex] [deps=CSRC-030] [scope=crates/core/src/logging.rs,crates/core/src/proxy,crates/core/src/state.rs]
  Goal: Separate balance signals from runtime health signals in logs and route
  traces for compact 429 diagnosis.
  Validation: cargo nextest run -p codex-helper-core route_affinity request_logs --no-fail-fast
  Review: Logs must not infer sub2api/new-api internals; they should report only observed provider endpoint facts.
  Evidence: `cargo nextest run -p codex-helper-core route_affinity request_logs --no-fail-fast`; `cargo nextest run -p codex-helper-core control_trace route_graph_selection_explain --no-fail-fast`; `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`; `cargo fmt --all --check`.
  Handoff: DONE. Control trace now reports continuity class, affinity source, failover allowance, failover block reason, and explicitly non-authoritative balance signal status without relay-specific inference.

## M4 - Documentation And Closeout

- [x] CSRC-050 [owner=codex] [deps=CSRC-040] [scope=README.md,README_EN.md,docs/CONFIGURATION.md,docs/CONFIGURATION.zh.md,CHANGELOG.md,docs/workstreams/codex-session-route-continuity]
  Goal: Document restart-safe route continuity, provider-opaque assumptions, and
  operator-visible diagnostics.
  Validation: cargo fmt --all --check && cargo nextest run -p codex-helper-core
  Review: review-workstream before closeout.
  Evidence: `cargo fmt --all --check`; `cargo nextest run -p codex-helper-core`.
  Handoff: DONE. Public docs and changelog explain restart-safe route continuity, provider-opaque compact behavior, control-trace diagnostics, and residual continuity-domain follow-up.

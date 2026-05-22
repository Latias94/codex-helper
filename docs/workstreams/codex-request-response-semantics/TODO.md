# Codex Request Response Semantics - TODO

Status: Complete
Last updated: 2026-05-22

## M0 - Scope And Evidence Freeze

- [x] CRRS-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-request-response-semantics]
  Goal: Freeze problem, target state, non-goals, and evidence anchors for Codex request/response
  semantics work.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json exist and agree.
  Evidence: docs/workstreams/codex-request-response-semantics/DESIGN.md
  Handoff: Complete. First executable task is CRRS-020.

## M1 - P1 Continuity Recovery

- [x] CRRS-020 [owner=codex] [deps=CRRS-010] [scope=crates/core/src/proxy]
  Goal: Retry confirmed stale Codex `previous_response_id` failures once without that field.
  Validation: cargo nextest run -p codex-helper-core previous_response_id
  Review: Verify the retry only triggers on Codex Responses requests and explicit stale-response
  upstream errors.
  Evidence: focused proxy tests.
  Handoff: DONE. Stale `previous_response_id` errors now retry the same upstream once after
  removing the field, with route attempt evidence.

- [x] CRRS-030 [owner=codex] [deps=CRRS-010] [scope=crates/core/src/proxy]
  Goal: Complete missing Codex session identifiers from existing request evidence without
  overwriting client-provided ids.
  Validation: cargo nextest run -p codex-helper-core session_completion
  Review: Verify no synthetic session id is generated and non-Codex requests are untouched.
  Evidence: focused request preparation/proxy tests.
  Handoff: DONE. Missing Codex session fields are completed from existing request evidence without
  synthetic ids or overwriting client-provided fields.

## M2 - P2 Observability And Repair

- [x] CRRS-040 [owner=codex] [deps=CRRS-020,CRRS-030] [scope=crates/core/src/logging.rs,crates/core/src/proxy]
  Goal: Preserve requested/effective/actual `service_tier` attribution across non-streaming and
  streaming Codex responses.
  Validation: cargo nextest run -p codex-helper-core service_tier
  Review: Verify logging remains observational and does not patch request tier.
  Evidence: request ledger/logging tests.
  Handoff: DONE. Proxy-level tests cover requested/effective/actual service tier attribution and
  verify the upstream request tier is not rewritten.

- [x] CRRS-050 [owner=codex] [deps=CRRS-020] [scope=crates/core/src/proxy]
  Goal: Add bounded response repair for known relay encoding defects without broad payload
  rewriting.
  Validation: cargo nextest run -p codex-helper-core response_fixer
  Review: Verify repaired responses strip stale encoding/length headers and normal responses pass
  through unchanged.
  Evidence: focused proxy tests.
  Handoff: DONE. Bounded gzip JSON response repair is implemented for non-streaming Codex
  responses.

## M3 - Docs And Closeout

- [x] CRRS-060 [owner=codex] [deps=CRRS-020,CRRS-030,CRRS-040,CRRS-050] [scope=README.md,README_EN.md,CHANGELOG.md,docs/workstreams/codex-request-response-semantics]
  Goal: Document shipped behavior, validation evidence, and ChatGPT backend compatibility as a
  follow-on exploration.
  Validation: cargo fmt --package codex-helper-core && cargo nextest run -p codex-helper-core
  Review: Final review-workstream/verification before closeout.
  Evidence: EVIDENCE_AND_GATES.md and HANDOFF.md.
  Handoff: DONE. README, README_EN, CHANGELOG, and evidence docs reflect shipped behavior. ChatGPT
  backend compatibility remains a follow-on exploration.

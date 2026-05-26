# Codex Continuity Follow-Up Hardening

Status: Complete
Last updated: 2026-05-26

## Why This Lane Exists

`codex-continuity-decision-refactor` closed the core routing behavior: state-bound compact
requests now fail closed outside explicit continuity domains, and ordinary turns can escape an
unhealthy soft affinity. This follow-up lane handles the integration debt that would otherwise
turn that core fix into an operator footgun: release CI still tries to package the desktop app,
the largest regression file is hard to review, topology counting is duplicated, configuration
surfaces do not expose `continuity_domain` consistently, and diagnostics need to remain readable
for existing UI and CLI users.

## Target State

- cargo-dist release planning builds only the published CLI artifacts and does not require Tauri
  desktop sidecars.
- `response_semantics` regressions are split by behavioral concern without weakening coverage.
- Continuity topology counting lives behind one helper instead of duplicated ad hoc loops.
- Provider and endpoint editors in operator-facing surfaces can preserve and edit
  `continuity_domain`.
- Official OpenAI handling remains conservative: no host/domain heuristic relaxes compact
  continuity unless the implementation can prove a single official account identity.
- Capability diagnostics expose new continuity fields in a backwards-compatible shape for CLI,
  TUI, GUI, and admin clients.

## In Scope

- cargo-dist and release workflow configuration for the non-desktop release boundary.
- Rust topology helper extraction and targeted regression tests.
- Test module split for the large failover response semantics coverage.
- Admin DTO, TUI, and desktop UI plumbing for `continuity_domain`.
- CLI/TUI/GUI capability diagnostic formatting for `expected.continuity` and selected
  continuity-domain data.
- Documentation and tests that make the official OpenAI heuristic stance explicit.

## Out Of Scope

- Publishing the Tauri desktop app.
- Implementing credential fingerprinting or encrypted state decryption.
- Automatically grouping third-party relay endpoints by hostname, base URL, provider name, model
  list, or balance endpoint.
- Changing upstream Codex remote compaction protocol behavior.
- Rewriting route graph scoring beyond the helper extraction needed here.

## Architecture Direction

Treat continuity as an explicit topology property, not a provider branding property. Runtime code
should ask one helper for configured endpoint counts, selected domain counts, and same-domain
fallback eligibility. Operator surfaces should make the explicit domain visible and editable, but
they should avoid suggesting that matching domains, hosts, or official-looking URLs prove shared
encrypted response state.

For release CI, the package boundary should be expressed in project metadata rather than by
teaching CI to manufacture desktop sidecars. Desktop remains a local app build target and can keep
its own CI sidecar preparation, but release artifact planning must not select it.

## Risks

| Risk | Mitigation |
| --- | --- |
| cargo-dist config may drift from generated workflow contents. | Validate with `dist host --steps=create --tag=v0.17.0 --output-format=json` and update the generated workflow together with metadata. |
| Splitting tests may accidentally change module visibility or test filters. | Keep helpers in a parent test module and run targeted `response_semantics`/continuity filters before broad core nextest. |
| UI forms may drop unknown provider fields. | Add DTO/schema tests that preserve `continuity_domain` through edit flows. |
| Official OpenAI direct support may be over-generalized to relays. | Add explicit docs/tests that no base URL or host-only heuristic creates shared continuity domains. |

## Closeout Condition

This lane can close when all six requested follow-ups are implemented or explicitly deferred with
tests/docs, release planning no longer touches desktop, focused and broad validation gates pass, and
the workstream evidence documents the exact commands and outcomes.

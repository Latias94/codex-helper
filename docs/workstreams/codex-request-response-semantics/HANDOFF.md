# Codex Request Response Semantics - Handoff

Status: Complete
Last updated: 2026-05-22

Current state:

- CRRS-020 stale `previous_response_id` retry is implemented and tested.
- CRRS-030 Codex session completion is implemented and tested.
- CRRS-040 requested/effective/actual `service_tier` attribution is covered by proxy-level tests.
- CRRS-050 bounded gzip JSON response repair is implemented and tested.
- README, README_EN, CHANGELOG, and evidence docs are updated.

Follow-on:

- Explore direct ChatGPT backend upstream compatibility separately if the project wants
  `https://chatgpt.com/backend-api/codex` as a supported upstream target.

Residual risks:

- Session completion intentionally does not synthesize ids when the request has no existing session
  evidence.
- Response repair intentionally handles gzip JSON only; broader SSE/JSON repair should be a
  separate provider-scoped design if needed.

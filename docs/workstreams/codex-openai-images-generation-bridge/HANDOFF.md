# Codex OpenAI Images Generation Bridge — Handoff

Status: Complete
Last updated: 2026-05-22

Current state:

- `POST /v1/images/generations` and `/images/generations` are implemented as a Responses hosted
  `image_generation` adapter.
- Focused endpoint tests pass.
- `ch-imagegen` is installed at `C:/Users/Administrator/.codex/skills/ch-imagegen` and mirrored to
  `.agents/ch-imagegen` for repository distribution.
- README, README_EN, CHANGELOG, and configuration docs describe the endpoint.

Follow-ons:

- Add `/v1/images/edits` only after a multipart/edit contract is designed.
- Add multi-image fan-out only if there is a real use case for `n > 1`.
- Run full workspace nextest before a release cut.

Guardrails:

- Preserve existing provider routing/failover; do not create a separate provider executor unless the edge-adapter approach proves insufficient.
- Do not embed real provider secrets in the skill.
- Use `cargo nextest` for Rust validation and `cargo fmt` for formatting.

# Codex OpenAI Images Generation Bridge — Milestones

Status: Complete
Last updated: 2026-05-22

## M0 - Scope Freeze

Exit criteria:

- DESIGN.md explains why the adapter belongs at the proxy edge.
- TODO.md decomposes vertical, independently verifiable slices.
- WORKSTREAM.json points to authoritative docs.

Status: complete.

## M1 - Proxy Endpoint

Exit criteria:

- `POST /v1/images/generations` and `/images/generations` are routed before the wildcard proxy fallback.
- Request JSON is normalized to Responses + hosted `image_generation`.
- Successful Responses output is converted to Images-style `data[].b64_json`.
- Invalid shape and unsupported `n > 1` return deterministic 4xx errors.
- Existing provider routing/failover remains the only upstream execution path.

Status: complete.

## M2 - ch-imagegen Skill

Exit criteria:

- Skill folder exists under the Codex skills directory.
- `SKILL.md` frontmatter triggers for local codex-helper image generation tasks.
- Script supports prompt, size/aspect presets, output format/quality, dry-run, timeout, and new-file validation.
- No provider secret is hardcoded.

Status: complete.

## M3 - Verification And Closeout

Exit criteria:

- Formatting passes.
- Focused nextest gates pass.
- Skill validation and dry-run pass.
- README/configuration/changelog mention behavior and limits.
- HANDOFF.md and EVIDENCE_AND_GATES.md reflect final state.

Status: complete.

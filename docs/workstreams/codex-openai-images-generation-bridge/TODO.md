# Codex OpenAI Images Generation Bridge — TODO

Status: Complete
Last updated: 2026-05-22

## M0 - Scope Freeze

- [x] COIG-010 [owner=planner] [deps=none] [scope=docs/workstreams/codex-openai-images-generation-bridge]
  Goal: Freeze problem, target state, non-goals, and validation gates for an OpenAI-compatible images endpoint plus local skill.
  Validation: DESIGN.md, MILESTONES.md, EVIDENCE_AND_GATES.md, WORKSTREAM.json, and HANDOFF.md exist and agree.
  Handoff: Complete. First executable task is COIG-020.

## M1 - Proxy Images Endpoint

- [x] COIG-020 [owner=codex] [deps=COIG-010] [scope=crates/core/src/proxy]
  Goal: Add `POST /v1/images/generations` and `/images/generations` as a Responses-hosted image_generation adapter that reuses existing proxy routing/failover.
  Validation: `cargo nextest run -p codex-helper-core openai_images_generation`
  Review: Verify no bespoke provider routing is introduced and upstream auth remains owned by existing request preparation.
  Evidence: `crates/core/src/proxy/openai_images.rs`, `router_setup.rs`, focused nextest.
  Handoff: DONE. The router now handles both image generation paths before wildcard proxy fallback and internally reuses `handle_proxy` via a synthetic `/v1/responses` request.

- [x] COIG-030 [owner=codex] [deps=COIG-020] [scope=crates/core/src/proxy]
  Goal: Add focused tests for request normalization, response conversion, unsupported `n > 1`, and upstream error pass-through.
  Validation: `cargo nextest run -p codex-helper-core openai_images_generation`
  Review: Verify large image response handling is bounded and explicit.
  Evidence: `crates/core/src/proxy/tests/openai_images_generation.rs`
  Handoff: DONE. Tests cover request translation, Images response conversion, `n > 1` rejection, upstream error pass-through, and missing image result errors.

## M2 - Local Skill

- [x] COIG-040 [owner=codex] [deps=COIG-020] [scope=C:/Users/Administrator/.codex/skills/ch-imagegen,.agents/ch-imagegen]
  Goal: Create and install `ch-imagegen` with a deterministic script that calls the local `/v1/images/generations` endpoint and writes new image files.
  Validation: skill quick validation plus script dry-run.
  Review: Verify no real provider secret is embedded.
  Evidence: `C:/Users/Administrator/.codex/skills/ch-imagegen`, `.agents/ch-imagegen`, quick_validate, dry-run.
  Handoff: DONE. Skill is installed locally and mirrored into repo `.agents/ch-imagegen` for distribution.

## M3 - Docs And Verification

- [x] COIG-050 [owner=codex] [deps=COIG-020,COIG-040] [scope=README.md,README_EN.md,CHANGELOG.md,docs/CONFIGURATION*.md,docs/workstreams/codex-openai-images-generation-bridge]
  Goal: Document the endpoint, skill usage, limitations, evidence, and follow-ons.
  Validation: `cargo fmt --check` and focused nextest gates.
  Review: Final self-review before closeout or handoff.
  Evidence: README/README_EN/CHANGELOG/docs updates and `EVIDENCE_AND_GATES.md`.
  Handoff: DONE. Public docs describe the endpoint, limitations, and OpenAI Images response shape.

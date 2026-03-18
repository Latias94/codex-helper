# Fearless Refactor Workstream: Codex Control Plane

> 中文速览：本目录用于跟踪 `codex-helper` 从“本地代理 + 观察面板”演进到“Codex-first 本地控制平面”的无畏重构。重点不是照搬 `CLIProxyAPI`，而是先把会话身份、会话级控制、配置模板、提供商管理与高可用语义做清楚，再决定后续 GUI / WebUI / LAN 共享能力如何承接。

## Purpose

This workstream defines the target product shape, technical design, phased TODO list, and milestone gates for the Codex control-plane refactor.

The intended end state is:

- A **Codex-first local control plane** rather than a generic multi-ecosystem proxy platform.
- Explicit **session identity** and **effective route visibility**.
- Explicit **scope-aware overrides** (`session` first; persistent changes only when requested).
- Structured **station/provider management** with health, drain, circuit breaker, and failover.
- A LAN / Tailscale friendly topology where multiple devices can share the same central relay without pretending every device has access to every local transcript file.

## Document Map

- `DESIGN.md`
  - Product definition, object model, configuration semantics, API shape, HA and multi-device constraints.
- `VOCABULARY.md`
  - Canonical control-plane vocabulary, legacy-to-target mapping, and compatibility-only wording rules.
- `OPERATOR_SUMMARY_CONTRACT.md`
  - Read-side home-payload contract for `GET /__codex_helper/api/v1/operator/summary`, including layering and compatibility rules for future GUI/WebUI/external clients.
- `TODO.md`
  - Actionable engineering checklist, open questions, and work breakdown.
- `MILESTONES.md`
  - Milestone sequencing, definition-of-done gates, and the current `P0 / P1 / P2` closeout priorities.
- `CLOSEOUT.md`
  - Exit-gap assessment for the refactor, including what is already closed and what still blocks a full semantic closeout.
- `BACKEND_GAP_MATRIX.md`
  - Backend-focused readiness assessment for session/profile/station/provider/retry/LAN control-plane surfaces, plus the remaining productization gaps for future external clients.
- `CENTRAL_RELAY.md`
  - Product-shape notes for LAN / Tailscale shared-relay deployment and remote-safe capability boundaries.
- `PHASE1_IMPLEMENTATION.md`
  - Concrete implementation plan for `SLICE-001` to `SLICE-005`, including module touch points, API additions, compatibility strategy, and testing guidance.
- `CONFIG_V2_MIGRATION.md`
  - Operator-focused guide for moving legacy `config.toml` into the station/provider `v2` layout.

## Scope

In scope:

- Codex session identity and binding semantics.
- Session-scoped control for `model`, `service_tier`, and `reasoning_effort`.
- Structured station/profile configuration.
- Station health, breaker, drain, and failover semantics.
- LAN-shared central relay behavior.

Out of scope for the initial refactor:

- A full `CLIProxyAPI` clone.
- Large-scale OAuth / account platform features.
- Assuming remote devices can browse host-local `~/.codex/sessions` without an explicit companion or exported service.

## Guiding Principles

1. Prefer **explicit control semantics** over hidden magic.
2. Treat **session continuity** as a first-class constraint.
3. Keep the system **Codex-first** and only generalize when the abstraction remains honest.
4. Separate:
   - data plane routing
   - control plane decisions
   - local-only enrichment
5. Make every effective decision explainable:
   - what route was chosen
   - why it was chosen
   - what was overridden
   - where each value came from

## Update Rules

- Keep milestone status in `TODO.md` and `MILESTONES.md`.
- Record major shape changes in `DESIGN.md` before implementation drifts.
- Add new tracking docs in this folder only if they reduce ambiguity, not just to create paperwork.

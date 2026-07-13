# Runtime Boundary Refactor - Handoff

Status: Complete
Last updated: 2026-05-31

> Boundary update (2026-07-12): container/server runtimes remain unable to read or modify Codex-owned files. The old broad "client patching" concept was removed; the core switch implementation may be invoked only by the explicit local `switch on/off` command and may update only the helper provider selector/stanza in Codex `config.toml`.

## Current State

The runtime boundary refactor is complete. Docker build/smoke is repaired, runtime construction uses `ProxyRuntimeOptions`, host-local capability policy is runtime-local, server config resolves through `EffectiveServerConfig`, and local CLI server orchestration is split into smaller lifecycle helpers.

## Required Context

Read `DESIGN.md`, `TODO.md`, `EVIDENCE_AND_GATES.md`, `CONTEXT.jsonl`, ADR-0001, and the prior container deployment workstream before editing.

## Guardrails

- Preserve local `codex-helper serve` behavior in follow-on work.
- Do not move client patching into core or server crate.
- Keep Docker/Synology defaults conservative.

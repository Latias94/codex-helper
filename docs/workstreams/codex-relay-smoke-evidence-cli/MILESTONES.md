# Codex Relay Smoke Evidence CLI — Milestones

Status: Complete
Last updated: 2026-05-19

## M0 — Contract

Status: Complete

Exit criteria:

- Evidence store path and record semantics are documented.
- CLI surface is named.
- Non-goals exclude routing/health mutation and automatic patch switching.

Primary evidence:

- `DESIGN.md`
- `TODO.md`

## M1 — Evidence Store

Status: Complete

Exit criteria:

- Capability diagnostics and live smoke write sanitized JSONL evidence after successful response
  construction.
- Missing acknowledgement and preflight failures do not append evidence.
- Recent evidence can be read and filtered locally.

Primary gate:

```bash
cargo nextest run -p codex-helper-core codex_relay_evidence
```

## M2 — CLI

Status: Complete

Exit criteria:

- CLI can run capability diagnostics and live smoke without a running admin listener.
- CLI can list recent evidence records.
- Human and JSON output are available.

Primary gate:

```bash
cargo nextest run -p codex-helper codex_relay_cli
```

## M3 — Closeout

Status: Complete

Exit criteria:

- Docs and changelog explain command usage and safety.
- Targeted gates and formatting pass.
- Any wider package gates needed for public contracts pass or are explicitly deferred.

Closeout note:

- Evidence UI, WebSocket relay smoke, and remote compaction v2 smoke remain separate follow-on lanes.

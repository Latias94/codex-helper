# Codex Remote Compaction V2 Continuity - Evidence And Gates

Status: Complete
Last updated: 2026-05-26

## Gate Plan

Targeted gates:

- `cargo nextest run -p codex-helper-core remote_compaction_v2 request_logs --no-fail-fast`
- `cargo nextest run -p codex-helper-core remote_compaction_v2 route_affinity --no-fail-fast`
- `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast`

Final gates:

- `cargo fmt --all --check`
- `cargo nextest run -p codex-helper-core`

## Evidence Log

### 2026-05-26 - CRC2-040 documentation and closeout

- Updated README, English README, English/Chinese configuration docs, changelog,
  and Codex bridge diagnostics to describe v2 compact as `/responses` plus a
  structured `compaction_trigger` input item.
- Documented that helper recognizes and protects v2 compact continuity but does
  not enable Codex `remote_compaction_v2` by preset.
- Final gates:
  - `cargo fmt --all --check` passed.
  - `cargo nextest run -p codex-helper-core` passed.

### 2026-05-26 - CRC2-030 continuity policy

- Replaced provider execution's v1-only compact predicate with a common remote
  compaction predicate for v1 and v2.
- V2 compact now uses known route affinity when present and fails closed with
  the existing `state_bound_compact_missing_affinity` continuity error when
  affinity is missing.
- Added integration coverage for v2 compact staying on the sticky provider and
  for missing-affinity rejection without touching upstream providers.
- Gates:
  - `cargo nextest run -p codex-helper-core remote_compaction_v2 route_affinity --no-fail-fast` passed.
  - `cargo nextest run -p codex-helper-core responses_compact route_affinity --no-fail-fast` passed.

### 2026-05-26 - CRC2-020 classification and logging

- Added conservative request-body detection for structured
  `{"type":"compaction_trigger"}` items under `/responses` `input`.
- Added `RequestFlavor::is_remote_compaction_v2_request` and
  `CodexBridgeLog::remote_compaction_v2_request`.
- Failed request logs now preserve `codex_bridge` metadata, so fail-closed v2
  compact is diagnosable.
- Gate:
  - `cargo nextest run -p codex-helper-core remote_compaction_v2 request_logs --no-fail-fast` passed.

### 2026-05-26 - Workstream opened

- Created DESIGN.md, TODO.md, MILESTONES.md, EVIDENCE_AND_GATES.md,
  WORKSTREAM.json, HANDOFF.md, and JOURNAL.
- Initial evidence:
  - Upstream Codex v2 compact sends ordinary `POST /responses` with a
    structured `compaction_trigger` input item.
  - Local logs currently classify v2 compact only as `/responses`, so
    `~/.codex-helper/logs` cannot directly distinguish v2 compact from a
    normal user turn without external session evidence.

## Open Evidence

- None.

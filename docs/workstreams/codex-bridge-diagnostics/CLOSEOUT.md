# Closeout

Status: complete.

> Historical status (superseded 2026-07-12): bridge-mode and client-feature diagnostics were replaced by provider-owned capability decisions and bounded observations. Current `status`/`doctor` surfaces do not infer client patch presets or inspect Codex-owned auth, model cache, or SQLite files.

Implemented:

- Reusable Codex bridge diagnostics in core.
- `status` and `doctor` bridge output, including JSON status payloads.
- Remote compaction v2 warning when Codex has `[features].remote_compaction_v2 = true`.
- Request log/control-trace metadata for Codex bridge mode and `/responses/compact` requests.

Validated with targeted tests, `cargo fmt --check`, and full workspace nextest.

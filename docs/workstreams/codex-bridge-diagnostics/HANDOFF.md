# Handoff

Current task: none; workstream complete.

> Historical status (superseded 2026-07-12): bridge-mode diagnostics were replaced by provider-owned capability decisions and bounded observations. Current diagnostics do not infer client patch modes or recommend presets, and they never read or modify Codex auth, model cache, or SQLite files.

The intended implementation is offline and deterministic. Do not add live upstream probes in this lane; keep those as a follow-on after diagnostics are stable.

Follow-ons:

- Add a live relay capability probe for `/responses/compact`, websocket v2, and hosted image generation.
- Surface `codex_bridge` request log metadata in GUI/TUI request detail panes if operator demand justifies it.

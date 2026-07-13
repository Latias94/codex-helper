# Handoff

Status: complete

> Historical status (superseded 2026-07-12): `official-imagegen-bridge`, auth facades, and client patch presets were removed. Hosted image-generation support is now a provider/model capability decision; the explicit local Codex switch does not expose or enable that capability.

Current task: none.

Notes:

- Use existing `imagegen-bridge` auth patch helpers.
- Use existing `official-relay-bridge` provider fields.
- Keep WebSocket disabled.
- Keep relay auth stripping behavior.

Implementation is complete and verified by workspace tests.

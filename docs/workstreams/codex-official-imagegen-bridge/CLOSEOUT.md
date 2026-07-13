# Closeout

Status: complete

> Historical status (superseded 2026-07-12): this closeout records a removed client patch preset and auth-facade design. Hosted image generation is now a provider/model capability decision, and the explicit local switch does not expose or enable it.

## Delivered

- Added `official-imagegen-bridge` as an explicit Codex client patch mode.
- Combined `name = "OpenAI"` and `supports_websockets = false` with the existing empty `{}` imagegen
  auth facade.
- Preserved helper-side upstream credential ownership and Codex client auth stripping.
- Added CLI, config, TUI, GUI, README, configuration docs, changelog, and workstream documentation.

## Evidence

- `cargo fmt --check`
- Targeted `cargo nextest run -p codex-helper-core ...` for the new mode
- `cargo nextest run --workspace`

See `EVIDENCE_AND_GATES.md` for command details.

## Follow-Ons

- WebSocket support remains separate. Codex-helper still disables Codex Responses WebSocket transport
  for these bridge modes.
- Runtime diagnostics can be improved later to explicitly classify hosted image generation upstream
  failures from tool exposure failures.
- Relay capability probing can be added later for `/responses/compact` and hosted image generation.

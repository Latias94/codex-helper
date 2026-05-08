# Routing Strategy

> This note captures the real provider/station switching model in `codex-helper` and the next fearless-refactor targets.

## What Exists Today

- Pinned selection comes first.
  - session pin
  - global pin
  - session profile default
- If a pinned station is missing, the proxy falls back to the configured active station.
- A pinned station is blocked only by breaker-open runtime state.
- General routing builds a station plan from runtime state, enabled state, and upstream eligibility.
- Known fully exhausted balance snapshots are now a route-priority signal.
- Provider adapters may opt out of routing trust for exhausted snapshots when a source returns misleading zeros.
- Only stations with every known balance snapshot exhausted are demoted.
- Partial exhaustion, stale, error, and unknown balance states remain risk signals, not hard ordering inputs.
- Same-level stations are ordered by exhaustion rank, then active station, then name.
- Multi-level stations are ordered by exhaustion rank, then level, then active tiebreak, then name.
- Station-local load balancing still owns upstream choice inside the selected station.
- Cross-station failover before first output is still gated by the retry policy guardrail.

## What Is Wrong With The Current Vocabulary

- `retry.provider.strategy` is really a route boundary policy, not a provider-local retry detail.
- `level` behaves like a priority group, not a mere display level.
- The current routing modes are implementation-shaped.
- Balance is now a first-class exhaustion signal, but price is still not first-class in route choice.

## Fearless Refactor Target

- Make route selection a first-class core model.
- Keep request orchestration thin.
- Keep balance, price, health, and capability as explicit inputs to route scoring.
- Keep session continuity the default.
- Keep after-first-output cross-station failover opt-in.

## Priority Order

1. Finish the explicit routing plan cleanup and trace labels.
2. Promote richer balance and price into route eligibility / ranking inputs.
3. Rename route-policy vocabulary to match actual semantics.
4. Add policy presets for sticky, preferred-failover, monthly-primary, balanced, and fast-first.

## Non-Goals

- Do not connect GUI/TUI directly to route logic.
- Do not split provider config into per-provider Codex clones.
- Do not make balance refresh timing a UI concern.

# Codex Relay Capability Profile - Milestones

Status: Complete
Last updated: 2026-05-19

## M0 - Scope And Evidence Freeze

Exit criteria:

- Problem and target state are explicit.
- Non-goals are explicit.
- Existing bridge workstreams are linked instead of reopened.
- First proof target is chosen.

Primary evidence:

- `docs/workstreams/codex-relay-capability-profile/DESIGN.md`
- `docs/workstreams/codex-relay-capability-profile/TODO.md`

Status: Complete.

## M1 - Static Capability Profile

Exit criteria:

- Patch mode, auth shape, provider identity, WebSocket state, and model metadata are represented in
  one core profile.
- Tests prove the known Codex gates for remote compaction v1 and hosted image generation.
- The profile can report uncertainty rather than overclaiming support.

Primary gates:

```bash
cargo nextest run -p codex-helper-core codex_capability_profile
```

Status: Complete.

## M2 - Relay Probe Evidence

Exit criteria:

- `/models`, `/responses`, and `/responses/compact` can be classified safely.
- Probe results are bounded, opt-in, and do not cause per-request retry storms.
- Recent request-ledger evidence can be distinguished from fresh active probes.

Primary gates:

```bash
cargo nextest run -p codex-helper-core codex_relay_probe
cargo nextest run -p codex-helper-core codex_capabilities_api
```

Status: Complete.

## M3 - Recommendations And Docs

Exit criteria:

- Operator-facing output recommends a patch mode for common capability combinations.
- Recommendations include reasons, confidence, and limitations.
- English and Chinese configuration docs explain sub2api and non-sub2api behavior.

Primary gates:

```bash
cargo nextest run -p codex-helper-core codex_patch_mode_recommendation
cargo fmt --check
```

Status: Complete.

## M4 - Closeout

Exit criteria:

- Gate set is recorded with fresh evidence.
- Remaining work is completed, deferred, or split into follow-ons.
- `WORKSTREAM.json` status is updated.
- Optional `CLOSEOUT.md` captures delivered behavior and residual risk.

Status: Complete. Final core package gate passed with 528 tests.

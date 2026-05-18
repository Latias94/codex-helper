# Milestones: Codex TUI Startup Guardrail

## Milestone Strategy

Work should proceed in this order:

1. define the startup readiness conditions and message contract;
2. wire the probe into the interactive TUI entry path;
3. make the alert visible but dismissible;
4. prove the quiet path stays quiet when no warning is needed;
5. finish with manual smoke coverage for direct startup.

## P0 - Readiness Contract

Goal:

- Agree on what should trigger a startup alert and what should stay silent.

Scope:

- direct-start entry points;
- Codex switch diagnostics;
- remote-control status and log confirmation;
- alert copy and next-action wording.

Acceptance:

- The startup probe is defined in terms of existing core state.
- The team can say exactly when the alert appears.
- The alert copy points at a concrete follow-up command or action.

Suggested verification:

- `cargo fmt --all --check`
- targeted core tests for the probe, once added

## P1 - Interactive Startup Alert

Goal:

- Show the startup guardrail in the TUI when the user is likely to skip manual verification.

Scope:

- modal/banner/toast presentation;
- dismissal behavior;
- interactive `serve` wiring;
- quiet noninteractive behavior.

Acceptance:

- Direct TUI startup can show a warning without blocking the whole app.
- Dismissal returns the user to normal work.
- Noninteractive startup remains silent.

Suggested verification:

- `cargo nextest run -p codex-helper-tui`
- manual interactive startup smoke test

## P2 - Polish And Closeout

Goal:

- Remove ambiguity and confirm the UX is actually useful.

Scope:

- narrow width behavior;
- alert copy polish;
- ready/warn/no-warning tests;
- smoke notes.

Acceptance:

- The alert is readable under narrow widths.
- The dismiss path is obvious.
- The startup guardrail does not become noisy in the common case.


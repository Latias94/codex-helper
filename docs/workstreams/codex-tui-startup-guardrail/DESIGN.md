# Design: Codex TUI Startup Guardrail

## Problem Statement

Many operators start the Codex TUI directly and do not run a separate verification command first.
When the local proxy patch, remote-control state, or Codex login state is stale, the mismatch is
only discovered after the session has already started.

The product gap is not another capability matrix. The gap is a visible, low-friction startup
guardrail that answers:

- Is this Codex session in a state that should be reviewed now?
- If so, what is the next action?
- Can the user keep working without being forced through a wizard?

## Design Goals

- Surface startup readiness in the interactive TUI flow.
- Reuse existing core diagnostics instead of duplicating config parsing in the UI.
- Make the prompt actionable, concise, and easy to dismiss.
- Keep the behavior quiet for noninteractive paths.
- Prefer a small, focused alert over a long onboarding flow.

## Non-goals

- Building the broader capability matrix.
- Adding a response fixer or protocol normalizer.
- Designing a GUI wizard.
- Changing provider routing semantics.

## Current Inputs

- `src/cli_app.rs`
  - the interactive `serve` path starts the TUI
  - `switch remote-control` already prints restart guidance
- `crates/core/src/codex_integration.rs`
  - `guard_codex_config_before_switch_on_interactive`
  - `codex_switch_status`
  - `codex_remote_control_status`
  - `codex_remote_control_successful_enablement_log_seen`
- `crates/tui/src/tui/state.rs`
  - `toast` and `overlay` already exist
- `crates/tui/src/tui/view/modals.rs`
  - modal infrastructure already exists for other startup-like warnings

## Target Behavior

- On interactive TUI startup, run a cheap readiness probe.
- If the probe finds a stale or incomplete helper state, show a one-time startup alert.
- The alert should prefer a modal or prominent banner, with a clear dismiss path.
- If the issue is minor or the terminal is narrow, fall back to a toast.
- Noninteractive CLI paths should remain quiet.

## Probe Candidates

- `~/.codex/config.toml` still reflects an old local proxy patch.
- Remote-control enablement is incomplete or not yet confirmed.
- The logs have not yet confirmed `experimentalFeature/enablement/set` success after a remote-control change.
- The user started the TUI directly and skipped the normal verification flow.

## UX Guidance

- Prefer an explanation plus next step over a generic warning.
- Offer exact follow-up commands in text form.
- Keep the alert dismissible without forcing a workflow change.
- Do not turn startup readiness into a full wizard.

## Relationship to Other Workstreams

- `codex-operator-experience-refactor` keeps the broader operator console and capability-gap story.
- `codex-tui-operator-polish` keeps layout, viewport, and footer stability.
- `codex-provider-concurrency-limits` already covers provider hard-stop semantics.

This workstream stays narrow: one startup guardrail for the direct TUI entry path.


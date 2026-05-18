# Manual Smoke: Codex TUI Startup Guardrail

Run this in a real interactive terminal. The built-in TUI does not start when
stdin/stdout are not TTYs.

## Ready State

1. Ensure `~/.codex/config.toml` points at the helper through a valid switch
   state, or start from a normal provider config.
2. Run `ch` or `codex-helper serve`.
3. Confirm the TUI opens normally.
4. Confirm no startup guardrail modal appears when there is no warning state.

## Client Config Changed

1. Start from a Codex config that is not already patched to the local helper.
2. Run `ch` or `codex-helper serve`.
3. Confirm the TUI opens with a `Startup guardrail` modal.
4. Confirm the modal explains that Codex client config changed on startup.
5. Press `Enter` or `Esc`.
6. Confirm the modal closes and normal TUI navigation works.

## Remote-Control Follow-Up

1. Enable remote control with `codex-helper switch remote-control enable`.
2. Start the TUI before Codex App has produced a successful
   `experimentalFeature/enablement/set` log.
3. Confirm the modal points to restarting Codex App and running
   `codex-helper switch remote-control check-logs`.
4. Press `Enter` or `Esc`.
5. Confirm the modal does not return during the same TUI session.

## Narrow Terminal

1. Resize the terminal to about 64 columns wide.
2. Trigger a startup guardrail warning.
3. Confirm the modal still shows the title, the core warning, and the
   `Esc/Enter` close hint.


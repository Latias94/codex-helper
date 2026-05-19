# Codex Official Bridge Diagnostics

## Problem

Official bridge modes now deliberately shape Codex client state so local relay users can reach first-party-like behavior through codex-helper:

- `official-relay-bridge`: OpenAI provider identity for remote compaction v1.
- `official-imagegen-bridge`: OpenAI provider identity plus `{}` auth facade for hosted image generation.

When image generation or remote compaction does not appear, the operator currently has to inspect `~/.codex/config.toml`, `~/.codex/auth.json`, helper config, request logs, and Codex feature flags manually.

## Target State

`codex-helper doctor` and `codex-helper switch status` should explain the bridge chain in plain terms:

- active patch mode,
- provider identity needed for remote compaction v1,
- websocket setting,
- auth facade state needed for imagegen bridge,
- helper upstream credential availability after client auth stripping,
- `remote_compaction_v2` warning when enabled.

Proxy request logs should also mark compact requests and bridge mode so request-ledger and control traces can answer whether a compaction request actually traversed helper.

## Scope

- Offline diagnostics only. No live OpenAI/sub2api network probe in this lane.
- Reuse existing switch status and doctor flows.
- Add compact/bridge metadata to request logs with tests.

## Non-Goals

- Enabling remote compaction v2 by default.
- Implementing websocket v2 compatibility probing.
- Changing routing behavior.

# Codex Official Imagegen Bridge

## Problem

`official-relay-bridge` lets Codex see the local proxy as the official OpenAI Responses provider,
which enables Codex remote compaction v1 through `/responses/compact`. It intentionally does not
touch `auth.json`, so it does not expose Codex hosted `image_generation` when the user has no real
ChatGPT login.

`imagegen-bridge` writes an empty `{}` auth facade. Current Codex auth loading resolves an auth file
with no `auth_mode` and no `OPENAI_API_KEY` as ChatGPT auth, so `AuthManager::current_auth_uses_codex_backend`
returns true and Codex includes the hosted image generation tool. That mode keeps provider
`name = "codex-helper"`, so Codex does not choose official remote compaction.

Users want the combined behavior for relay accounts backed by official subscriptions:

- model traffic goes through codex-helper relay routing;
- Codex chooses official remote compaction v1;
- Codex exposes hosted `image_generation`;
- Codex client auth is not forwarded to third-party relays.

## Source Findings

- Codex remote compaction v1 is selected through
  `core/src/compact.rs::should_use_remote_compact_task`, which delegates to
  `ModelProviderInfo::supports_remote_compaction`.
- `model-provider-info/src/lib.rs::supports_remote_compaction` returns true for provider
  `name == "OpenAI"` or Azure Responses providers.
- Codex hosted image generation is gated in `tools/src/tool_config.rs` by:
  `image_generation_tool_auth_allowed && Feature::ImageGeneration && supports_image_generation(model_info)`.
- `core/src/session/turn_context.rs::image_generation_tool_auth_allowed` is true only when the
  `AuthManager` reports Codex-backend auth.
- `login/src/auth/manager.rs::current_auth_uses_codex_backend` is true for ChatGPT, ChatGPT auth
  tokens, or agent identity auth.
- `login/src/auth/manager.rs::AuthDotJson::resolved_mode` defaults to ChatGPT when `auth_mode` and
  `OPENAI_API_KEY` are both absent, which explains why the empty `{}` facade works.
- These conditions are independent: provider `name = "OpenAI"` and empty-auth ChatGPT facade can be
  combined.

## Target State

Add an explicit `official-imagegen-bridge` patch mode:

- writes `model_providers.codex_proxy.name = "OpenAI"`;
- writes `wire_api = "responses"`;
- writes `supports_websockets = false`;
- does not write `requires_openai_auth`;
- writes the same empty `{}` auth facade as `imagegen-bridge`;
- records/restores the auth facade through the existing switch state;
- strips Codex client auth at relay forwarding time unless an upstream secret is configured.

## Non-Goals

- Do not enable Codex Responses WebSocket transport in codex-helper in this lane.
- Do not change Codex upstream source code.
- Do not auto-detect relay compact/imagegen support before switching.
- Do not claim image generation succeeds for relays that reject hosted image generation calls.

## Risks

- Codex may later change `AuthDotJson::resolved_mode` and remove the empty-object ChatGPT fallback.
  The mode remains experimental and should be documented as relying on current Codex behavior.
- Some relays support `/responses/compact` but reject `image_generation` hosted tools. Operators
  need request logs to distinguish tool exposure from upstream support.
- If a user edits `auth.json` after enabling the mode, helper must preserve the safety rule and not
  overwrite user changes during restore.

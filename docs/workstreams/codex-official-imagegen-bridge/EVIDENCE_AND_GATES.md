# Evidence And Gates

## Commands

```text
cargo fmt --check
```

Result: passed.

```text
cargo nextest run -p codex-helper-core codex_switch_on_official_imagegen_bridge_sets_openai_name_and_disables_websockets empty_auth_json_facade_detection_uses_json_semantics official_imagegen_bridge_ready_check_rejects_unresolved_upstream_env codex_switch_on_official_imagegen_bridge_records_mode_and_patches_auth_json codex_switch_status_infers_official_imagegen_bridge_from_empty_auth_facade_without_state codex_switch_default_restores_official_imagegen_bridge_auth_json codex_switch_official_imagegen_to_chatgpt_bridge_uses_original_auth_json codex_client_patch_mode_parses_official_imagegen_bridge prepare_attempt_request_strips_client_auth_in_official_imagegen_bridge_without_upstream_secret
```

Result: passed, 9 tests run.

```text
cargo nextest run --workspace
```

Result: passed, 754 tests run.

## Claims

- `official-imagegen-bridge` combines the existing official relay and imagegen auth facade behavior.
- Existing modes retain their prior behavior.
- CLI/TUI/GUI/config parsing compile together with the new mode.

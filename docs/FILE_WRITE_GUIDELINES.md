# Local File Persistence Guidelines

This note documents the repository convention for writing local config/state files safely.

## Why

On Windows, a naive `tmp + rename` flow can fail when the destination file already exists.
For config/state files, this can break repeated saves, backup refresh, or restore flows.

## Rules

1. Do not open-code `tmp + rename` for files that may overwrite an existing path.
2. Reuse `crates/core/src/file_replace.rs`:
   - `write_text_file()` for sync text writes
   - `write_bytes_file_async()` for async byte writes
3. Keep the temp file in the same directory as the destination.
4. If the feature keeps a `.bak` snapshot, copy the old file before replacing it.
5. Prefer one shared helper over per-module file-write logic.

## Scope

These rules apply to local files whose path stays stable across writes, for example:

- `~/.codex-helper/config.toml`
- `~/.codex-helper/config.json`
- `notify_state.json`
- session/stat cache files
- Codex / Claude client config files patched by `switch on/off`

## Non-goals

This guideline does not target log rotation paths that rename to a new timestamped filename.
Those flows do not replace an existing destination file.

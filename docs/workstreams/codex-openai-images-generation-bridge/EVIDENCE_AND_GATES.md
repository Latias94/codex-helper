# Codex OpenAI Images Generation Bridge — Evidence And Gates

Status: Complete
Last updated: 2026-05-22

## Required Gates

- `cargo fmt --check`
- `cargo nextest run -p codex-helper-core openai_images_generation`
- `python C:/Users/Administrator/.codex/skills/.system/skill-creator/scripts/quick_validate.py C:/Users/Administrator/.codex/skills/ch-imagegen`
- `python C:/Users/Administrator/.codex/skills/ch-imagegen/scripts/generate_image.py --prompt "dry run cat" --dry-run`

## Evidence Log

```text
cargo fmt --package codex-helper-core
```

Result: passed.

```text
cargo nextest run -p codex-helper-core openai_images_generation
```

Result: passed, 7 tests run.

```text
python C:/Users/Administrator/.codex/skills/.system/skill-creator/scripts/quick_validate.py C:/Users/Administrator/.codex/skills/ch-imagegen
python C:/Users/Administrator/.codex/skills/ch-imagegen/scripts/generate_image.py --prompt "dry run cat" --dry-run
```

Result: passed. The dry-run computed `3840x2160` and printed the request payload without calling the upstream.

```text
python C:/Users/Administrator/.codex/skills/.system/skill-creator/scripts/quick_validate.py .agents/ch-imagegen
python .agents/ch-imagegen/scripts/generate_image.py --prompt "dry run cat" --dry-run
```

Result: passed. The repository-distributed skill mirror validates and dry-runs.

## Claims To Prove

- Images-style requests are translated into hosted Responses image-generation requests.
- The normal provider chain still owns routing, retry, failover, model mapping, auth injection, and logging.
- Successful Responses image-generation calls become `data[].b64_json` responses for skill callers.
- The skill writes only the newly generated image and does not infer success from old files.

## Notes

- Full workspace nextest remains advisable before release, but focused gates cover the new endpoint
  and skill behavior.
- One unrelated pre-existing working-tree change remains in
  `docs/workstreams/tauri-desktop-replacement-parity/scripts/tdrp_080_packaged_smoke.ps1`; it was
  not modified by this lane.

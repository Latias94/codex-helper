---
name: ch-imagegen
description: Generate or reference-edit raster images through a running local codex-helper proxy using its OpenAI-compatible `/v1/images/generations` and JSON `/v1/images/edits` bridges. Use when Codex should create bitmap images via the user's local relay/provider chain, when the built-in imagegen tool is unstable with the relay, or when the user asks for `ch-imagegen`, codex-helper image generation, OpenAI Images API generation, reference image generation, gpt-image-2 images, 2K/4K image outputs, or local proxy image generation.
---

# CH Imagegen

Use this skill for local codex-helper image generation. It calls the proxy's
OpenAI-compatible `/v1/images/generations` endpoint, or `/v1/images/edits` when
reference images are passed with `--image`, saves the returned base64 image, and
validates only the newly written file.

## Rules

- Do not use the system `.system/imagegen` workflow for requests that explicitly ask for
  `ch-imagegen` or local codex-helper image generation.
- Require a running codex-helper proxy that exposes `/v1/images/generations`; reference-image
  mode additionally requires `/v1/images/edits`.
- Do not ask the user to paste provider API keys. Upstream credentials belong in codex-helper
  config or environment variables.
- Treat `scripts/generate_image.py` exit code and stdout JSON as the source of truth.
- If the script exits non-zero, report the error and stop. Do not infer success from older files.
- Default model: `gpt-image-2`.
- Default resolution: `4k`; default aspect ratio: `16:9`; default output format: `png`;
  default quality: `high`.
- Save final outputs under `output/imagegen/` unless the user specifies another directory.

## Command

```bash
python "${CODEX_HOME:-$HOME/.codex}/skills/ch-imagegen/scripts/generate_image.py" \
  --prompt "<user prompt>" \
  --aspect "16:9" \
  --resolution "4k"
```

Reference image mode:

```bash
python "${CODEX_HOME:-$HOME/.codex}/skills/ch-imagegen/scripts/generate_image.py" \
  --prompt "<user prompt>" \
  --image "/path/to/reference.png" \
  --aspect "3:4" \
  --resolution "4k"
```

Useful overrides:

- `--base-url "http://127.0.0.1:3211/v1/images/generations"`
- `--edits-base-url "http://127.0.0.1:3211/v1/images/edits"`
- `--image "reference.png"`; may be repeated; accepts local image paths, `data:image/...`,
  HTTP(S) URLs, and `file_id` values
- `--input-fidelity "high"` for reference-image edits
- `--aspect "4:3"` or `--aspect "9:16"`
- `--resolution "2k"`
- `--size "3840x2160"`
- `--quality "medium"`
- `--output-format "webp"`
- `--title "short-slug"`
- `--out-dir "output/imagegen"`
- `--dry-run`

## Size behavior

- `4k` and `2k` are pixel-budget presets. The script computes a valid model size from the
  requested aspect ratio.
- Explicit sizes are clamped to `gpt-image-2` limits: max edge 3840, total pixels no more than
  8,294,400, long-to-short ratio no more than 3:1, and 16-pixel alignment.
- Use explicit `3840x2160` for 4K landscape or `2160x3840` for 4K portrait.

## Validation

After generation, report:

- endpoint used;
- requested size and actual local image size;
- reference image count when present;
- output path;
- revised prompt if present.

Never scan old output files to guess that generation succeeded.

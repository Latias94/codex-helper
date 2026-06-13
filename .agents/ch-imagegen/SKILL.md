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
- Default Image API model intent: `gpt-image-2`.
- Default Responses wrapper model: `gpt-5.5`, sent as the helper-only `responses_model`
  JSON field so the local bridge uses `/v1/responses` hosted `image_generation` through the
  same kind of mainline model path as Codex's built-in `imagegen` tool.
- Default resolution: `2k`; default aspect ratio: `16:9`; default output format: `png`;
  default quality: `high`.
- Use `4k` only when the user explicitly needs final-resolution output. Set a tool/shell timeout
  longer than the script timeout plus retry buffer; do not use a 120s shell timeout for 4K images.
- Treat stdout JSON as authoritative for both success and failure. On failure the script prints
  `ok:false` with `error.status`, `error.classification`, `error.request_id`,
  `error.failure_hint`, `error.retryable`, `error.attempts`, and `error.suggested_action`.
- If `error.classification` is `image_generation_route_failed` and `error.failure_hint` is
  `all_upstreams_failed` or `route_unavailable`, report that the configured codex-helper route
  did not have a currently usable image-capable upstream for the requested model. Do not present
  that as a prompt, reference image, or resolution problem unless the error says so explicitly.
- Do not automatically fall back to the system `.system/imagegen` workflow after local proxy
  failure. It may use a different account/path; ask or use it only when the user explicitly
  approves that bypass.
- Save final outputs under `output/imagegen/` unless the user specifies another directory.

## Command

```bash
python "${CODEX_HOME:-$HOME/.codex}/skills/ch-imagegen/scripts/generate_image.py" \
  --prompt "<user prompt>" \
  --aspect "16:9" \
  --resolution "2k"
```

Reference image mode:

```bash
python "${CODEX_HOME:-$HOME/.codex}/skills/ch-imagegen/scripts/generate_image.py" \
  --prompt "<user prompt>" \
  --image "/path/to/reference.png" \
  --aspect "3:4" \
  --resolution "2k"
```

Useful overrides:

- `--base-url "http://127.0.0.1:3211/v1/images/generations"`
- `--edits-base-url "http://127.0.0.1:3211/v1/images/edits"`
- `--responses-model "gpt-5.5"` to choose the hosted `image_generation` wrapper model used
  by codex-helper's `/v1/responses` bridge
- `--image "reference.png"`; may be repeated; accepts local image paths, `data:image/...`,
  HTTP(S) URLs, and `file_id` values
- `--input-fidelity "high"` for reference-image edits
- `--aspect "4:3"` or `--aspect "9:16"`
- `--resolution "4k"` for explicit final output
- `--resolution "2k"`
- `--fallback-resolution "2k"` when trying 4K but willing to retry smaller after route/provider
  failures
- `--size "3840x2160"`
- `--quality "medium"`
- `--retries 2`
- `--retry-delay 30`
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
- Image API model intent and Responses wrapper model;
- requested size and actual local image size;
- reference image count when present;
- output path;
- revised prompt if present.

If generation fails, report the structured `error` block from stdout, especially
`classification`, `failure_hint`, `request_id`, `retryable`, and `suggested_action`.

Never scan old output files to guess that generation succeeded.

---
name: ch-imagegen
description: "Generate or edit raster images through a running local codex-helper or OpenAI-compatible relay, including text-to-image, image-to-image, multi-image edits, masks/inpainting, streaming image responses, batch generation, 2K/4K gpt-image-2 sizing, and sub2api/NewAPI-compatible JSON or multipart OpenAI Images API requests. Use when the user asks for ch-imagegen, codex-helper image generation, local proxy image generation, OpenAI Images API generation or edits, gpt-image-2 images, image-to-image, or generated bitmap assets through the user's relay/provider chain."
---

# CH Imagegen

Use this skill for local image generation and image editing through a running
codex-helper proxy or another OpenAI-compatible relay. It calls `/v1/images/generations`
or `/v1/images/edits`, saves returned image bytes, and treats the script exit code plus
stdout JSON as the source of truth.

## Rules

- Do not use the system `.system/imagegen` workflow when the user explicitly asks for
  `ch-imagegen`, codex-helper image generation, or local proxy image generation.
- Require a running proxy or relay that exposes OpenAI-compatible Images API endpoints.
- Do not ask the user to paste provider API keys. Upstream credentials belong in
  codex-helper config, the relay, or local environment variables.
- Default model: `gpt-image-2`.
- Default resolution: `4k`; default aspect ratio: `16:9`; default output format: `png`;
  default quality: `high`.
- Save final outputs under `output/imagegen/` unless the user specifies another directory.
- Never scan old output files to infer success. Validate only the files written by the
  current script invocation.

## Quick Commands

Text-to-image:

```bash
python .agents/skills/ch-imagegen/scripts/image_gen.py generate \
  --prompt "<user prompt>" \
  --aspect "16:9" \
  --resolution "4k"
```

Image-to-image or edit with one or more local input images:

```bash
python .agents/skills/ch-imagegen/scripts/image_gen.py edit \
  --prompt "<edit instruction>" \
  --image "path/to/input.png" \
  --quality high \
  --output-format png
```

Masked edit/inpainting:

```bash
python .agents/skills/ch-imagegen/scripts/image_gen.py edit \
  --prompt "<edit instruction>" \
  --image "path/to/input.png" \
  --mask "path/to/mask.png"
```

Batch generation from JSONL:

```bash
python .agents/skills/ch-imagegen/scripts/image_gen.py generate-batch \
  --input tmp/imagegen/jobs.jsonl \
  --out-dir output/imagegen \
  --concurrency 3
```

The legacy text-to-image wrapper is also available:

```bash
python .agents/skills/ch-imagegen/scripts/generate_image.py --prompt "<user prompt>"
```

## Endpoint And Auth

Useful overrides:

- `--base-url "http://127.0.0.1:3211/v1"` for a root `/v1` URL.
- `--base-url "http://127.0.0.1:3211/v1/images/generations"` for an endpoint URL.
- `--api-key "$CH_IMAGEGEN_API_KEY"` for relays that require bearer auth.
- Environment variables: `CH_IMAGEGEN_BASE_URL`, `CH_IMAGEGEN_API_KEY`.
- `--header "X-Provider: value"` for relay-specific public headers. Do not print secrets.

`--base-url` may be either a `/v1` root, a `/v1/images/generations` endpoint, or a
`/v1/images/edits` endpoint. The script derives the matching endpoint for the selected
command.

## Request Modes

Generation always uses JSON.

Edits support:

- `--request-mode auto` (default): use multipart for local file inputs, JSON for URL/data URL/file-id references.
- `--request-mode multipart`: send OpenAI-compatible multipart fields `model`, `prompt`,
  `image`, optional `mask`, and native image options. This is the broadest choice for
  local files and is compatible with sub2api/NewAPI-style gateways.
- `--request-mode json`: encode local files as data URLs under `images[].image_url`;
  also supports `images[].file_id` via `--image file_id:<id>`.

Prefer multipart for local image-to-image and masked edits. Use JSON mode when the
input image is already a URL/data URL or when codex-helper should convert JSON image
references into Responses API inputs.

## Size Behavior

- `4k` and `2k` are pixel-budget presets. The script computes a model-valid size from
  `--aspect`.
- Explicit `--size WIDTHxHEIGHT` values are clamped to `gpt-image-2` constraints:
  max edge 3840, total pixels no more than 8,294,400, long-to-short ratio no more than
  3:1, and 16-pixel alignment.
- Use `--size auto` to pass provider/model auto sizing through unchanged.
- Use explicit `--size 3840x2160` for 4K landscape or `--size 2160x3840` for 4K portrait.

## Output And Validation

After each run, report from stdout JSON:

- endpoint used;
- command and request mode;
- requested size and detected local image size;
- output path(s);
- revised prompt if present.

If the script exits non-zero, report the error and stop. Do not infer success from older
files in `output/imagegen/`.

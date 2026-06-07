#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from mimetypes import guess_type
from typing import Any

DEFAULT_GENERATIONS_URL = "http://127.0.0.1:3211/v1/images/generations"
DEFAULT_MODEL = "gpt-image-2"
DEFAULT_RESOLUTION = "4k"
DEFAULT_ASPECT = "16:9"
DEFAULT_OUTPUT_FORMAT = "png"
DEFAULT_QUALITY = "high"
DEFAULT_TIMEOUT = 900
DEFAULT_PROGRESS_INTERVAL = 15

MIN_PIXELS = 655_360
MAX_EDGE = 3840
MAX_PIXELS = 8_294_400
MAX_ASPECT = 3.0
ALIGN = 16
PIXEL_BUDGETS = {
    "4k": 8_294_400,
    "2k": 3_686_400,
}
STOPWORDS = {
    "a",
    "an",
    "and",
    "at",
    "for",
    "from",
    "in",
    "into",
    "of",
    "on",
    "one",
    "realistic",
    "scene",
    "the",
    "to",
    "with",
}


def _die(message: str, code: int = 1) -> None:
    print(f"Error: {message}", file=sys.stderr)
    raise SystemExit(code)


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


def _floor_aligned(value: float, align: int = ALIGN) -> int:
    ivalue = int(math.floor(value))
    return max(align, ivalue - (ivalue % align))


def _parse_aspect(text: str) -> tuple[int, int]:
    match = re.fullmatch(r"\s*(\d+)\s*[:/xX]\s*(\d+)\s*", text)
    if not match:
        _die(f"invalid aspect ratio: {text}")
    width = int(match.group(1))
    height = int(match.group(2))
    if width < 1 or height < 1:
        _die(f"invalid aspect ratio: {text}")
    ratio = max(width / height, height / width)
    if ratio > MAX_ASPECT:
        _die(f"aspect ratio exceeds {MAX_ASPECT}:1 limit: {text}")
    return width, height


def _parse_size(text: str) -> tuple[int, int]:
    match = re.fullmatch(r"\s*(\d+)\s*x\s*(\d+)\s*", text.lower())
    if not match:
        _die(f"invalid size: {text}")
    width = int(match.group(1))
    height = int(match.group(2))
    if width < 1 or height < 1:
        _die(f"invalid size: {text}")
    return width, height


def _clamp_size(width: int, height: int) -> tuple[int, int]:
    ratio = max(width / height, height / width)
    if ratio > MAX_ASPECT:
        _die(f"requested size exceeds {MAX_ASPECT}:1 aspect limit: {width}x{height}")

    scale = min(1.0, MAX_EDGE / max(width, height), math.sqrt(MAX_PIXELS / (width * height)))
    width = _floor_aligned(width * scale)
    height = _floor_aligned(height * scale)
    width = min(width, MAX_EDGE - (MAX_EDGE % ALIGN))
    height = min(height, MAX_EDGE - (MAX_EDGE % ALIGN))

    while width * height > MAX_PIXELS:
        width = max(ALIGN, width - ALIGN)
        height = max(ALIGN, height - ALIGN)

    if width * height < MIN_PIXELS:
        _die(f"computed size is below gpt-image-2 minimum pixel count: {width}x{height}")

    return width, height


def _compute_budgeted_size(aspect: tuple[int, int], resolution: str) -> tuple[int, int]:
    if resolution not in PIXEL_BUDGETS:
        _die(f"unsupported resolution preset: {resolution}")
    budget = PIXEL_BUDGETS[resolution]
    aw, ah = aspect
    raw_width = math.sqrt(budget * aw / ah)
    raw_height = math.sqrt(budget * ah / aw)
    return _clamp_size(_floor_aligned(raw_width), _floor_aligned(raw_height))


def _slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    slug = re.sub(r"-{2,}", "-", slug)
    return slug[:64] if slug else ""


def _derive_title(prompt: str, explicit_title: str | None) -> str:
    if explicit_title:
        title = _slugify(explicit_title)
        if title:
            return title

    tokens = [token for token in re.findall(r"[a-zA-Z0-9]+", prompt.lower()) if token not in STOPWORDS]
    if tokens:
        return _slugify("-".join(tokens[:6]))

    digest = hashlib.sha1(prompt.encode("utf-8")).hexdigest()[:8]
    return f"image-{digest}"


def _timestamp_ms() -> str:
    now = datetime.now(timezone.utc)
    return f"{now.strftime('%Y%m%d-%H%M%S')}-{now.microsecond // 1000:03d}"


def _png_size(data: bytes) -> tuple[int, int]:
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        _die("generated file is not a PNG")
    if len(data) < 24:
        _die("generated PNG is truncated")
    width, height = struct.unpack(">II", data[16:24])
    return width, height


def _image_size(path: Path, data: bytes, output_format: str) -> tuple[int | None, int | None]:
    fmt = output_format.lower()
    if fmt == "png":
        return _png_size(data)
    try:
        from PIL import Image
    except Exception:
        return None, None
    with Image.open(path) as image:
        return image.size


def _reference_endpoint(base_url: str, edits_base_url: str | None, has_images: bool) -> str:
    if not has_images:
        return base_url
    if edits_base_url:
        return edits_base_url
    if base_url.rstrip("/").endswith("/images/generations"):
        return re.sub(r"/images/generations/?$", "/images/edits", base_url.rstrip("/"))
    return base_url


def _image_reference(value: str) -> dict[str, str]:
    value = value.strip()
    if not value:
        _die("empty --image value")
    if value.startswith(("data:image/", "http://", "https://")):
        return {"image_url": value}
    if value.startswith(("file-", "file_")):
        return {"file_id": value}

    path = Path(value).expanduser().resolve()
    if not path.is_file():
        _die(f"reference image not found: {path}")
    mime_type = guess_type(path.name)[0] or "application/octet-stream"
    if not mime_type.startswith("image/"):
        _die(f"reference file is not an image type: {path}")
    image_b64 = base64.b64encode(path.read_bytes()).decode("ascii")
    return {"image_url": f"data:{mime_type};base64,{image_b64}"}


def _redacted_payload(payload: dict[str, Any]) -> dict[str, Any]:
    redacted = json.loads(json.dumps(payload))
    images = redacted.get("images")
    if isinstance(images, list):
        for image in images:
            if not isinstance(image, dict):
                continue
            image_url = image.get("image_url")
            if isinstance(image_url, str) and image_url.startswith("data:image/"):
                prefix = image_url.split(",", 1)[0]
                image["image_url"] = f"{prefix},<redacted>"
    return redacted


def _request_json(url: str, api_key: str | None, payload: dict[str, Any], timeout: int) -> dict[str, Any]:
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json",
    }
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        _die(f"HTTP {exc.code}: {body}")
    except urllib.error.URLError as exc:
        _die(f"request failed: {exc}")
    return {}


class _Heartbeat:
    def __init__(self, interval: int, label: str) -> None:
        self.interval = interval
        self.label = label
        self._start = time.monotonic()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=0.2)

    def _run(self) -> None:
        while not self._stop.wait(self.interval):
            elapsed = int(time.monotonic() - self._start)
            _log(f"[ch-imagegen] still waiting for image response ... elapsed={elapsed}s")


def _extract_image_result(data: dict[str, Any]) -> tuple[str, str | None]:
    items = data.get("data")
    if not isinstance(items, list) or not items:
        _die("response contained no data array")
    first = items[0]
    if not isinstance(first, dict):
        _die("response data[0] is not an object")
    image_b64 = first.get("b64_json")
    if not isinstance(image_b64, str) or not image_b64.strip():
        _die("response data[0] contained no b64_json")
    revised_prompt = first.get("revised_prompt")
    return image_b64, revised_prompt if isinstance(revised_prompt, str) else None


def _build_payload(args: argparse.Namespace, size: str) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "model": args.model,
        "prompt": args.prompt,
        "size": size,
        "quality": args.quality,
        "output_format": args.output_format,
    }
    if args.background:
        payload["background"] = args.background
    if args.moderation:
        payload["moderation"] = args.moderation
    if args.input_fidelity:
        payload["input_fidelity"] = args.input_fidelity
    if args.image:
        payload["images"] = [_image_reference(image) for image in args.image]
    return payload


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate images through local codex-helper")
    parser.add_argument("--prompt", required=True)
    parser.add_argument("--title")
    parser.add_argument("--aspect", default=DEFAULT_ASPECT)
    parser.add_argument("--resolution", default=DEFAULT_RESOLUTION, choices=sorted(PIXEL_BUDGETS))
    parser.add_argument("--size")
    parser.add_argument("--out-dir", default="output/imagegen")
    parser.add_argument("--base-url", default=os.getenv("CH_IMAGEGEN_BASE_URL", DEFAULT_GENERATIONS_URL))
    parser.add_argument("--edits-base-url", default=os.getenv("CH_IMAGEGEN_EDITS_BASE_URL"))
    parser.add_argument("--api-key", default=os.getenv("CH_IMAGEGEN_API_KEY"))
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--quality", default=DEFAULT_QUALITY)
    parser.add_argument("--background")
    parser.add_argument("--moderation")
    parser.add_argument("--input-fidelity", choices=("low", "high"))
    parser.add_argument("--image", action="append", help="Reference image path, data URL, HTTP URL, or file ID; may be repeated")
    parser.add_argument("--output-format", default=DEFAULT_OUTPUT_FORMAT)
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    parser.add_argument("--progress-interval", type=int, default=DEFAULT_PROGRESS_INTERVAL)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if args.size:
        requested_width, requested_height = _parse_size(args.size)
        computed_width, computed_height = _clamp_size(requested_width, requested_height)
    else:
        computed_width, computed_height = _compute_budgeted_size(
            _parse_aspect(args.aspect),
            args.resolution,
        )

    requested_size = f"{computed_width}x{computed_height}"
    title = _derive_title(args.prompt, args.title)
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    timestamp = _timestamp_ms()
    dry_path = out_dir / f"{title}-{requested_size}-{timestamp}.{args.output_format}"
    payload = _build_payload(args, requested_size)
    endpoint = _reference_endpoint(args.base_url, args.edits_base_url, bool(args.image))
    mode = "edits" if args.image else "generations"

    if args.dry_run:
        print(
            json.dumps(
                {
                    "ok": True,
                    "mode": "dry-run",
                    "request_mode": mode,
                    "base_url": endpoint,
                    "model": args.model,
                    "size": requested_size,
                    "reference_count": len(args.image or []),
                    "title": title,
                    "output": str(dry_path),
                    "payload": _redacted_payload(payload),
                },
                ensure_ascii=False,
                indent=2,
            )
        )
        return 0

    _log(
        f"[ch-imagegen] starting {mode} request model={args.model} size={requested_size} "
        f"references={len(args.image or [])} "
        f"timeout={args.timeout}s output={dry_path}"
    )
    _log(f"[ch-imagegen] endpoint={endpoint}")
    heartbeat = _Heartbeat(args.progress_interval, "image response")
    heartbeat.start()
    try:
        response = _request_json(endpoint, args.api_key, payload, args.timeout)
    finally:
        heartbeat.stop()
    _log("[ch-imagegen] received API response")

    image_b64, revised_prompt = _extract_image_result(response)
    image_bytes = base64.b64decode(image_b64)
    suffix = "." + args.output_format.lower().lstrip(".")
    final_path = out_dir / f"{title}-{requested_size}-{timestamp}{suffix}"

    with tempfile.NamedTemporaryFile(dir=out_dir, prefix=".ch-imagegen-", suffix=".tmp", delete=False) as tmp:
        tmp.write(image_bytes)
        tmp_path = Path(tmp.name)
    tmp_path.replace(final_path)

    actual_width, actual_height = _image_size(final_path, image_bytes, args.output_format)
    actual_size = (
        f"{actual_width}x{actual_height}"
        if actual_width is not None and actual_height is not None
        else None
    )
    if actual_size:
        renamed_path = out_dir / f"{title}-{actual_size}-{timestamp}{suffix}"
        if renamed_path != final_path:
            final_path.replace(renamed_path)
            final_path = renamed_path
    _log(f"[ch-imagegen] wrote image {final_path}")

    print(
        json.dumps(
            {
                "ok": True,
                "request_mode": mode,
                "base_url": endpoint,
                "model": args.model,
                "requested_size": requested_size,
                "actual_size": actual_size,
                "reference_count": len(args.image or []),
                "title": title,
                "output": str(final_path),
                "revised_prompt": revised_prompt,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

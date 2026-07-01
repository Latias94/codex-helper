#!/usr/bin/env python3
"""Local OpenAI Images API CLI for codex-helper-compatible relays."""

from __future__ import annotations

import argparse
import base64
import concurrent.futures
from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import math
import mimetypes
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import threading
import time
from typing import Any
import urllib.error
import urllib.parse
import urllib.request


DEFAULT_BASE_URL = "http://127.0.0.1:3211/v1"
DEFAULT_MODEL = "gpt-image-2"
DEFAULT_RESOLUTION = "4k"
DEFAULT_ASPECT = "16:9"
DEFAULT_OUTPUT_FORMAT = "png"
DEFAULT_QUALITY = "high"
DEFAULT_TIMEOUT = 900
DEFAULT_PROGRESS_INTERVAL = 15
DEFAULT_OUT_DIR = "output/imagegen"

MIN_PIXELS = 655_360
MAX_EDGE = 3840
MAX_PIXELS = 8_294_400
MAX_ASPECT = 3.0
ALIGN = 16
MAX_IMAGE_BYTES = 50 * 1024 * 1024
PIXEL_BUDGETS = {
    "4k": 8_294_400,
    "2k": 3_686_400,
}
COMMANDS = {"generate", "edit", "generate-batch"}
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


@dataclass
class ImageInput:
    raw: str
    kind: str
    path: Path | None = None
    value: str | None = None


@dataclass
class MultipartFile:
    field: str
    path: Path
    filename: str
    content_type: str
    data: bytes


@dataclass
class ImageResult:
    data: bytes
    revised_prompt: str | None = None
    output_format: str | None = None
    source: str = "b64_json"
    partial: bool = False


def _die(message: str, code: int = 1) -> None:
    print(f"Error: {message}", file=sys.stderr)
    raise SystemExit(code)


def _warn(message: str) -> None:
    print(f"Warning: {message}", file=sys.stderr)


def _log(message: str) -> None:
    print(message, file=sys.stderr, flush=True)


class Heartbeat:
    def __init__(self, interval: int, label: str) -> None:
        self.interval = interval
        self.label = label
        self._start = time.monotonic()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        if self.interval > 0:
            self._thread.start()

    def stop(self) -> None:
        if self.interval > 0:
            self._stop.set()
            self._thread.join(timeout=0.2)

    def _run(self) -> None:
        while not self._stop.wait(self.interval):
            elapsed = int(time.monotonic() - self._start)
            _log(f"[ch-imagegen] still waiting for {self.label} ... elapsed={elapsed}s")


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


def _resolve_size(args: argparse.Namespace) -> str:
    if args.size:
        if args.size.strip().lower() == "auto":
            return "auto"
        requested_width, requested_height = _parse_size(args.size)
        width, height = _clamp_size(requested_width, requested_height)
        return f"{width}x{height}"
    width, height = _compute_budgeted_size(_parse_aspect(args.aspect), args.resolution)
    return f"{width}x{height}"


def _normalize_output_format(value: str | None) -> str | None:
    if value is None:
        return None
    fmt = value.strip().lower()
    if not fmt:
        return None
    if fmt == "jpg":
        return "jpeg"
    if fmt not in {"png", "jpeg", "webp"}:
        _die("--output-format must be png, jpeg, jpg, or webp")
    return fmt


def _slugify(text: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    slug = re.sub(r"-{2,}", "-", slug)
    return slug[:64] if slug else ""


def _derive_title(prompt: str, explicit_title: str | None) -> str:
    if explicit_title:
        title = _slugify(explicit_title)
        if title:
            return title

    tokens = [
        token
        for token in re.findall(r"[a-zA-Z0-9]+", prompt.lower())
        if token not in STOPWORDS
    ]
    if tokens:
        return _slugify("-".join(tokens[:6]))

    digest = hashlib.sha1(prompt.encode("utf-8")).hexdigest()[:8]
    return f"image-{digest}"


def _timestamp_ms() -> str:
    now = datetime.now(timezone.utc)
    return f"{now.strftime('%Y%m%d-%H%M%S')}-{now.microsecond // 1000:03d}"


def _read_prompt(prompt: str | None, prompt_file: str | None) -> str:
    if prompt and prompt_file:
        _die("Use --prompt or --prompt-file, not both.")
    if prompt_file:
        path = Path(prompt_file)
        if not path.exists():
            _die(f"prompt file not found: {path}")
        value = path.read_text(encoding="utf-8").strip()
    elif prompt:
        value = prompt.strip()
    else:
        _die("missing prompt. Use --prompt or --prompt-file.")
    if not value:
        _die("prompt is empty")
    return value


def _endpoint_url(base_url: str, kind: str) -> str:
    if kind not in {"generations", "edits"}:
        _die(f"invalid endpoint kind: {kind}")
    raw = (base_url or DEFAULT_BASE_URL).strip()
    parsed = urllib.parse.urlsplit(raw)
    if not parsed.scheme or not parsed.netloc:
        _die(f"invalid base URL: {raw}")

    path = parsed.path.rstrip("/")
    if re.search(r"/images/(generations|edits)$", path):
        path = re.sub(r"/images/(generations|edits)$", f"/images/{kind}", path)
    elif path.endswith("/images"):
        path = f"{path}/{kind}"
    elif path.endswith("/v1"):
        path = f"{path}/images/{kind}"
    elif path:
        path = f"{path}/v1/images/{kind}"
    else:
        path = f"/v1/images/{kind}"

    return urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, path, "", ""))


def _parse_header(raw: str) -> tuple[str, str]:
    if ":" not in raw:
        _die(f"invalid header {raw!r}; expected 'Name: value'")
    name, value = raw.split(":", 1)
    name = name.strip()
    if not name:
        _die(f"invalid header {raw!r}; header name is empty")
    return name, value.strip()


def _request_headers(args: argparse.Namespace, content_type: str) -> dict[str, str]:
    headers = {
        "Content-Type": content_type,
        "Accept": "application/json, text/event-stream",
        "User-Agent": "ch-imagegen/1.0",
    }
    api_key = getattr(args, "api_key", None)
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"
    for raw in getattr(args, "header", None) or []:
        name, value = _parse_header(raw)
        headers[name] = value
    return headers


def _redacted_header_names(headers: dict[str, str]) -> list[str]:
    return sorted(headers.keys())


def _post_bytes(
    url: str,
    args: argparse.Namespace,
    body: bytes,
    content_type: str,
    label: str,
) -> tuple[dict[str, str], bytes]:
    headers = _request_headers(args, content_type)
    request = urllib.request.Request(url, data=body, headers=headers, method="POST")
    heartbeat = Heartbeat(args.progress_interval, label)
    heartbeat.start()
    try:
        with urllib.request.urlopen(request, timeout=args.timeout) as response:
            response_headers = {k.lower(): v for k, v in response.headers.items()}
            return response_headers, response.read()
    except urllib.error.HTTPError as exc:
        error_body = exc.read().decode("utf-8", errors="replace")
        _die(f"HTTP {exc.code}: {error_body}")
    except urllib.error.URLError as exc:
        _die(f"request failed: {exc}")
    finally:
        heartbeat.stop()
    return {}, b""


def _post_json(url: str, args: argparse.Namespace, payload: dict[str, Any], label: str) -> Any:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    headers, raw = _post_bytes(url, args, body, "application/json", label)
    return _parse_response_body(headers, raw, args)


def _post_multipart(
    url: str,
    args: argparse.Namespace,
    fields: dict[str, Any],
    files: list[MultipartFile],
    label: str,
) -> Any:
    boundary = f"ch-imagegen-{hashlib.sha1(os.urandom(16)).hexdigest()}"
    chunks: list[bytes] = []

    for name, value in fields.items():
        if value is None:
            continue
        chunks.append(f"--{boundary}\r\n".encode("utf-8"))
        chunks.append(f'Content-Disposition: form-data; name="{name}"\r\n\r\n'.encode("utf-8"))
        chunks.append(str(value).encode("utf-8"))
        chunks.append(b"\r\n")

    for item in files:
        chunks.append(f"--{boundary}\r\n".encode("utf-8"))
        disposition = (
            f'Content-Disposition: form-data; name="{item.field}"; '
            f'filename="{item.filename}"\r\n'
        )
        chunks.append(disposition.encode("utf-8"))
        chunks.append(f"Content-Type: {item.content_type}\r\n\r\n".encode("utf-8"))
        chunks.append(item.data)
        chunks.append(b"\r\n")

    chunks.append(f"--{boundary}--\r\n".encode("utf-8"))
    body = b"".join(chunks)
    content_type = f"multipart/form-data; boundary={boundary}"
    headers, raw = _post_bytes(url, args, body, content_type, label)
    return _parse_response_body(headers, raw, args)


def _parse_response_body(headers: dict[str, str], raw: bytes, args: argparse.Namespace) -> Any:
    content_type = headers.get("content-type", "").lower()
    text = raw.decode("utf-8", errors="replace")
    if "text/event-stream" in content_type or _looks_like_sse(text):
        return _parse_sse(text, save_partials=args.save_partials)
    try:
        return json.loads(text)
    except json.JSONDecodeError as exc:
        _die(f"response was not JSON or SSE: {exc}: {text[:500]}")


def _looks_like_sse(text: str) -> bool:
    return any(line.startswith("data:") for line in text.splitlines())


def _parse_sse(text: str, *, save_partials: bool) -> list[ImageResult]:
    results: list[ImageResult] = []
    data_lines: list[str] = []

    def flush() -> None:
        if not data_lines:
            return
        data = "\n".join(data_lines).strip()
        data_lines.clear()
        if not data or data == "[DONE]":
            return
        try:
            event = json.loads(data)
        except json.JSONDecodeError:
            return
        results.extend(_extract_results_from_json(event, save_partials=save_partials))

    for raw_line in text.splitlines():
        line = raw_line.rstrip("\r")
        if line == "":
            flush()
            continue
        if line.startswith("data:"):
            data_lines.append(line[5:].lstrip())
    flush()

    if not results:
        _die("SSE response contained no final image result")
    return results


def _extract_results(response: Any, *, save_partials: bool) -> list[ImageResult]:
    if isinstance(response, list) and all(isinstance(item, ImageResult) for item in response):
        return response
    return _extract_results_from_json(response, save_partials=save_partials)


def _extract_results_from_json(value: Any, *, save_partials: bool) -> list[ImageResult]:
    if not isinstance(value, dict):
        return []

    results: list[ImageResult] = []

    data = value.get("data")
    if isinstance(data, list):
        for item in data:
            result = _image_result_from_mapping(item, partial=False)
            if result is not None:
                results.append(result)

    output = value.get("output")
    if isinstance(output, list):
        for item in output:
            if isinstance(item, dict) and item.get("type") == "image_generation_call":
                result = _image_result_from_mapping(item, partial=False)
                if result is not None:
                    results.append(result)

    if value.get("type") in {"image_generation.completed", "image_generation.partial_image"}:
        partial = value.get("type") == "image_generation.partial_image"
        if not partial or save_partials:
            result = _image_result_from_mapping(value, partial=partial)
            if result is not None:
                results.append(result)

    if value.get("type") == "response.image_generation_call.partial_image" and save_partials:
        result = _image_result_from_mapping(value, partial=True)
        if result is not None:
            results.append(result)

    if value.get("type") == "response.output_item.done":
        item = value.get("item")
        if isinstance(item, dict):
            result = _image_result_from_mapping(item, partial=False)
            if result is not None:
                results.append(result)

    response = value.get("response")
    if isinstance(response, dict):
        results.extend(_extract_results_from_json(response, save_partials=save_partials))

    if not results:
        result = _image_result_from_mapping(value, partial=False)
        if result is not None:
            results.append(result)

    return results


def _image_result_from_mapping(item: Any, *, partial: bool) -> ImageResult | None:
    if not isinstance(item, dict):
        return None

    b64_value = (
        item.get("b64_json")
        or item.get("result")
        or item.get("partial_image_b64")
        or item.get("image_b64")
    )
    if isinstance(b64_value, str) and b64_value.strip():
        return ImageResult(
            data=base64.b64decode(b64_value),
            revised_prompt=_string_or_none(item.get("revised_prompt")),
            output_format=_string_or_none(item.get("output_format")),
            source="b64_json",
            partial=partial,
        )

    url = item.get("url")
    if isinstance(url, str) and url.strip():
        data, output_format = _bytes_from_url(url.strip())
        return ImageResult(
            data=data,
            revised_prompt=_string_or_none(item.get("revised_prompt")),
            output_format=_string_or_none(item.get("output_format")) or output_format,
            source="url",
            partial=partial,
        )
    return None


def _string_or_none(value: Any) -> str | None:
    if isinstance(value, str) and value.strip():
        return value
    return None


def _bytes_from_url(url: str) -> tuple[bytes, str | None]:
    if url.startswith("data:"):
        header, _, encoded = url.partition(",")
        if not encoded:
            _die("data URL response was empty")
        is_base64 = ";base64" in header.lower()
        media_type = header[5:].split(";", 1)[0] if header.startswith("data:") else ""
        if not is_base64:
            data = urllib.parse.unquote_to_bytes(encoded)
        else:
            data = base64.b64decode(encoded)
        return data, _format_from_media_type(media_type)

    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme not in {"http", "https"}:
        _die(f"unsupported image URL scheme in response: {parsed.scheme}")
    with urllib.request.urlopen(url, timeout=120) as response:
        content_type = response.headers.get("content-type", "")
        return response.read(), _format_from_media_type(content_type.split(";", 1)[0])


def _format_from_media_type(media_type: str) -> str | None:
    media_type = media_type.strip().lower()
    if media_type == "image/png":
        return "png"
    if media_type in {"image/jpeg", "image/jpg"}:
        return "jpeg"
    if media_type == "image/webp":
        return "webp"
    if media_type == "image/gif":
        return "gif"
    return None


def _common_payload(args: argparse.Namespace, prompt: str, size: str) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "model": args.model,
        "prompt": prompt,
        "n": args.n,
        "size": size,
        "quality": args.quality,
        "output_format": _normalize_output_format(args.output_format),
    }
    optional_fields = [
        "background",
        "moderation",
        "response_format",
        "style",
        "user",
        "responses_model",
    ]
    for name in optional_fields:
        value = getattr(args, name, None)
        if value is not None:
            payload[name] = value
    if getattr(args, "stream", False):
        payload["stream"] = True
    if getattr(args, "output_compression", None) is not None:
        payload["output_compression"] = args.output_compression
    if getattr(args, "partial_images", None) is not None:
        payload["partial_images"] = args.partial_images
    if getattr(args, "input_fidelity", None) is not None:
        payload["input_fidelity"] = args.input_fidelity
    return {k: v for k, v in payload.items() if v is not None}


def _parse_image_input(raw: str) -> ImageInput:
    raw = raw.strip()
    if not raw:
        _die("image input is empty")
    if raw.startswith("file_id:"):
        value = raw.split(":", 1)[1].strip()
        if not value:
            _die("file_id image reference is empty")
        return ImageInput(raw=raw, kind="file_id", value=value)
    if raw.startswith("data:"):
        return ImageInput(raw=raw, kind="image_url", value=raw)
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme in {"http", "https"}:
        return ImageInput(raw=raw, kind="image_url", value=raw)

    path = Path(raw)
    if not path.exists():
        _die(f"image file not found: {path}")
    if not path.is_file():
        _die(f"image path is not a file: {path}")
    if path.stat().st_size > MAX_IMAGE_BYTES:
        _warn(f"image exceeds 50MB OpenAI Images API limit: {path}")
    return ImageInput(raw=raw, kind="path", path=path)


def _input_to_json_ref(image: ImageInput) -> dict[str, str]:
    if image.kind == "path" and image.path is not None:
        return {"image_url": _data_url_for_path(image.path)}
    if image.kind == "image_url" and image.value:
        return {"image_url": image.value}
    if image.kind == "file_id" and image.value:
        return {"file_id": image.value}
    _die(f"unsupported image input: {image.raw}")
    return {}


def _data_url_for_path(path: Path) -> str:
    data = path.read_bytes()
    content_type = _content_type_for_path(path, data)
    encoded = base64.b64encode(data).decode("ascii")
    return f"data:{content_type};base64,{encoded}"


def _content_type_for_path(path: Path, data: bytes) -> str:
    guessed = mimetypes.guess_type(path.name)[0]
    if guessed and guessed.startswith("image/"):
        return guessed
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "image/png"
    if data.startswith(b"\xff\xd8\xff"):
        return "image/jpeg"
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return "image/webp"
    if data.startswith(b"GIF87a") or data.startswith(b"GIF89a"):
        return "image/gif"
    return "application/octet-stream"


def _multipart_files(
    images: list[ImageInput],
    mask: ImageInput | None,
    image_field: str,
) -> list[MultipartFile]:
    files: list[MultipartFile] = []
    for idx, image in enumerate(images):
        if image.kind != "path" or image.path is None:
            _die(
                "multipart edit mode requires local file paths. Use --request-mode json for URL/data URL/file-id inputs."
            )
        data = image.path.read_bytes()
        field = image_field
        if image_field == "indexed":
            field = f"image[{idx}]"
        files.append(
            MultipartFile(
                field=field,
                path=image.path,
                filename=image.path.name,
                content_type=_content_type_for_path(image.path, data),
                data=data,
            )
        )

    if mask is not None:
        if mask.kind != "path" or mask.path is None:
            _die("multipart mask mode requires a local mask file path")
        data = mask.path.read_bytes()
        files.append(
            MultipartFile(
                field="mask",
                path=mask.path,
                filename=mask.path.name,
                content_type=_content_type_for_path(mask.path, data),
                data=data,
            )
        )
    return files


def _choose_edit_mode(args: argparse.Namespace, images: list[ImageInput], mask: ImageInput | None) -> str:
    if args.request_mode != "auto":
        return args.request_mode
    if all(image.kind == "path" for image in images) and (mask is None or mask.kind == "path"):
        return "multipart"
    return "json"


def _output_suffix(result: ImageResult, requested_format: str | None) -> str:
    fmt = (result.output_format or _sniff_format(result.data) or requested_format or "png").lower()
    if fmt == "jpg":
        fmt = "jpeg"
    if fmt == "jpeg":
        return ".jpg"
    if fmt in {"png", "webp", "gif"}:
        return f".{fmt}"
    return ".img"


def _sniff_format(data: bytes) -> str | None:
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if data.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return "webp"
    if data.startswith(b"GIF87a") or data.startswith(b"GIF89a"):
        return "gif"
    return None


def _image_dimensions(data: bytes) -> tuple[int | None, int | None]:
    if data.startswith(b"\x89PNG\r\n\x1a\n") and len(data) >= 24:
        return struct.unpack(">II", data[16:24])
    if data.startswith(b"\xff\xd8\xff"):
        return _jpeg_dimensions(data)
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return _webp_dimensions(data)
    if (data.startswith(b"GIF87a") or data.startswith(b"GIF89a")) and len(data) >= 10:
        return struct.unpack("<HH", data[6:10])
    return None, None


def _jpeg_dimensions(data: bytes) -> tuple[int | None, int | None]:
    idx = 2
    while idx + 9 < len(data):
        if data[idx] != 0xFF:
            idx += 1
            continue
        marker = data[idx + 1]
        idx += 2
        if marker in {0xD8, 0xD9}:
            continue
        if idx + 2 > len(data):
            break
        length = struct.unpack(">H", data[idx : idx + 2])[0]
        if length < 2 or idx + length > len(data):
            break
        if marker in {
            0xC0,
            0xC1,
            0xC2,
            0xC3,
            0xC5,
            0xC6,
            0xC7,
            0xC9,
            0xCA,
            0xCB,
            0xCD,
            0xCE,
            0xCF,
        }:
            height = struct.unpack(">H", data[idx + 3 : idx + 5])[0]
            width = struct.unpack(">H", data[idx + 5 : idx + 7])[0]
            return width, height
        idx += length
    return None, None


def _webp_dimensions(data: bytes) -> tuple[int | None, int | None]:
    if len(data) < 30:
        return None, None
    chunk_type = data[12:16]
    if chunk_type == b"VP8X" and len(data) >= 30:
        width = int.from_bytes(data[24:27], "little") + 1
        height = int.from_bytes(data[27:30], "little") + 1
        return width, height
    if chunk_type == b"VP8L" and len(data) >= 25:
        bits = int.from_bytes(data[21:25], "little")
        width = (bits & 0x3FFF) + 1
        height = ((bits >> 14) & 0x3FFF) + 1
        return width, height
    if chunk_type == b"VP8 " and len(data) >= 30:
        start = 20
        width = struct.unpack("<H", data[start + 6 : start + 8])[0] & 0x3FFF
        height = struct.unpack("<H", data[start + 8 : start + 10])[0] & 0x3FFF
        return width, height
    return None, None


def _write_results(
    results: list[ImageResult],
    *,
    prompt: str,
    title: str | None,
    requested_size: str,
    out_dir: str,
    out: str | None,
    output_format: str | None,
    force: bool,
) -> list[dict[str, Any]]:
    if not results:
        _die("response contained no image result")

    out_base = Path(out_dir).resolve()
    out_base.mkdir(parents=True, exist_ok=True)
    resolved_title = _derive_title(prompt, title)
    timestamp = _timestamp_ms()
    written: list[dict[str, Any]] = []

    for idx, result in enumerate(results, start=1):
        suffix = _output_suffix(result, output_format)
        if out:
            output_path = Path(out).resolve()
            if len(results) > 1:
                output_path = output_path.with_name(f"{output_path.stem}-{idx}{output_path.suffix or suffix}")
            elif output_path.suffix == "":
                output_path = output_path.with_suffix(suffix)
        else:
            partial = "-partial" if result.partial else ""
            ordinal = f"-{idx}" if len(results) > 1 else ""
            output_path = out_base / f"{resolved_title}-{requested_size}{partial}-{timestamp}{ordinal}{suffix}"

        if output_path.exists() and not force:
            _die(f"output already exists: {output_path} (use --force to overwrite)")
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            dir=output_path.parent,
            prefix=".ch-imagegen-",
            suffix=".tmp",
            delete=False,
        ) as tmp:
            tmp.write(result.data)
            tmp_path = Path(tmp.name)
        tmp_path.replace(output_path)

        width, height = _image_dimensions(result.data)
        actual_size = f"{width}x{height}" if width is not None and height is not None else None
        written.append(
            {
                "path": str(output_path),
                "actual_size": actual_size,
                "source": result.source,
                "partial": result.partial,
                "revised_prompt": result.revised_prompt,
            }
        )
    return written


def _run_generate(args: argparse.Namespace) -> dict[str, Any]:
    prompt = _read_prompt(args.prompt, args.prompt_file)
    requested_size = _resolve_size(args)
    endpoint = _endpoint_url(args.base_url, "generations")
    payload = _common_payload(args, prompt, requested_size)

    if args.dry_run:
        output_preview = _output_preview(prompt, args.title, requested_size, args.out_dir, args.out, args.output_format)
        return {
            "ok": True,
            "mode": "dry-run",
            "command": "generate",
            "endpoint": endpoint,
            "headers": _redacted_header_names(_request_headers(args, "application/json")),
            "requested_size": requested_size,
            "output_preview": output_preview,
            "payload": payload,
        }

    _log(f"[ch-imagegen] generation endpoint={endpoint}")
    _log(
        f"[ch-imagegen] starting generation model={args.model} size={requested_size} timeout={args.timeout}s"
    )
    response = _post_json(endpoint, args, payload, "image generation response")
    results = _extract_results(response, save_partials=args.save_partials)
    outputs = _write_results(
        results,
        prompt=prompt,
        title=args.title,
        requested_size=requested_size,
        out_dir=args.out_dir,
        out=args.out,
        output_format=args.output_format,
        force=args.force,
    )
    return {
        "ok": True,
        "command": "generate",
        "endpoint": endpoint,
        "request_mode": "json",
        "model": args.model,
        "requested_size": requested_size,
        "outputs": outputs,
    }


def _run_edit(args: argparse.Namespace) -> dict[str, Any]:
    prompt = _read_prompt(args.prompt, args.prompt_file)
    requested_size = _resolve_size(args)
    endpoint = _endpoint_url(args.base_url, "edits")
    images = [_parse_image_input(raw) for raw in args.image]
    if not images:
        _die("at least one --image is required")
    if len(images) > 16:
        _die("OpenAI Images edits supports at most 16 input images")
    mask = _parse_image_input(args.mask) if args.mask else None
    request_mode = _choose_edit_mode(args, images, mask)
    payload = _common_payload(args, prompt, requested_size)

    if request_mode == "json":
        payload["images"] = [_input_to_json_ref(image) for image in images]
        if mask is not None:
            payload["mask"] = _input_to_json_ref(mask)
        if args.dry_run:
            output_preview = _output_preview(
                prompt,
                args.title,
                requested_size,
                args.out_dir,
                args.out,
                args.output_format,
            )
            return {
                "ok": True,
                "mode": "dry-run",
                "command": "edit",
                "endpoint": endpoint,
                "request_mode": request_mode,
                "headers": _redacted_header_names(_request_headers(args, "application/json")),
                "requested_size": requested_size,
                "image_count": len(images),
                "has_mask": mask is not None,
                "output_preview": output_preview,
                "payload": payload,
            }
        _log(f"[ch-imagegen] edit endpoint={endpoint} mode=json")
        response = _post_json(endpoint, args, payload, "image edit response")
    else:
        fields = {k: v for k, v in payload.items() if v is not None}
        files = _multipart_files(images, mask, args.multipart_image_field)
        if args.dry_run:
            file_preview = [
                {
                    "field": item.field,
                    "path": str(item.path),
                    "filename": item.filename,
                    "content_type": item.content_type,
                    "bytes": len(item.data),
                }
                for item in files
            ]
            output_preview = _output_preview(
                prompt,
                args.title,
                requested_size,
                args.out_dir,
                args.out,
                args.output_format,
            )
            return {
                "ok": True,
                "mode": "dry-run",
                "command": "edit",
                "endpoint": endpoint,
                "request_mode": request_mode,
                "headers": _redacted_header_names(
                    _request_headers(args, "multipart/form-data; boundary=<generated>")
                ),
                "requested_size": requested_size,
                "fields": fields,
                "files": file_preview,
                "output_preview": output_preview,
            }
        _log(f"[ch-imagegen] edit endpoint={endpoint} mode=multipart files={len(files)}")
        response = _post_multipart(endpoint, args, fields, files, "image edit response")

    results = _extract_results(response, save_partials=args.save_partials)
    outputs = _write_results(
        results,
        prompt=prompt,
        title=args.title,
        requested_size=requested_size,
        out_dir=args.out_dir,
        out=args.out,
        output_format=args.output_format,
        force=args.force,
    )
    return {
        "ok": True,
        "command": "edit",
        "endpoint": endpoint,
        "request_mode": request_mode,
        "model": args.model,
        "requested_size": requested_size,
        "image_count": len(images),
        "has_mask": mask is not None,
        "outputs": outputs,
    }


def _output_preview(
    prompt: str,
    title: str | None,
    requested_size: str,
    out_dir: str,
    out: str | None,
    output_format: str | None,
) -> str:
    normalized_format = _normalize_output_format(output_format) or DEFAULT_OUTPUT_FORMAT
    suffix = "." + normalized_format.lower().lstrip(".")
    if suffix == ".jpeg":
        suffix = ".jpg"
    if out:
        path = Path(out).resolve()
        if path.suffix == "":
            path = path.with_suffix(suffix)
        return str(path)
    return str(
        Path(out_dir).resolve()
        / f"{_derive_title(prompt, title)}-{requested_size}-{_timestamp_ms()}{suffix}"
    )


def _read_batch_jobs(path: str) -> list[dict[str, Any]]:
    p = Path(path)
    if not p.exists():
        _die(f"batch input not found: {p}")
    jobs: list[dict[str, Any]] = []
    for line_no, raw in enumerate(p.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("{"):
            try:
                item = json.loads(line)
            except json.JSONDecodeError as exc:
                _die(f"invalid JSONL at line {line_no}: {exc}")
            if not isinstance(item, dict):
                _die(f"invalid JSONL at line {line_no}: expected object")
            if not str(item.get("prompt", "")).strip():
                _die(f"batch job line {line_no} is missing prompt")
            jobs.append(item)
        else:
            jobs.append({"prompt": line})
    if not jobs:
        _die("batch input contained no jobs")
    return jobs


def _namespace_for_job(args: argparse.Namespace, job: dict[str, Any], index: int) -> argparse.Namespace:
    data = vars(args).copy()
    data["prompt"] = str(job.get("prompt", "")).strip()
    data["prompt_file"] = None
    data["title"] = job.get("title") or f"{index:03d}-{_derive_title(data['prompt'], None)}"
    data["out"] = job.get("out")
    for key in [
        "model",
        "n",
        "size",
        "aspect",
        "resolution",
        "quality",
        "background",
        "output_format",
        "output_compression",
        "moderation",
        "response_format",
        "style",
        "user",
        "responses_model",
        "stream",
        "partial_images",
        "out_dir",
    ]:
        if key in job:
            data[key] = job[key]
    return argparse.Namespace(**data)


def _is_transient_error(exc: BaseException) -> bool:
    text = str(exc).lower()
    return any(
        marker in text
        for marker in [
            "http 429",
            "http 500",
            "http 502",
            "http 503",
            "http 504",
            "timed out",
            "timeout",
            "connection reset",
            "temporarily unavailable",
        ]
    )


def _run_generate_batch(args: argparse.Namespace) -> dict[str, Any]:
    jobs = _read_batch_jobs(args.input)
    if args.dry_run:
        results = []
        for index, job in enumerate(jobs, start=1):
            job_args = _namespace_for_job(args, job, index)
            results.append(_run_generate(job_args))
        return {
            "ok": True,
            "mode": "dry-run",
            "command": "generate-batch",
            "job_count": len(jobs),
            "jobs": results,
        }

    def run_one(index: int, job: dict[str, Any]) -> tuple[int, dict[str, Any]]:
        job_args = _namespace_for_job(args, job, index)
        last_exc: BaseException | None = None
        for attempt in range(1, args.max_attempts + 1):
            try:
                return index, _run_generate(job_args)
            except SystemExit as exc:
                last_exc = exc
                if not _is_transient_error(exc) or attempt == args.max_attempts:
                    raise
            except Exception as exc:
                last_exc = exc
                if not _is_transient_error(exc) or attempt == args.max_attempts:
                    raise
            sleep_s = min(60.0, 2.0**attempt)
            _warn(f"job {index} attempt {attempt} failed; retrying in {sleep_s:.1f}s")
            time.sleep(sleep_s)
        raise last_exc or RuntimeError("unknown batch job failure")

    outputs: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = {
            pool.submit(run_one, index, job): index for index, job in enumerate(jobs, start=1)
        }
        for future in concurrent.futures.as_completed(futures):
            index = futures[future]
            try:
                _, result = future.result()
                outputs.append(result)
            except BaseException as exc:
                failures.append({"job": index, "error": str(exc)})
                if args.fail_fast:
                    for pending in futures:
                        pending.cancel()
                    break

    outputs.sort(key=lambda item: item.get("outputs", [{}])[0].get("path", ""))
    return {
        "ok": not failures,
        "command": "generate-batch",
        "job_count": len(jobs),
        "outputs": outputs,
        "failures": failures,
    }


def _print_json(data: dict[str, Any]) -> None:
    print(json.dumps(data, ensure_ascii=False, indent=2))


def _add_common_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--prompt")
    parser.add_argument("--prompt-file")
    parser.add_argument("--title")
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--n", type=int, default=1)
    parser.add_argument("--size")
    parser.add_argument("--aspect", default=DEFAULT_ASPECT)
    parser.add_argument("--resolution", default=DEFAULT_RESOLUTION, choices=sorted(PIXEL_BUDGETS))
    parser.add_argument("--quality", default=DEFAULT_QUALITY)
    parser.add_argument("--background")
    parser.add_argument("--moderation")
    parser.add_argument("--response-format")
    parser.add_argument("--style")
    parser.add_argument("--output-format", default=DEFAULT_OUTPUT_FORMAT)
    parser.add_argument("--output-compression", type=int)
    parser.add_argument("--partial-images", type=int)
    parser.add_argument("--user")
    parser.add_argument("--responses-model")
    parser.add_argument("--stream", action="store_true")
    parser.add_argument("--save-partials", action="store_true")
    parser.add_argument("--out-dir", default=DEFAULT_OUT_DIR)
    parser.add_argument("--out")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--base-url", default=os.getenv("CH_IMAGEGEN_BASE_URL", DEFAULT_BASE_URL))
    parser.add_argument("--api-key", default=os.getenv("CH_IMAGEGEN_API_KEY"))
    parser.add_argument("--header", action="append")
    parser.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT)
    parser.add_argument("--progress-interval", type=int, default=DEFAULT_PROGRESS_INTERVAL)
    parser.add_argument("--dry-run", action="store_true")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Generate or edit images through a local relay")
    subparsers = parser.add_subparsers(dest="command", required=True)

    generate = subparsers.add_parser("generate", help="Create a new image")
    _add_common_args(generate)
    generate.set_defaults(func=_run_generate)

    edit = subparsers.add_parser("edit", help="Edit or transform image inputs")
    _add_common_args(edit)
    edit.add_argument("--image", action="append", required=True)
    edit.add_argument("--mask")
    edit.add_argument("--input-fidelity")
    edit.add_argument("--request-mode", choices=["auto", "json", "multipart"], default="auto")
    edit.add_argument(
        "--multipart-image-field",
        choices=["image", "image[]", "indexed"],
        default="image",
    )
    edit.set_defaults(func=_run_edit)

    batch = subparsers.add_parser("generate-batch", help="Generate many images from JSONL")
    _add_common_args(batch)
    batch.add_argument("--input", required=True)
    batch.add_argument("--concurrency", type=int, default=3)
    batch.add_argument("--max-attempts", type=int, default=3)
    batch.add_argument("--fail-fast", action="store_true")
    batch.set_defaults(func=_run_generate_batch)

    return parser


def _validate_args(args: argparse.Namespace) -> None:
    if args.n < 1 or args.n > 10:
        _die("--n must be between 1 and 10")
    if getattr(args, "concurrency", 1) < 1 or getattr(args, "concurrency", 1) > 25:
        _die("--concurrency must be between 1 and 25")
    if getattr(args, "max_attempts", 1) < 1 or getattr(args, "max_attempts", 1) > 10:
        _die("--max-attempts must be between 1 and 10")
    if args.output_compression is not None and not (0 <= args.output_compression <= 100):
        _die("--output-compression must be between 0 and 100")
    if args.partial_images is not None and args.partial_images < 0:
        _die("--partial-images must be >= 0")
    args.output_format = _normalize_output_format(args.output_format) or DEFAULT_OUTPUT_FORMAT


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] not in COMMANDS and argv[0].startswith("-"):
        argv.insert(0, "generate")
    parser = _build_parser()
    args = parser.parse_args(argv)
    _validate_args(args)
    result = args.func(args)
    _print_json(result)
    return 0 if result.get("ok", False) else 1


if __name__ == "__main__":
    raise SystemExit(main())

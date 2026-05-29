#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Iterable


TEXT_SUFFIXES = {
    "",
    ".log",
    ".jsonl",
    ".json",
    ".toml",
    ".txt",
    ".md",
}

SECRET_PATTERNS = [
    re.compile(r"(?i)(authorization\s*[:=]\s*bearer\s+)[^\s\"',}]+"),
    re.compile(r"(?i)((?:api[_-]?key|auth[_-]?token|access[_-]?token|refresh[_-]?token|cookie)\s*[:=]\s*)[^\s\"',}]+"),
    re.compile(r"(?i)(\"(?:api[_-]?key|auth[_-]?token|access[_-]?token|refresh[_-]?token|cookie)\"\s*:\s*\")[^\"]+"),
    re.compile(r"(?i)(\"encrypted_content\"\s*:\s*\")[^\"]+"),
]

ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
MAX_RENDERED_LINE = 2000

REQUEST_ID_PATTERNS = [
    re.compile(r"\brequest[_-]?id[\"'=:\s]+(\d{3,})\b", re.IGNORECASE),
    re.compile(r"\btrace[_-]?id[\"'=:\s]+codex-(\d{3,})\b", re.IGNORECASE),
    re.compile(r"\bcodex-(\d{3,})\b", re.IGNORECASE),
]


@dataclass
class MatchLine:
    path: str
    line: int
    text: str


def redact(text: str) -> str:
    redacted = text
    for pattern in SECRET_PATTERNS:
        redacted = pattern.sub(r"\1<redacted>", redacted)
    return redacted


def sanitize_line(text: str) -> str:
    clean = ANSI_RE.sub("", text)
    clean = redact(clean)
    if len(clean) > MAX_RENDERED_LINE:
        return clean[:MAX_RENDERED_LINE] + "... <truncated>"
    return clean


def home_path(*parts: str) -> Path:
    return Path.home().joinpath(*parts)


def default_roots() -> list[Path]:
    return [
        home_path(".codex-helper", "logs"),
        home_path(".codex-helper", "config.toml"),
        home_path(".codex-helper", "state"),
        home_path(".codex-helper", "run"),
        home_path(".codex", "sessions"),
        home_path(".codex", "logs"),
        home_path(".codex", "log"),
    ]


def existing_roots(paths: Iterable[Path]) -> list[Path]:
    return [path for path in paths if path.exists()]


def iter_text_files(root: Path) -> Iterable[Path]:
    if root.is_file():
        if root.suffix.lower() in TEXT_SUFFIXES:
            yield root
        return
    for path in root.rglob("*"):
        if path.is_file() and path.suffix.lower() in TEXT_SUFFIXES:
            yield path


def find_filename_matches(roots: list[Path], needle: str, limit: int) -> list[str]:
    needle_l = needle.lower()
    matches: list[str] = []
    for root in roots:
        for path in iter_text_files(root):
            if needle_l in str(path).lower():
                matches.append(str(path))
                if len(matches) >= limit:
                    return matches
    return matches


def parse_request_ids(text: str) -> list[str]:
    found: set[str] = set()
    for pattern in REQUEST_ID_PATTERNS:
        for match in pattern.finditer(text):
            found.add(match.group(1))
    return sorted(found, key=lambda value: int(value))


def run_rg(pattern: str, roots: list[Path], context: int, max_lines: int) -> tuple[list[str], str | None]:
    rg = shutil.which("rg")
    if not rg:
        return [], "rg not found"
    args = [
        rg,
        "--hidden",
        "--no-ignore",
        "--color",
        "never",
        "--max-columns",
        str(MAX_RENDERED_LINE),
        "--max-columns-preview",
        "-n",
        "-C",
        str(context),
        "-i",
        pattern,
    ]
    args.extend(str(path) for path in roots)
    try:
        proc = subprocess.run(
            args,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=60,
        )
    except Exception as exc:  # noqa: BLE001
        return [], f"rg failed: {exc}"
    if proc.returncode not in (0, 1):
        return [], proc.stderr.strip() or f"rg exited {proc.returncode}"
    lines = (proc.stdout or "").splitlines()
    return [sanitize_line(line) for line in lines[:max_lines]], None


def fallback_search(pattern: str, roots: list[Path], context: int, max_lines: int) -> list[str]:
    needle = pattern.lower()
    rendered: list[str] = []
    for root in roots:
        for path in iter_text_files(root):
            try:
                lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                continue
            matched = [idx for idx, line in enumerate(lines) if needle in line.lower()]
            for idx in matched:
                start = max(0, idx - context)
                end = min(len(lines), idx + context + 1)
                for line_no in range(start, end):
                    rendered.append(sanitize_line(f"{path}:{line_no + 1}:{lines[line_no]}"))
                    if len(rendered) >= max_lines:
                        return rendered
    return rendered


def search(pattern: str, roots: list[Path], context: int, max_lines: int) -> tuple[list[str], str | None]:
    lines, error = run_rg(pattern, roots, context, max_lines)
    if lines or error is None:
        return lines, error
    return fallback_search(pattern, roots, context, max_lines), error


def recent_files(root: Path, limit: int) -> list[dict[str, object]]:
    if not root.exists():
        return []
    files = [path for path in iter_text_files(root)]
    files.sort(key=lambda path: path.stat().st_mtime if path.exists() else 0, reverse=True)
    result = []
    for path in files[:limit]:
        try:
            stat = path.stat()
        except OSError:
            continue
        result.append(
            {
                "path": str(path),
                "size": stat.st_size,
                "modified": datetime.fromtimestamp(stat.st_mtime).isoformat(timespec="seconds"),
            }
        )
    return result


def emit_markdown(data: dict[str, object]) -> None:
    print("# Codex Session Context")
    print()
    print(f"- session_key: `{data['session_key']}`")
    print(f"- generated_at: `{data['generated_at']}`")
    print()
    print("## Roots")
    for root in data["roots"]:
        print(f"- `{root}`")
    print()
    print("## Filename Matches")
    filename_matches = data["filename_matches"]
    if filename_matches:
        for path in filename_matches:
            print(f"- `{path}`")
    else:
        print("- none")
    print()
    print("## Session Key Matches")
    for line in data["session_key_matches"] or ["none"]:
        print(line)
    print()
    print("## Derived Request IDs")
    ids = data["request_ids"]
    print(", ".join(f"`{item}`" for item in ids) if ids else "none")
    print()
    print("## Request ID Matches")
    for line in data["request_id_matches"] or ["none"]:
        print(line)
    print()
    print("## Recent Helper Log Files")
    for item in data["recent_helper_logs"]:
        print(f"- `{item['path']}` size={item['size']} modified={item['modified']}")


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser(description="Collect read-only local evidence for a Codex session key.")
    parser.add_argument("session_key", help="Codex session id/key or distinctive session substring")
    parser.add_argument("--context", type=int, default=2, help="context lines around matches")
    parser.add_argument("--max-lines", type=int, default=240, help="maximum rendered lines per search phase")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of Markdown")
    args = parser.parse_args()

    roots = existing_roots(default_roots())
    if not roots:
        print("No codex-helper or Codex roots found under the current home directory.", file=sys.stderr)
        return 2

    session_matches, session_search_warning = search(args.session_key, roots, args.context, args.max_lines)
    request_ids = parse_request_ids("\n".join(session_matches))
    request_id_matches: list[str] = []
    for request_id in request_ids[:12]:
        lines, _warning = search(request_id, roots, args.context, max(40, args.max_lines // max(1, len(request_ids[:12]))))
        request_id_matches.extend(lines)
    request_id_matches = request_id_matches[: args.max_lines]

    data: dict[str, object] = {
        "session_key": args.session_key,
        "generated_at": datetime.now().astimezone().isoformat(timespec="seconds"),
        "roots": [str(path) for path in roots],
        "warnings": [warning for warning in [session_search_warning] if warning],
        "filename_matches": find_filename_matches(roots, args.session_key, 80),
        "session_key_matches": session_matches,
        "request_ids": request_ids,
        "request_id_matches": request_id_matches,
        "recent_helper_logs": recent_files(home_path(".codex-helper", "logs"), 20),
    }

    if args.json:
        print(json.dumps(data, ensure_ascii=False, indent=2))
    else:
        emit_markdown(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

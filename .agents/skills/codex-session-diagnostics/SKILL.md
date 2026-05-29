---
name: codex-session-diagnostics
description: Diagnoses codex-helper and Codex failures from a Codex session key by correlating helper logs, helper configuration/state, and Codex session JSONL files. Use when the user provides a Codex session id/key or asks why Codex is waiting, stuck, disconnected, failed to resume, lost continuity, or behaved differently through codex-helper.
---

# Codex Session Diagnostics

Use this skill to investigate a specific Codex session from local evidence. The workflow is read-only by default and should build a precise timeline before proposing a fix.

## Quick Start

Run the collector from the repository root, or resolve the script relative to this skill directory:

```bash
python .agents/skills/codex-session-diagnostics/scripts/collect_session_context.py <SESSION_KEY>
```

Useful options:

- `--context 4` increases lines around each match.
- `--max-lines 400` raises the output cap per search phase.
- `--json` emits machine-readable output for follow-up parsing.

## Sources To Inspect

Prefer local evidence over guesses:

- `~/.codex-helper/logs/`: runtime logs, request ledger, retry/control traces, relay evidence.
- `~/.codex-helper/config.toml`: provider, endpoint, routing, retry, affinity, and client-patch settings.
- `~/.codex-helper/state/` and `~/.codex-helper/run/`: route affinity, runtime markers, daemon ownership, crash markers.
- `~/.codex/sessions/`: Codex rollout/session JSONL files.
- `~/.codex/logs/` or `~/.codex/log/` if present: Codex-side client logs.

Never print secrets. Redact bearer tokens, API keys, auth headers, cookies, and refresh/access tokens in summaries.

## Workflow

1. Identify the session key, symptom, and approximate wall-clock window. If the user already provided a key, start collecting evidence immediately.
2. Run the collector and note matched helper log files, Codex session files, derived `request_id` / `trace_id`, provider/endpoint, status code, TTFB, duration, usage, and terminal events.
3. If the collector finds request ids, search them directly across `~/.codex-helper/logs` and request ledgers. Exact request id correlation is stronger than broad timestamp correlation.
4. Compare the stuck request with the next successful resume request when available. Differences in duration, usage, terminal SSE events, selected provider, route affinity, or upstream errors often reveal the failure class.
5. Classify the failure before fixing:
   - upstream HTTP error before stream start;
   - HTTP 200 followed by SSE body read error or idle/no bytes;
   - missing `response.completed` / `response.failed` terminal event;
   - client disconnect/drop after long wait;
   - route affinity or remote compaction continuity failure;
   - balance/cooldown/no-route selection failure;
   - proxy, DNS, TLS, or outbound proxy failure;
   - Codex local session JSONL corruption or resume mismatch.
6. State what evidence proves the diagnosis, what evidence is absent, and which code path or config should change.
7. If a code fix is needed, switch to a normal diagnose/fix loop: add a regression test at the proxy seam, implement, run targeted tests, then update changelog/docs when user-visible.

## Reporting Template

Report in this order:

- **Session**: key, exact local time window, matched files.
- **Timeline**: request ids, provider/endpoint, status, TTFB, duration, usage, terminal event.
- **Finding**: the most likely root cause and why competing explanations are weaker.
- **Fix/Next step**: config change, code change, retry command, or requested artifact.
- **Privacy**: mention any redaction applied and avoid copying tokens.

## Guardrails

- Do not modify files under `~/.codex-helper` or `~/.codex` during diagnosis unless the user explicitly asks for repair.
- Do not delete logs, sessions, state, or config while investigating.
- Do not assume the latest log entry belongs to the session; correlate by session key, request id, trace id, and timestamp.
- Use concrete timestamps with timezone when the user mentions "today", "yesterday", waiting duration, or resume timing.

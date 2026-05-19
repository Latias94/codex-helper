# Task Ledger

## CBD-010 Bridge Diagnostic Model

Status: completed

Owner: codex

Scope: `crates/core/src/codex_integration.rs`, unit tests.

Deliverable: reusable bridge diagnostic report with checks for official provider identity, websocket setting, auth facade, upstream credentials, and remote compaction v2 feature risk.

Validation: targeted `cargo nextest run -p codex-helper-core codex_bridge`.

## CBD-020 CLI Doctor/Status Output

Status: completed

Owner: codex

Scope: `src/commands/doctor.rs`, `src/cli_app.rs`.

Deliverable: human and JSON-visible diagnostics for official bridge modes.

Validation: `cargo check --workspace` and workspace nextest.

## CBD-030 Compact Request Observability

Status: completed

Owner: codex

Scope: `crates/core/src/logging.rs`, proxy request preparation/finalization tests.

Deliverable: compact requests and bridge mode are serialized in request logs/control traces without exposing secrets.

Validation: targeted logging/proxy tests and workspace nextest.

## CBD-040 Verification and Closeout

Status: completed

Owner: codex

Scope: docs and evidence.

Deliverable: fmt, targeted tests, workspace tests, closeout notes.

Validation: see `EVIDENCE_AND_GATES.md`.

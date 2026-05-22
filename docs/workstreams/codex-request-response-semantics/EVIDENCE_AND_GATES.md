# Codex Request Response Semantics - Evidence And Gates

Status: Complete
Last updated: 2026-05-22

## Required Gates

| Gate | Command | Status | Evidence |
| --- | --- | --- | --- |
| Format | `cargo fmt --package codex-helper-core` | Passed | Ran 2026-05-22. |
| P1 previous response | `cargo nextest run -p codex-helper-core previous_response_id` | Passed | 2 tests passed, 621 skipped. |
| P1 session completion | `cargo nextest run -p codex-helper-core session_completion` | Passed | 1 test passed, 622 skipped. |
| P2 service tier | `cargo nextest run -p codex-helper-core service_tier` | Passed | 3 tests passed, 621 skipped. |
| P2 response fixer | `cargo nextest run -p codex-helper-core response_fixer` | Passed | 4 tests passed, 619 skipped. |
| Full core | `cargo nextest run -p codex-helper-core` | Passed | 624 tests passed. |

## Evidence Notes

- No unrelated failures were observed in the full core gate.

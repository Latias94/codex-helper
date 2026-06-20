# Engineering Memory Update Log

## 2026-06-20
* **Initialization**: Created engineering wiki memory bundle.
* **Memory optimization pass**: Confirmed the remaining hot path is the `recent_finished` VecDeque of full `FinishedRequest` records. Forecast samples and provider balance history stay lightweight, `CodexRecentBranchCache` is bounded, and the recent retention default is now 1000 instead of 2000.
* **Follow-up optimization**: Re-verified the shrink pass after fixing a forecast trait regression. `recent_finished_max()` still defaults to 1000; forecast tail default is also 1000 with clamp `200..10000`; forecast ledger cache now stores shared `Arc` slices instead of cloning the ledger Vec on cache hits; forecast sample rows dropped the redundant `service` field. Focused `cargo fmt`, `cargo check`, and `cargo nextest` all passed.

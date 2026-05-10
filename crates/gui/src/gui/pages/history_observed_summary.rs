pub(super) use super::history_observed_card_summary::{
    build_observed_history_summaries_from_cards, history_service_tier_display,
};
pub(super) use super::history_observed_runtime::{
    history_session_supports_local_actions, load_local_history_sessions,
    observed_session_row_from_snapshot, resolve_history_sessions_with_fallback,
};
use super::*;

pub(super) fn build_observed_history_summaries(
    snapshot: &GuiRuntimeSnapshot,
    lang: Language,
) -> Vec<SessionSummary> {
    let now = now_ms();
    if !snapshot.session_cards.is_empty() {
        return build_observed_history_summaries_from_cards(snapshot, lang, now);
    }
    super::history_observed_request_summary::build_observed_history_summaries_from_requests(
        snapshot, lang, now,
    )
}

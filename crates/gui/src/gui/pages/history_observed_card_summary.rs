use std::path::PathBuf;

use super::*;

fn observed_summary_sort_ms(card: &SessionIdentityCard) -> Option<u64> {
    card.last_ended_at_ms.or(card.active_started_at_ms_min)
}

fn observed_card_effective_station_name(card: &SessionIdentityCard) -> Option<&str> {
    card.effective_station
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_station_name.as_deref())
}

pub(super) fn history_service_tier_display(value: Option<&str>, lang: Language) -> String {
    super::format_service_tier_display(value, lang, "auto")
}

fn observed_route_summary_from_card(card: &SessionIdentityCard, lang: Language) -> String {
    let station = observed_card_effective_station_name(card).unwrap_or("auto");
    let model = card
        .effective_model
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_model.as_deref())
        .unwrap_or("auto");
    let tier = card
        .effective_service_tier
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_service_tier.as_deref())
        .map(|value| history_service_tier_display(Some(value), lang))
        .unwrap_or_else(|| "auto".to_string());

    let mut parts = vec![
        format!("station={station}"),
        format!("model={model}"),
        format!("tier={tier}"),
    ];
    if let Some(provider) = card.last_provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(client) = super::format_observed_client_identity(
        card.last_client_name.as_deref(),
        card.last_client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    if let Some(profile) = card.binding_profile_name.as_deref() {
        parts.push(format!("profile={profile}"));
    }
    if let Some(status) = card.last_status {
        parts.push(format!("status={status}"));
    }
    if card.active_count > 0 {
        parts.push(format!("active={}", card.active_count));
    }
    format!(
        "{}: {}",
        pick(lang, "共享观测", "Observed"),
        parts.join(", ")
    )
}

fn build_observed_summary_from_card(
    card: &SessionIdentityCard,
    lang: Language,
    now: u64,
) -> Option<SessionSummary> {
    let sid = card.session_id.clone()?;
    let sort_hint_ms = observed_summary_sort_ms(card);
    let updated_at = sort_hint_ms.map(|ms| format_age(now, Some(ms)));
    let turns = card.turns_total.unwrap_or(0).min(usize::MAX as u64) as usize;
    Some(SessionSummary {
        id: sid,
        path: PathBuf::new(),
        cwd: card.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(observed_route_summary_from_card(card, lang)),
        source: SessionSummarySource::ObservedOnly,
        sort_hint_ms,
    })
}

pub(super) fn build_observed_history_summaries_from_cards(
    snapshot: &GuiRuntimeSnapshot,
    lang: Language,
    now: u64,
) -> Vec<SessionSummary> {
    let mut out = snapshot
        .session_cards
        .iter()
        .filter_map(|card| build_observed_summary_from_card(card, lang, now))
        .collect::<Vec<_>>();
    sort_session_summaries_by_mtime_desc(&mut out);
    out
}

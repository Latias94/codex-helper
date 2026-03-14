use std::collections::HashMap;
use std::path::PathBuf;

use super::history_observed_card_summary::history_service_tier_display;
use super::*;

#[derive(Debug, Default)]
struct ObservedAggregate {
    cwd: Option<String>,
    sort_hint_ms: Option<u64>,
    client_name: Option<String>,
    client_addr: Option<String>,
    model: Option<String>,
    tier: Option<String>,
    station: Option<String>,
    provider: Option<String>,
    status: Option<u16>,
    active_count: u64,
}

fn collect_active_request_aggregate(
    map: &mut HashMap<String, ObservedAggregate>,
    req: &ActiveRequest,
) {
    let Some(sid) = req.session_id.as_deref().map(str::to_owned) else {
        return;
    };
    let entry = map.entry(sid).or_default();
    if entry.cwd.is_none() {
        entry.cwd = req.cwd.clone();
    }
    if entry.client_name.is_none() {
        entry.client_name = req.client_name.clone();
    }
    if entry.client_addr.is_none() {
        entry.client_addr = req.client_addr.clone();
    }
    entry.sort_hint_ms = Some(
        entry
            .sort_hint_ms
            .unwrap_or(req.started_at_ms)
            .max(req.started_at_ms),
    );
    if entry.model.is_none() {
        entry.model = req.model.clone();
    }
    if entry.tier.is_none() {
        entry.tier = req.service_tier.clone();
    }
    if entry.station.is_none() {
        entry.station = req.station_name.clone();
    }
    if entry.provider.is_none() {
        entry.provider = req.provider_id.clone();
    }
    entry.active_count = entry.active_count.saturating_add(1);
}

fn collect_recent_request_aggregate(
    map: &mut HashMap<String, ObservedAggregate>,
    req: &FinishedRequest,
) {
    let Some(sid) = req.session_id.as_deref().map(str::to_owned) else {
        return;
    };
    let entry = map.entry(sid).or_default();
    if entry.cwd.is_none() {
        entry.cwd = req.cwd.clone();
    }
    entry.client_name = req.client_name.clone().or(entry.client_name.clone());
    entry.client_addr = req.client_addr.clone().or(entry.client_addr.clone());
    entry.sort_hint_ms = Some(
        entry
            .sort_hint_ms
            .unwrap_or(req.ended_at_ms)
            .max(req.ended_at_ms),
    );
    entry.model = req.model.clone().or(entry.model.clone());
    entry.tier = req.service_tier.clone().or(entry.tier.clone());
    entry.station = req.station_name.clone().or(entry.station.clone());
    entry.provider = req.provider_id.clone().or(entry.provider.clone());
    entry.status = Some(req.status_code);
}

fn observed_summary_from_aggregate(
    sid: String,
    aggregate: ObservedAggregate,
    lang: Language,
    now: u64,
) -> SessionSummary {
    let updated_at = aggregate.sort_hint_ms.map(|ms| format_age(now, Some(ms)));
    let mut parts = vec![
        format!("station={}", aggregate.station.as_deref().unwrap_or("auto")),
        format!("model={}", aggregate.model.as_deref().unwrap_or("auto")),
        format!(
            "tier={}",
            history_service_tier_display(aggregate.tier.as_deref(), lang)
        ),
    ];
    if let Some(provider) = aggregate.provider.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(client) = super::format_observed_client_identity(
        aggregate.client_name.as_deref(),
        aggregate.client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    if let Some(status) = aggregate.status {
        parts.push(format!("status={status}"));
    }
    if aggregate.active_count > 0 {
        parts.push(format!("active={}", aggregate.active_count));
    }

    SessionSummary {
        id: sid,
        path: PathBuf::new(),
        cwd: aggregate.cwd,
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: 0,
        assistant_turns: 0,
        rounds: 0,
        first_user_message: Some(format!(
            "{}: {}",
            pick(lang, "共享观测", "Observed"),
            parts.join(", ")
        )),
        source: SessionSummarySource::ObservedOnly,
        sort_hint_ms: aggregate.sort_hint_ms,
    }
}

pub(super) fn build_observed_history_summaries_from_requests(
    snapshot: &GuiRuntimeSnapshot,
    lang: Language,
    now: u64,
) -> Vec<SessionSummary> {
    let mut map: HashMap<String, ObservedAggregate> = HashMap::new();
    for req in snapshot.active.iter() {
        collect_active_request_aggregate(&mut map, req);
    }
    for req in snapshot.recent.iter() {
        collect_recent_request_aggregate(&mut map, req);
    }

    let mut out = map
        .into_iter()
        .map(|(sid, aggregate)| observed_summary_from_aggregate(sid, aggregate, lang, now))
        .collect::<Vec<_>>();
    sort_session_summaries_by_mtime_desc(&mut out);
    out
}

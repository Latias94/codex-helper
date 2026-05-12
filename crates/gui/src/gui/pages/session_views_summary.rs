use super::*;

fn resolved_route_value_text(value: Option<&ResolvedRouteValue>) -> Option<&str> {
    value.map(|value| value.value.as_str())
}

pub(super) fn session_row_matches_query(row: &SessionRow, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    for value in [
        row.session_id.as_deref(),
        row.last_client_name.as_deref(),
        row.last_client_addr.as_deref(),
        row.cwd.as_deref(),
        row.last_model.as_deref(),
        row.last_service_tier.as_deref(),
        row.last_provider_id.as_deref(),
        row.last_station_name(),
        row.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.provider_id.as_deref()),
        row.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.endpoint_id.as_deref()),
        row.route_affinity
            .as_ref()
            .map(|affinity| affinity.upstream_base_url.as_str()),
        row.last_route_decision
            .as_ref()
            .and_then(|decision| decision.endpoint_id.as_deref()),
        row.last_upstream_base_url.as_deref(),
        row.binding_profile_name.as_deref(),
        row.effective_model.as_ref().map(|v| v.value.as_str()),
        row.effective_reasoning_effort
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_service_tier
            .as_ref()
            .map(|v| v.value.as_str()),
        row.effective_station().map(|v| v.value.as_str()),
        row.effective_upstream_base_url
            .as_ref()
            .map(|v| v.value.as_str()),
        row.override_model.as_deref(),
        row.override_effort.as_deref(),
        row.override_station_name(),
        row.override_service_tier.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.to_lowercase().contains(q) {
            return true;
        }
    }
    false
}

pub(super) fn session_effective_route_inline_summary(row: &SessionRow, lang: Language) -> String {
    let service_tier = row
        .effective_service_tier
        .as_ref()
        .map(|value| format_service_tier_display(Some(value.value.as_str()), lang, "-"))
        .or_else(|| {
            row.last_service_tier
                .as_deref()
                .map(|value| format_service_tier_display(Some(value), lang, "-"))
        })
        .unwrap_or_else(|| pick(lang, "<未解析>", "<unresolved>").to_string());
    format!(
        "station={}, model={}, reasoning={}, service_tier={}",
        session_route_preview_value(row.effective_station(), row.last_station_name(), lang),
        session_route_preview_value(
            row.effective_model.as_ref(),
            row.last_model.as_deref(),
            lang
        ),
        session_route_preview_value(
            row.effective_reasoning_effort.as_ref(),
            row.last_reasoning_effort.as_deref(),
            lang,
        ),
        service_tier,
    )
}

pub(super) fn session_current_target_summary(row: &SessionRow, lang: Language) -> String {
    let station = format_resolved_route_value_for_field(
        row.effective_station(),
        EffectiveRouteField::Station,
        lang,
    );
    let upstream = row
        .effective_upstream_base_url
        .as_ref()
        .map(|value| {
            format!(
                "{} [{}]",
                summarize_upstream_target(&value.value, 56),
                route_value_source_label(value.source, lang)
            )
        })
        .unwrap_or_else(|| "-".to_string());

    let current_station = resolved_route_value_text(row.effective_station());
    let current_upstream = resolved_route_value_text(row.effective_upstream_base_url.as_ref());
    let provider = if let Some(affinity) = row.route_affinity.as_ref() {
        format_route_decision_provider_endpoint(
            affinity.provider_id.as_deref(),
            affinity.endpoint_id.as_deref(),
        )
        .map(|provider| {
            format!(
                "{provider} [{}]",
                pick(lang, "session 粘性", "session affinity")
            )
        })
    } else if let Some(decision) = row.last_route_decision.as_ref() {
        let decision_station = decision
            .effective_station
            .as_ref()
            .map(|value| value.value.as_str());
        let decision_upstream = decision
            .effective_upstream_base_url
            .as_ref()
            .map(|value| value.value.as_str());
        if current_station == decision_station && current_upstream == decision_upstream {
            format_route_decision_provider_endpoint(
                decision.provider_id.as_deref(),
                decision.endpoint_id.as_deref(),
            )
            .map(|provider| format!("{provider} [{}]", pick(lang, "最近决策", "last decision")))
        } else {
            None
        }
    } else {
        None
    }
    .or_else(|| {
        let observed_station = row.last_station_name();
        let observed_upstream = row.last_upstream_base_url.as_deref();
        if current_station == observed_station && current_upstream == observed_upstream {
            format_route_decision_provider_endpoint(row.last_provider_id.as_deref(), None).map(
                |provider| format!("{provider} [{}]", pick(lang, "最近执行", "last execution")),
            )
        } else {
            None
        }
    })
    .unwrap_or_else(|| pick(lang, "<需新请求刷新>", "<needs fresh request>").to_string());

    format!("station={station}, provider={provider}, upstream={upstream}")
}

pub(super) fn session_route_affinity_summary(row: &SessionRow, lang: Language) -> Option<String> {
    let affinity = row.route_affinity.as_ref()?;
    let provider = format_route_decision_provider_endpoint(
        affinity.provider_id.as_deref(),
        affinity.endpoint_id.as_deref(),
    )
    .unwrap_or_else(|| "-".to_string());
    Some(format!(
        "{} / {} / {} [{}] {}",
        affinity.station_name,
        provider,
        shorten_middle(&affinity.upstream_base_url, 56),
        pick(lang, "路由图", "route graph"),
        affinity.change_reason,
    ))
}

pub(super) fn session_last_executed_target_summary(row: &SessionRow, lang: Language) -> String {
    let upstream = row
        .last_upstream_base_url
        .as_deref()
        .map(|value| summarize_upstream_target(value, 56))
        .unwrap_or_else(|| "-".to_string());
    let provider = if let Some(decision) = row.last_route_decision.as_ref() {
        format_route_decision_provider_endpoint(
            decision.provider_id.as_deref(),
            decision.endpoint_id.as_deref(),
        )
        .or_else(|| row.last_provider_id.as_deref().map(ToOwned::to_owned))
    } else {
        row.last_provider_id.as_deref().map(ToOwned::to_owned)
    }
    .unwrap_or_else(|| "-".to_string());

    format!(
        "station={}, provider={}, upstream={}, service_tier={}",
        row.last_station_name().unwrap_or("-"),
        provider,
        upstream,
        format_service_tier_display(row.last_service_tier.as_deref(), lang, "-"),
    )
}

pub(super) fn session_last_activity_summary(row: &SessionRow) -> String {
    let status = row
        .last_status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "-".to_string());
    let duration = row
        .last_duration_ms
        .map(|duration| format!("{duration} ms"))
        .unwrap_or_else(|| "-".to_string());
    let last = format_age(now_ms(), row.last_ended_at_ms);
    format!("status={status}, duration={duration}, last={last}")
}

pub(super) fn session_list_control_label(row: &SessionRow) -> String {
    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        return format!("pf:{}", shorten(profile_name, 10));
    }
    if let Some(station_name) = row.override_station_name() {
        return format!("pin:{}", shorten(station_name, 10));
    }
    let override_count = usize::from(row.override_model.is_some())
        + usize::from(row.override_effort.is_some())
        + usize::from(row.override_service_tier.is_some());
    if override_count > 0 {
        return format!("ovr:{override_count}");
    }
    if row.effective_station_source() == Some(RouteValueSource::GlobalOverride) {
        return "global".to_string();
    }
    "-".to_string()
}

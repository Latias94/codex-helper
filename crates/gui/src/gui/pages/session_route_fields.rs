use super::session_route_state::EffectiveRouteField;
use super::*;

pub(super) fn route_value_source_label(source: RouteValueSource, lang: Language) -> &'static str {
    match source {
        RouteValueSource::RequestPayload => pick(lang, "请求体", "request payload"),
        RouteValueSource::SessionOverride => pick(lang, "会话覆盖", "session override"),
        RouteValueSource::GlobalOverride => pick(lang, "全局覆盖", "global override"),
        RouteValueSource::ProfileDefault => pick(lang, "profile 默认", "profile default"),
        RouteValueSource::StationMapping => pick(lang, "站点映射", "station mapping"),
        RouteValueSource::RuntimeFallback => pick(lang, "运行时兜底", "runtime fallback"),
    }
}

pub(super) fn unresolved_route_source_label(lang: Language) -> &'static str {
    pick(lang, "未解析", "unresolved")
}

pub(super) fn effective_route_field_label(
    field: EffectiveRouteField,
    lang: Language,
) -> &'static str {
    match field {
        EffectiveRouteField::Model => pick(lang, "模型", "model"),
        EffectiveRouteField::Station => pick(lang, "站点", "station"),
        EffectiveRouteField::Upstream => "upstream",
        EffectiveRouteField::Effort => pick(lang, "思考强度", "effort"),
        EffectiveRouteField::ServiceTier => "service_tier",
    }
}

pub(super) fn effective_route_field_value(
    row: &SessionRow,
    field: EffectiveRouteField,
) -> Option<&ResolvedRouteValue> {
    match field {
        EffectiveRouteField::Model => row.effective_model.as_ref(),
        EffectiveRouteField::Station => row.effective_station(),
        EffectiveRouteField::Upstream => row.effective_upstream_base_url.as_ref(),
        EffectiveRouteField::Effort => row.effective_reasoning_effort.as_ref(),
        EffectiveRouteField::ServiceTier => row.effective_service_tier.as_ref(),
    }
}

pub(super) fn route_decision_field_value(
    decision: &RouteDecisionProvenance,
    field: EffectiveRouteField,
) -> Option<&ResolvedRouteValue> {
    match field {
        EffectiveRouteField::Model => decision.effective_model.as_ref(),
        EffectiveRouteField::Station => decision.effective_station.as_ref(),
        EffectiveRouteField::Upstream => decision.effective_upstream_base_url.as_ref(),
        EffectiveRouteField::Effort => decision.effective_reasoning_effort.as_ref(),
        EffectiveRouteField::ServiceTier => decision.effective_service_tier.as_ref(),
    }
}

pub(super) fn route_decision_changed_fields(row: &SessionRow, lang: Language) -> Vec<String> {
    let Some(decision) = row.last_route_decision.as_ref() else {
        return Vec::new();
    };
    EffectiveRouteField::ALL
        .into_iter()
        .filter(|field| {
            effective_route_field_value(row, *field) != route_decision_field_value(decision, *field)
        })
        .map(|field| effective_route_field_label(field, lang).to_string())
        .collect()
}

pub(super) fn session_route_decision_status_line(row: &SessionRow, lang: Language) -> String {
    let Some(decision) = row.last_route_decision.as_ref() else {
        return pick(
            lang,
            "暂无最近路由决策快照",
            "No recent route decision snapshot",
        )
        .to_string();
    };
    let age = format_age(now_ms(), Some(decision.decided_at_ms));
    let changed = route_decision_changed_fields(row, lang);
    if changed.is_empty() {
        format!(
            "{}: {}",
            pick(
                lang,
                "最近路由决策仍与当前 effective route 一致",
                "Last route decision still matches the current effective route",
            ),
            age
        )
    } else {
        format!(
            "{}: {} ({})",
            pick(lang, "最近路由决策快照", "Last route decision snapshot"),
            age,
            changed.join(", ")
        )
    }
}

pub(super) fn binding_profile_reference(row: &SessionRow, lang: Language) -> String {
    match row.binding_profile_name.as_deref() {
        Some(name) => format!("profile {name}"),
        None => pick(lang, "当前绑定 profile", "the bound profile").to_string(),
    }
}

pub(super) fn session_route_preview_value(
    resolved: Option<&ResolvedRouteValue>,
    fallback: Option<&str>,
    lang: Language,
) -> String {
    resolved
        .map(|value| value.value.clone())
        .or_else(|| {
            fallback
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| pick(lang, "<未解析>", "<unresolved>").to_string())
}

pub(super) fn session_profile_target_value(raw: Option<&str>, lang: Language) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>").to_string())
}

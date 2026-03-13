use crate::lb::SelectedUpstream;
use crate::model_routing;
use crate::state::{ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SessionBinding};

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_request_field_provenance(
    request_value: Option<&str>,
    override_value: Option<&str>,
    binding_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = trim_non_empty(override_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    if let Some(value) = trim_non_empty(binding_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::ProfileDefault,
        ));
    }
    trim_non_empty(request_value)
        .map(|value| ResolvedRouteValue::new(value, RouteValueSource::RequestPayload))
}

fn resolve_station_provenance(
    selected_station_name: &str,
    session_override_config: Option<&str>,
    global_config_override: Option<&str>,
    binding_station_name: Option<&str>,
) -> ResolvedRouteValue {
    if let Some(value) = trim_non_empty(session_override_config) {
        return ResolvedRouteValue::new(value, RouteValueSource::SessionOverride);
    }
    if let Some(value) = trim_non_empty(global_config_override) {
        return ResolvedRouteValue::new(value, RouteValueSource::GlobalOverride);
    }
    if let Some(value) = trim_non_empty(binding_station_name) {
        return ResolvedRouteValue::new(value, RouteValueSource::ProfileDefault);
    }
    ResolvedRouteValue::new(
        selected_station_name.to_string(),
        RouteValueSource::RuntimeFallback,
    )
}

pub(super) fn build_route_decision_provenance(
    decided_at_ms: u64,
    session_binding: Option<&SessionBinding>,
    session_override_config: Option<&str>,
    global_config_override: Option<&str>,
    override_model: Option<&str>,
    override_effort: Option<&str>,
    override_service_tier: Option<&str>,
    request_model: Option<&str>,
    effective_effort: Option<&str>,
    effective_service_tier: Option<&str>,
    selected: &SelectedUpstream,
    provider_id: Option<&str>,
) -> RouteDecisionProvenance {
    let mut effective_model = resolve_request_field_provenance(
        request_model,
        override_model,
        session_binding.and_then(|binding| binding.model.as_deref()),
    );
    if let Some(current) = effective_model.as_mut() {
        let mapped = model_routing::effective_model(
            &selected.upstream.model_mapping,
            current.value.as_str(),
        );
        if mapped != current.value {
            *current = ResolvedRouteValue::new(mapped, RouteValueSource::StationMapping);
        }
    }

    RouteDecisionProvenance {
        decided_at_ms,
        binding_profile_name: session_binding.and_then(|binding| binding.profile_name.clone()),
        binding_continuity_mode: session_binding.map(|binding| binding.continuity_mode),
        effective_model,
        effective_reasoning_effort: resolve_request_field_provenance(
            effective_effort,
            override_effort,
            session_binding.and_then(|binding| binding.reasoning_effort.as_deref()),
        ),
        effective_service_tier: resolve_request_field_provenance(
            effective_service_tier,
            override_service_tier,
            session_binding.and_then(|binding| binding.service_tier.as_deref()),
        ),
        effective_station: Some(resolve_station_provenance(
            selected.station_name.as_str(),
            session_override_config,
            global_config_override,
            session_binding.and_then(|binding| binding.station_name.as_deref()),
        )),
        effective_upstream_base_url: Some(ResolvedRouteValue::new(
            selected.upstream.base_url.clone(),
            RouteValueSource::RuntimeFallback,
        )),
        provider_id: trim_non_empty(provider_id),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::build_route_decision_provenance;
    use super::{
        ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SelectedUpstream,
        SessionBinding,
    };
    use crate::config::{UpstreamAuth, UpstreamConfig};
    use crate::state::SessionContinuityMode;

    fn make_binding() -> SessionBinding {
        SessionBinding {
            session_id: "session-1".to_string(),
            profile_name: Some("default".to_string()),
            station_name: Some("bound-station".to_string()),
            model: Some("binding-model".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_seen_ms: 3,
        }
    }

    fn make_selected_upstream(model_mapping: &[(&str, &str)]) -> SelectedUpstream {
        SelectedUpstream {
            station_name: "selected-station".to_string(),
            index: 0,
            upstream: UpstreamConfig {
                base_url: "https://example.com/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: model_mapping
                    .iter()
                    .map(|(from, to)| (from.to_string(), to.to_string()))
                    .collect(),
            },
        }
    }

    fn assert_route_value(
        value: Option<ResolvedRouteValue>,
        expected_value: &str,
        expected_source: RouteValueSource,
    ) {
        assert_eq!(
            value,
            Some(ResolvedRouteValue {
                value: expected_value.to_string(),
                source: expected_source,
            })
        );
    }

    #[test]
    fn route_provenance_prefers_session_override_then_binding_then_request_payload() {
        let binding = make_binding();
        let selected = make_selected_upstream(&[]);

        let decision = build_route_decision_provenance(
            42,
            Some(&binding),
            Some("session-station"),
            Some("global-station"),
            Some("override-model"),
            Some("medium"),
            Some("flex"),
            Some("request-model"),
            Some("request-effort"),
            Some("request-tier"),
            &selected,
            Some(" provider-1 "),
        );

        assert_eq!(decision.binding_profile_name.as_deref(), Some("default"));
        assert_eq!(
            decision.binding_continuity_mode,
            Some(SessionContinuityMode::ManualProfile)
        );
        assert_route_value(
            decision.effective_model,
            "override-model",
            RouteValueSource::SessionOverride,
        );
        assert_route_value(
            decision.effective_reasoning_effort,
            "medium",
            RouteValueSource::SessionOverride,
        );
        assert_route_value(
            decision.effective_service_tier,
            "flex",
            RouteValueSource::SessionOverride,
        );
        assert_route_value(
            decision.effective_station,
            "session-station",
            RouteValueSource::SessionOverride,
        );
        assert_route_value(
            decision.effective_upstream_base_url,
            "https://example.com/v1",
            RouteValueSource::RuntimeFallback,
        );
        assert_eq!(decision.provider_id.as_deref(), Some("provider-1"));
    }

    #[test]
    fn route_provenance_uses_binding_and_runtime_fallback_when_overrides_absent() {
        let binding = make_binding();
        let selected = make_selected_upstream(&[]);

        let decision = build_route_decision_provenance(
            7,
            Some(&binding),
            Some("   "),
            None,
            None,
            None,
            None,
            Some("request-model"),
            Some("request-effort"),
            Some("request-tier"),
            &selected,
            Some(""),
        );

        assert_route_value(
            decision.effective_model,
            "binding-model",
            RouteValueSource::ProfileDefault,
        );
        assert_route_value(
            decision.effective_reasoning_effort,
            "high",
            RouteValueSource::ProfileDefault,
        );
        assert_route_value(
            decision.effective_service_tier,
            "priority",
            RouteValueSource::ProfileDefault,
        );
        assert_route_value(
            decision.effective_station,
            "bound-station",
            RouteValueSource::ProfileDefault,
        );
        assert_eq!(decision.provider_id, None);
    }

    #[test]
    fn route_provenance_marks_station_mapping_when_model_is_remapped() {
        let selected = make_selected_upstream(&[("gpt-5", "provider-model")]);

        let decision = build_route_decision_provenance(
            9,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("gpt-5"),
            None,
            None,
            &selected,
            None,
        );

        assert_route_value(
            decision.effective_model,
            "provider-model",
            RouteValueSource::StationMapping,
        );
        assert_route_value(
            decision.effective_station,
            "selected-station",
            RouteValueSource::RuntimeFallback,
        );
    }

    #[test]
    fn route_provenance_keeps_request_payload_when_no_binding_or_override_exists() {
        let selected = make_selected_upstream(&[]);

        let decision = build_route_decision_provenance(
            11,
            None,
            None,
            Some("global-station"),
            None,
            None,
            None,
            Some("request-model"),
            Some("low"),
            Some("priority"),
            &selected,
            None,
        );

        assert_eq!(
            decision,
            RouteDecisionProvenance {
                decided_at_ms: 11,
                binding_profile_name: None,
                binding_continuity_mode: None,
                effective_model: Some(ResolvedRouteValue {
                    value: "request-model".to_string(),
                    source: RouteValueSource::RequestPayload,
                }),
                effective_reasoning_effort: Some(ResolvedRouteValue {
                    value: "low".to_string(),
                    source: RouteValueSource::RequestPayload,
                }),
                effective_service_tier: Some(ResolvedRouteValue {
                    value: "priority".to_string(),
                    source: RouteValueSource::RequestPayload,
                }),
                effective_station: Some(ResolvedRouteValue {
                    value: "global-station".to_string(),
                    source: RouteValueSource::GlobalOverride,
                }),
                effective_upstream_base_url: Some(ResolvedRouteValue {
                    value: "https://example.com/v1".to_string(),
                    source: RouteValueSource::RuntimeFallback,
                }),
                provider_id: None,
            }
        );
    }
}

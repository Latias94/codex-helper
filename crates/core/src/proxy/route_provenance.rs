use crate::model_routing;
use crate::state::{ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SessionBinding};

use super::attempt_target::AttemptTarget;

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
    selected_station_name: Option<&str>,
    session_override_config: Option<&str>,
    global_config_override: Option<&str>,
    binding_station_name: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = trim_non_empty(session_override_config) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::SessionOverride,
        ));
    }
    if let Some(value) = trim_non_empty(global_config_override) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::GlobalOverride,
        ));
    }
    if let Some(value) = trim_non_empty(binding_station_name) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::ProfileDefault,
        ));
    }
    trim_non_empty(selected_station_name)
        .map(|station| ResolvedRouteValue::new(station, RouteValueSource::RuntimeFallback))
}

pub(super) struct RouteDecisionProvenanceParams<'a> {
    pub(super) decided_at_ms: u64,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) session_override_config: Option<&'a str>,
    pub(super) global_config_override: Option<&'a str>,
    pub(super) override_model: Option<&'a str>,
    pub(super) override_effort: Option<&'a str>,
    pub(super) override_service_tier: Option<&'a str>,
    pub(super) request_model: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) target: &'a AttemptTarget,
    pub(super) provider_id: Option<&'a str>,
}

pub(super) fn build_route_decision_provenance(
    params: RouteDecisionProvenanceParams<'_>,
) -> RouteDecisionProvenance {
    let RouteDecisionProvenanceParams {
        decided_at_ms,
        session_binding,
        session_override_config,
        global_config_override,
        override_model,
        override_effort,
        override_service_tier,
        request_model,
        effective_effort,
        effective_service_tier,
        target,
        provider_id,
    } = params;

    let mut effective_model = resolve_request_field_provenance(
        request_model,
        override_model,
        session_binding.and_then(|binding| binding.model.as_deref()),
    );
    if let Some(current) = effective_model.as_mut() {
        let mapped = model_routing::effective_model(
            &target.upstream().model_mapping,
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
        effective_station: resolve_station_provenance(
            target.compatibility_station_name(),
            session_override_config,
            global_config_override,
            session_binding.and_then(|binding| binding.station_name.as_deref()),
        ),
        effective_upstream_base_url: Some(ResolvedRouteValue::new(
            target.upstream().base_url.clone(),
            RouteValueSource::RuntimeFallback,
        )),
        provider_id: trim_non_empty(provider_id)
            .or_else(|| target.provider_id().map(ToOwned::to_owned)),
        endpoint_id: target.endpoint_id(),
        route_path: target.route_path(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SessionBinding};
    use super::{RouteDecisionProvenanceParams, build_route_decision_provenance};
    use crate::config::{UpstreamAuth, UpstreamConfig};
    use crate::lb::SelectedUpstream;
    use crate::proxy::attempt_target::AttemptTarget;
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
        let target = AttemptTarget::legacy(selected);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 42,
            session_binding: Some(&binding),
            session_override_config: Some("session-station"),
            global_config_override: Some("global-station"),
            override_model: Some("override-model"),
            override_effort: Some("medium"),
            override_service_tier: Some("flex"),
            request_model: Some("request-model"),
            effective_effort: Some("request-effort"),
            effective_service_tier: Some("request-tier"),
            target: &target,
            provider_id: Some(" provider-1 "),
        });

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
        assert_eq!(decision.endpoint_id.as_deref(), Some("0"));
        assert_eq!(
            decision.route_path,
            vec!["legacy", "selected-station", "selected-station#0"]
        );
    }

    #[test]
    fn route_provenance_uses_binding_and_runtime_fallback_when_overrides_absent() {
        let binding = make_binding();
        let selected = make_selected_upstream(&[]);
        let target = AttemptTarget::legacy(selected);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 7,
            session_binding: Some(&binding),
            session_override_config: Some("   "),
            global_config_override: None,
            override_model: None,
            override_effort: None,
            override_service_tier: None,
            request_model: Some("request-model"),
            effective_effort: Some("request-effort"),
            effective_service_tier: Some("request-tier"),
            target: &target,
            provider_id: Some(""),
        });

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
        let target = AttemptTarget::legacy(selected);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 9,
            session_binding: None,
            session_override_config: None,
            global_config_override: None,
            override_model: None,
            override_effort: None,
            override_service_tier: None,
            request_model: Some("gpt-5"),
            effective_effort: None,
            effective_service_tier: None,
            target: &target,
            provider_id: None,
        });

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
        let target = AttemptTarget::legacy(selected);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 11,
            session_binding: None,
            session_override_config: None,
            global_config_override: Some("global-station"),
            override_model: None,
            override_effort: None,
            override_service_tier: None,
            request_model: Some("request-model"),
            effective_effort: Some("low"),
            effective_service_tier: Some("priority"),
            target: &target,
            provider_id: None,
        });

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
                endpoint_id: Some("0".to_string()),
                route_path: vec![
                    "legacy".to_string(),
                    "selected-station".to_string(),
                    "selected-station#0".to_string(),
                ],
            }
        );
    }
}

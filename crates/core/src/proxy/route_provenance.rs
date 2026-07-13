use crate::state::{
    ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SessionBinding,
    SessionContinuityMode,
};

use crate::routing_ir::CapturedRouteCandidate;

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_request_field_provenance(
    request_value: Option<&str>,
    binding_value: Option<&str>,
) -> Option<ResolvedRouteValue> {
    if let Some(value) = trim_non_empty(binding_value) {
        return Some(ResolvedRouteValue::new(
            value,
            RouteValueSource::ProfileDefault,
        ));
    }
    trim_non_empty(request_value)
        .map(|value| ResolvedRouteValue::new(value, RouteValueSource::RequestPayload))
}

fn binding_for_request_field_provenance(
    binding: Option<&SessionBinding>,
) -> Option<&SessionBinding> {
    let binding = binding?;
    if binding.continuity_mode != SessionContinuityMode::ManualProfile {
        return None;
    }
    Some(binding)
}

pub(super) struct RouteDecisionProvenanceParams<'a> {
    pub(super) decided_at_ms: u64,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) request_model: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) provider_id: Option<&'a str>,
}

pub(super) fn build_route_decision_provenance(
    params: RouteDecisionProvenanceParams<'_>,
) -> RouteDecisionProvenance {
    let RouteDecisionProvenanceParams {
        decided_at_ms,
        session_binding,
        request_model,
        effective_effort,
        effective_service_tier,
        target,
        provider_id,
    } = params;

    let request_field_binding = binding_for_request_field_provenance(session_binding);
    let mut effective_model = resolve_request_field_provenance(
        request_model,
        request_field_binding.and_then(|binding| binding.model.as_deref()),
    );
    if let Some(current) = effective_model.as_mut() {
        let mapped = target.effective_model(current.value.as_str());
        if mapped != current.value {
            *current = ResolvedRouteValue::new(mapped, RouteValueSource::ProviderMapping);
        }
    }

    RouteDecisionProvenance {
        decided_at_ms,
        binding_profile_name: session_binding.and_then(|binding| binding.profile_name.clone()),
        binding_continuity_mode: session_binding.map(|binding| binding.continuity_mode),
        effective_model,
        effective_reasoning_effort: resolve_request_field_provenance(
            effective_effort,
            request_field_binding.and_then(|binding| binding.reasoning_effort.as_deref()),
        ),
        effective_service_tier: resolve_request_field_provenance(
            effective_service_tier,
            request_field_binding.and_then(|binding| binding.service_tier.as_deref()),
        ),
        effective_upstream_base_url: Some(ResolvedRouteValue::new(
            target.base_url().to_owned(),
            RouteValueSource::RuntimeFallback,
        )),
        provider_id: trim_non_empty(provider_id).or_else(|| Some(target.provider_id().to_owned())),
        endpoint_id: Some(target.endpoint_id().to_owned()),
        route_path: target.route_path().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource, SessionBinding};
    use super::{RouteDecisionProvenanceParams, build_route_decision_provenance};
    use crate::config::UpstreamAuth;
    use crate::routing_ir::CapturedRouteCandidate;
    use crate::routing_ir::{RouteCandidate, RouteCandidateConcurrency};
    use crate::state::SessionContinuityMode;

    fn make_binding() -> SessionBinding {
        SessionBinding {
            session_id: "session-1".to_string(),
            profile_name: Some("default".to_string()),
            model: Some("binding-model".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_seen_ms: 3,
        }
    }

    fn make_target(model_mapping: &[(&str, &str)]) -> CapturedRouteCandidate {
        let candidate = RouteCandidate {
            provider_id: "test-provider".to_string(),
            provider_alias: None,
            endpoint_id: "default".to_string(),
            base_url: "https://example.com/v1".to_string(),
            continuity_domain: None,
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: model_mapping
                .iter()
                .map(|(from, to)| (from.to_string(), to.to_string()))
                .collect(),
            model_rules: std::sync::Arc::new(
                crate::model_routing::CompiledModelRules::compile(
                    [],
                    model_mapping
                        .iter()
                        .map(|(from, to)| (from.to_string(), to.to_string())),
                )
                .expect("compile test model rules"),
            ),
            route_path: vec!["root".to_string(), "test-provider".to_string()],
            preference_group: 0,
            stable_index: 0,
            concurrency: RouteCandidateConcurrency::default(),
        };
        CapturedRouteCandidate::capture_for_service("codex", &candidate)
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
    fn route_provenance_uses_manual_profile_fields() {
        let binding = make_binding();
        let target = make_target(&[]);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 42,
            session_binding: Some(&binding),
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
            decision.effective_upstream_base_url,
            "https://example.com/v1",
            RouteValueSource::RuntimeFallback,
        );
        assert_eq!(decision.provider_id.as_deref(), Some("provider-1"));
        assert_eq!(decision.endpoint_id.as_deref(), Some("default"));
        assert_eq!(decision.route_path, vec!["root", "test-provider"]);
    }

    #[test]
    fn route_provenance_uses_binding_and_runtime_fallback() {
        let binding = make_binding();
        let target = make_target(&[]);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 7,
            session_binding: Some(&binding),
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
        assert_eq!(decision.provider_id.as_deref(), Some("test-provider"));
    }

    #[test]
    fn route_provenance_ignores_default_profile_for_request_fields() {
        let mut binding = make_binding();
        binding.continuity_mode = SessionContinuityMode::DefaultProfile;
        let target = make_target(&[]);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 8,
            session_binding: Some(&binding),
            request_model: Some("request-model"),
            effective_effort: Some("request-effort"),
            effective_service_tier: Some("request-tier"),
            target: &target,
            provider_id: None,
        });

        assert_route_value(
            decision.effective_model,
            "request-model",
            RouteValueSource::RequestPayload,
        );
        assert_route_value(
            decision.effective_reasoning_effort,
            "request-effort",
            RouteValueSource::RequestPayload,
        );
        assert_route_value(
            decision.effective_service_tier,
            "request-tier",
            RouteValueSource::RequestPayload,
        );
    }

    #[test]
    fn route_provenance_marks_provider_mapping_when_model_is_remapped() {
        let target = make_target(&[("gpt-5", "provider-model")]);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 9,
            session_binding: None,
            request_model: Some("gpt-5"),
            effective_effort: None,
            effective_service_tier: None,
            target: &target,
            provider_id: None,
        });

        assert_route_value(
            decision.effective_model,
            "provider-model",
            RouteValueSource::ProviderMapping,
        );
    }

    #[test]
    fn route_provenance_keeps_request_payload_when_no_binding_exists() {
        let target = make_target(&[]);

        let decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
            decided_at_ms: 11,
            session_binding: None,
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
                effective_upstream_base_url: Some(ResolvedRouteValue {
                    value: "https://example.com/v1".to_string(),
                    source: RouteValueSource::RuntimeFallback,
                }),
                provider_id: Some("test-provider".to_string()),
                endpoint_id: Some("default".to_string()),
                route_path: vec!["root".to_string(), "test-provider".to_string()],
            }
        );
    }
}

use axum::body::Bytes;

use crate::logging::BodyPreview;
use crate::logging::now_ms;
use crate::provider_catalog::ProviderModelRequestContract;
use crate::state::{RouteDecisionProvenance, SessionBinding};

use super::ProxyService;
use super::request_body::{
    DeferredReasoningIntentError, ReasoningOrchestrationIntent, RequestDialect,
    apply_deferred_reasoning_intent, apply_model_override_value,
};
use super::request_preparation::build_body_previews;
use super::route_provenance::{RouteDecisionProvenanceParams, build_route_decision_provenance};
use crate::routing_ir::CapturedRouteCandidate;

pub(super) struct SelectedUpstreamRequestSetup {
    pub(super) model_note: String,
    pub(super) provider_id: Option<String>,
    pub(super) route_decision: RouteDecisionProvenance,
    pub(super) filtered_body: Bytes,
    pub(super) effective_effort: Option<String>,
    pub(super) upstream_request_body_len: usize,
    pub(super) upstream_request_body_debug: Option<BodyPreview>,
    pub(super) upstream_request_body_warn: Option<BodyPreview>,
}

pub(super) struct SelectedUpstreamRequestSetupParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) model_mapping: SelectedModelMapping,
    pub(super) request_contract: Option<&'a ProviderModelRequestContract>,
    pub(super) request_dialect: RequestDialect,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) client_content_type: Option<&'a str>,
    pub(super) request_body_previews: bool,
    pub(super) apply_request_filter: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
}

pub(super) fn prepare_selected_upstream_request(
    params: SelectedUpstreamRequestSetupParams<'_>,
) -> Result<SelectedUpstreamRequestSetup, DeferredReasoningIntentError> {
    let SelectedUpstreamRequestSetupParams {
        proxy,
        target,
        model_mapping,
        request_contract,
        request_dialect,
        request_model,
        session_binding,
        effective_effort,
        deferred_reasoning_intent,
        effective_service_tier,
        client_content_type,
        request_body_previews,
        apply_request_filter,
        debug_max,
        warn_max,
    } = params;

    let SelectedModelMapping {
        model_note,
        effective_model: _,
        body: mut body_for_selected,
    } = model_mapping;
    let selected_effective_effort = if let Some(intent) = deferred_reasoning_intent {
        body_for_selected = apply_deferred_reasoning_intent(
            &body_for_selected,
            request_dialect,
            intent,
            request_contract,
        )?;
        Some("max".to_string())
    } else {
        effective_effort.map(ToOwned::to_owned)
    };
    let provider_id = Some(target.provider_id().to_owned());
    let route_decision = build_route_decision_provenance(RouteDecisionProvenanceParams {
        decided_at_ms: now_ms(),
        session_binding,
        request_model,
        effective_effort: selected_effective_effort.as_deref(),
        effective_service_tier,
        target,
        provider_id: provider_id.as_deref(),
    });

    let filtered_body = if apply_request_filter {
        proxy.filter.apply_bytes(body_for_selected)
    } else {
        body_for_selected
    };
    let upstream_request_body_len = filtered_body.len();
    let upstream_body_previews = build_body_previews(
        &filtered_body,
        client_content_type,
        request_body_previews,
        debug_max,
        warn_max,
    );

    Ok(SelectedUpstreamRequestSetup {
        model_note,
        provider_id,
        route_decision,
        filtered_body,
        effective_effort: selected_effective_effort,
        upstream_request_body_len,
        upstream_request_body_debug: upstream_body_previews.debug,
        upstream_request_body_warn: upstream_body_previews.warn,
    })
}

pub(super) struct SelectedModelMapping {
    pub(super) model_note: String,
    pub(super) effective_model: Option<String>,
    pub(super) body: Bytes,
}

pub(super) fn apply_selected_model_mapping(
    target: &CapturedRouteCandidate,
    body_for_upstream: &Bytes,
    request_model: Option<&str>,
) -> SelectedModelMapping {
    let Some(requested_model) = request_model else {
        return SelectedModelMapping {
            model_note: "-".to_string(),
            effective_model: None,
            body: body_for_upstream.clone(),
        };
    };

    let effective_model = target.effective_model(requested_model);
    if effective_model != requested_model {
        let body = serde_json::from_slice::<serde_json::Value>(body_for_upstream.as_ref())
            .ok()
            .and_then(|mut value| {
                value.as_object_mut()?;
                apply_model_override_value(&mut value, effective_model.as_str());
                serde_json::to_vec(&value).ok()
            })
            .map(Bytes::from)
            .unwrap_or_else(|| body_for_upstream.clone());
        return SelectedModelMapping {
            model_note: format!("{requested_model}->{effective_model}"),
            effective_model: Some(effective_model),
            body,
        };
    }

    SelectedModelMapping {
        model_note: requested_model.to_string(),
        effective_model: Some(effective_model),
        body: body_for_upstream.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::config::{
        HelperConfig, ProviderConfig, RouteGraphConfig, ServiceRouteConfig, UpstreamAuth,
    };
    use crate::provider_catalog::{
        AccountFingerprint, ProviderAdapter, ProviderCatalogEpoch, ProviderCatalogScope,
        ProviderModelRequestContract,
    };
    use crate::routing_ir::{RouteCandidate, RouteCandidateConcurrency};
    use crate::state::SessionContinuityMode;

    fn test_proxy_service() -> ProxyService {
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "test-provider".to_string(),
                    ProviderConfig {
                        base_url: Some("https://example.com/v1".to_string()),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "test-provider".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        ProxyService::new(reqwest::Client::new(), Arc::new(source), "codex")
    }

    fn test_binding() -> SessionBinding {
        SessionBinding {
            session_id: "session-1".to_string(),
            profile_name: Some("default".to_string()),
            model: Some("gpt-5".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            continuity_mode: SessionContinuityMode::DefaultProfile,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_seen_ms: 3,
        }
    }

    fn test_route_candidate() -> RouteCandidate {
        RouteCandidate {
            provider_id: "test-provider".to_string(),
            provider_alias: None,
            endpoint_id: "default".to_string(),
            base_url: "https://example.com/v1".to_string(),
            continuity_domain: None,
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::from([("gpt-5".to_string(), "gpt-5.4".to_string())]),
            model_rules: std::sync::Arc::new(
                crate::model_routing::CompiledModelRules::compile(
                    [],
                    [("gpt-5".to_string(), "gpt-5.4".to_string())],
                )
                .expect("compile test model rules"),
            ),
            route_path: vec![
                "provider:test-provider".to_string(),
                "endpoint:default".to_string(),
            ],
            preference_group: 0,
            stable_index: 0,
            concurrency: RouteCandidateConcurrency::default(),
        }
    }

    fn official_codex_route_candidate() -> RouteCandidate {
        RouteCandidate {
            base_url: "https://api.openai.com/v1".to_string(),
            ..test_route_candidate()
        }
    }

    fn official_codex_route_candidate_with_mapping(
        requested: &str,
        effective: &str,
    ) -> RouteCandidate {
        let mapping = BTreeMap::from([(requested.to_string(), effective.to_string())]);
        RouteCandidate {
            model_mapping: mapping.clone(),
            model_rules: Arc::new(
                crate::model_routing::CompiledModelRules::compile([], mapping)
                    .expect("compile mapped official model rules"),
            ),
            ..official_codex_route_candidate()
        }
    }

    fn request_contract(model: &str) -> ProviderModelRequestContract {
        let scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "codex/test-provider/default",
            AccountFingerprint::from_digest([9; 32]),
            "test-runtime",
        )
        .expect("provider scope");
        ProviderCatalogEpoch::bundled_openai_codex(scope)
            .expect("provider epoch")
            .capture_model_request_contract(model)
            .expect("provider request contract")
    }

    #[tokio::test]
    async fn prepare_selected_upstream_request_applies_mapping_and_route_provenance() {
        let proxy = test_proxy_service();
        let target = CapturedRouteCandidate::capture_for_service("codex", &test_route_candidate());
        let body = Bytes::from_static(br#"{"model":"gpt-5"}"#);
        let binding = test_binding();
        let model_mapping = apply_selected_model_mapping(&target, &body, Some("gpt-5"));

        let setup = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy: &proxy,
            target: &target,
            model_mapping,
            request_contract: None,
            request_dialect: RequestDialect::ResponsesHttp,
            request_model: Some("gpt-5"),
            session_binding: Some(&binding),
            effective_effort: Some("medium"),
            deferred_reasoning_intent: None,
            effective_service_tier: Some("priority"),
            client_content_type: Some("application/json"),
            request_body_previews: true,
            apply_request_filter: true,
            debug_max: 128,
            warn_max: 64,
        })
        .expect("selected request setup");

        assert_eq!(setup.model_note, "gpt-5->gpt-5.4");
        assert_eq!(setup.provider_id.as_deref(), Some("test-provider"));
        assert_eq!(
            setup.route_decision.provider_id.as_deref(),
            Some("test-provider")
        );
        assert_eq!(setup.route_decision.endpoint_id.as_deref(), Some("default"));
        assert_eq!(
            setup.route_decision.route_path,
            vec!["provider:test-provider", "endpoint:default"]
        );
        assert_eq!(setup.upstream_request_body_len, setup.filtered_body.len());
        assert!(setup.upstream_request_body_debug.is_some());
        assert!(String::from_utf8_lossy(setup.filtered_body.as_ref()).contains("gpt-5.4"));
        assert_eq!(setup.effective_effort.as_deref(), Some("medium"));
    }

    #[tokio::test]
    async fn selected_target_rejects_ultra_without_a_captured_provider_contract() {
        let proxy = test_proxy_service();
        let target = CapturedRouteCandidate::capture_for_service("codex", &test_route_candidate());
        let body = Bytes::from_static(br#"{"model":"gpt-5.6-sol","reasoning":{"mode":"pro"}}"#);
        let model_mapping = apply_selected_model_mapping(&target, &body, Some("gpt-5.6-sol"));

        let result = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy: &proxy,
            target: &target,
            model_mapping,
            request_contract: None,
            request_dialect: RequestDialect::ResponsesHttp,
            request_model: Some("gpt-5.6-sol"),
            session_binding: None,
            effective_effort: None,
            deferred_reasoning_intent: Some(ReasoningOrchestrationIntent::Ultra),
            effective_service_tier: None,
            client_content_type: Some("application/json"),
            request_body_previews: false,
            apply_request_filter: true,
            debug_max: 0,
            warn_max: 0,
        });
        let error = match result {
            Ok(_) => panic!("missing provider contract must reject ultra"),
            Err(error) => error,
        };

        assert_eq!(error, DeferredReasoningIntentError::MissingCapturedContract);
    }

    #[tokio::test]
    async fn selected_official_catalog_target_maps_ultra_to_max() {
        let proxy = test_proxy_service();
        let target =
            CapturedRouteCandidate::capture_for_service("codex", &official_codex_route_candidate());
        let body = Bytes::from_static(
            br#"{"model":"gpt-5.6-sol","reasoning":{"mode":"pro","future_mode":"deliberate"}}"#,
        );
        let model_mapping = apply_selected_model_mapping(&target, &body, Some("gpt-5.6-sol"));
        let contract = request_contract("gpt-5.6-sol");

        let setup = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy: &proxy,
            target: &target,
            model_mapping,
            request_contract: Some(&contract),
            request_dialect: RequestDialect::ResponsesHttp,
            request_model: Some("gpt-5.6-sol"),
            session_binding: None,
            effective_effort: None,
            deferred_reasoning_intent: Some(ReasoningOrchestrationIntent::Ultra),
            effective_service_tier: None,
            client_content_type: Some("application/json"),
            request_body_previews: false,
            apply_request_filter: true,
            debug_max: 0,
            warn_max: 0,
        })
        .expect("captured provider contract should resolve ultra");

        let value: serde_json::Value =
            serde_json::from_slice(setup.filtered_body.as_ref()).expect("json body");
        assert_eq!(value["reasoning"]["effort"].as_str(), Some("max"));
        assert_eq!(value["reasoning"]["mode"].as_str(), Some("pro"));
        assert_eq!(
            value["reasoning"]["future_mode"].as_str(),
            Some("deliberate")
        );
        assert_eq!(setup.effective_effort.as_deref(), Some("max"));
    }

    #[tokio::test]
    async fn selected_mapped_luna_contract_rejects_ultra() {
        let proxy = test_proxy_service();
        let candidate = official_codex_route_candidate_with_mapping("gpt-5.6-fast", "gpt-5.6-luna");
        let target = CapturedRouteCandidate::capture_for_service("codex", &candidate);
        let body = Bytes::from_static(
            br#"{"model":"gpt-5.6-fast","reasoning":{"mode":"pro","future_mode":"deliberate"}}"#,
        );
        let model_mapping = apply_selected_model_mapping(&target, &body, Some("gpt-5.6-fast"));
        assert_eq!(
            model_mapping.effective_model.as_deref(),
            Some("gpt-5.6-luna")
        );
        let contract = request_contract("gpt-5.6-luna");

        let result = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy: &proxy,
            target: &target,
            model_mapping,
            request_contract: Some(&contract),
            request_dialect: RequestDialect::ResponsesHttp,
            request_model: Some("gpt-5.6-fast"),
            session_binding: None,
            effective_effort: None,
            deferred_reasoning_intent: Some(ReasoningOrchestrationIntent::Ultra),
            effective_service_tier: None,
            client_content_type: Some("application/json"),
            request_body_previews: false,
            apply_request_filter: true,
            debug_max: 0,
            warn_max: 0,
        });

        assert!(matches!(
            result,
            Err(DeferredReasoningIntentError::UnsupportedUltra)
        ));
    }

    #[test]
    fn apply_selected_model_mapping_keeps_original_when_mapping_missing() {
        let target = CapturedRouteCandidate::capture_for_service("codex", &test_route_candidate());
        let body = Bytes::from_static(br#"{"model":"gpt-4.1"}"#);

        let mapping = apply_selected_model_mapping(&target, &body, Some("gpt-4.1"));

        assert_eq!(mapping.model_note, "gpt-4.1");
        assert_eq!(mapping.effective_model.as_deref(), Some("gpt-4.1"));
        assert_eq!(mapping.body, body);
    }
}

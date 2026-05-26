use axum::http::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::codex_capability_profile::{
    CodexCapabilityDecision, CodexCapabilityProfile, CodexCapabilityProfileInput,
    CodexCapabilitySupport, CodexModelCatalogProfile, CodexPatchModeRecommendation,
    CodexPatchModeRecommendationInput,
};
use crate::codex_integration::CodexPatchMode;
use crate::config::{ServiceViewV4, effective_v4_routing};
use crate::routing_ir::compile_v4_route_plan_template_for_compat_runtime;
use crate::runtime_identity::ContinuityDomainKey;

use super::codex_relay_probe::CodexRelayProbeObservation;
use super::codex_relay_probe::codex_relay_probe_cases;
use super::codex_relay_target::{
    CodexRelayTargetSelection, SelectedCodexRelayTarget, select_codex_relay_target,
};
use super::models_compat::maybe_decode_models_response_body;
use super::{
    CodexRelayProbeClient, CodexRelayProbeKind, CodexRelayProbeResult, ProxyControlError,
    ProxyService,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesRequest {
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub upstream_index: Option<usize>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, alias = "patch_preset")]
    pub patch_mode: Option<CodexPatchMode>,
    #[serde(default)]
    pub responses_websocket: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesResponse {
    pub api_version: u32,
    pub service_name: String,
    pub station_name: String,
    pub upstream_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    pub upstream_base_url: String,
    pub patch_mode: CodexPatchMode,
    pub responses_websocket: bool,
    pub model: Option<String>,
    pub expected: CodexCapabilityProfile,
    pub observed: CodexRelayCapabilitiesObserved,
    pub recommendation: CodexPatchModeRecommendation,
    #[serde(default)]
    pub continuity: CodexRelayContinuityDiagnostics,
    pub mismatches: Vec<CodexRelayCapabilityMismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesObserved {
    pub models: CodexRelayProbeResult,
    pub responses: CodexRelayProbeResult,
    pub responses_compact: CodexRelayProbeResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayCapabilityMismatch {
    pub capability: String,
    pub expected: String,
    pub observed: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayContinuityDiagnostics {
    pub selected_domain: CodexRelayContinuityDomainSummary,
    pub same_domain_endpoint_count: usize,
    pub configured_endpoint_count: usize,
    pub affinity_policy: Option<String>,
    pub can_state_bound_failover_within_domain: bool,
    pub warnings: Vec<String>,
    pub recommendations: Vec<String>,
}

impl Default for CodexRelayContinuityDiagnostics {
    fn default() -> Self {
        Self {
            selected_domain: CodexRelayContinuityDomainSummary::default(),
            same_domain_endpoint_count: 1,
            configured_endpoint_count: 1,
            affinity_policy: None,
            can_state_bound_failover_within_domain: false,
            warnings: Vec::new(),
            recommendations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayContinuityDomainSummary {
    pub key: String,
    pub explicit: bool,
}

impl Default for CodexRelayContinuityDomainSummary {
    fn default() -> Self {
        Self {
            key: "unknown".to_string(),
            explicit: false,
        }
    }
}

pub(super) async fn codex_relay_capabilities_for_proxy(
    proxy: &ProxyService,
    payload: CodexRelayCapabilitiesRequest,
) -> Result<CodexRelayCapabilitiesResponse, ProxyControlError> {
    if proxy.service_name != "codex" {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            "Codex relay capabilities are only available for the codex service",
        ));
    }

    let cfg = proxy.config.snapshot().await;
    let mgr = proxy.service_manager(cfg.as_ref());
    let target = select_codex_relay_target(
        mgr,
        CodexRelayTargetSelection {
            station_name: payload.station_name.as_deref(),
            upstream_index: payload.upstream_index,
            provider_id: payload.provider_id.as_deref(),
            endpoint_id: payload.endpoint_id.as_deref(),
        },
    )?;
    let patch_mode = payload
        .patch_mode
        .or_else(current_codex_switch_patch_mode)
        .or_else(|| {
            crate::config::codex_client_patch_config_from_config_file()
                .ok()
                .map(|cfg| cfg.preset)
        })
        .unwrap_or_default();
    let responses_websocket = payload
        .responses_websocket
        .or_else(current_codex_switch_responses_websocket)
        .or_else(|| {
            crate::config::codex_client_patch_config_from_config_file()
                .ok()
                .map(|cfg| cfg.options.responses_websocket)
        })
        .unwrap_or(false);
    let model = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let probe_client = CodexRelayProbeClient::new(proxy.client.clone());
    let observations = run_capability_probe_cases(&probe_client, &target.upstream).await;
    let models_observation = observation_for_kind(&observations, CodexRelayProbeKind::Models);

    let expected = build_expected_profile(
        patch_mode,
        responses_websocket,
        model.as_deref(),
        models_observation,
    );
    let observed = build_observed_from_probe_observations(&observations);
    let recommendation = build_recommendation(patch_mode, &expected, &observed);
    let mismatches = build_mismatches(&expected, &observed);
    let v4_source = proxy.config.v4_snapshot().await;
    let continuity = build_continuity_diagnostics(
        proxy.service_name,
        v4_source.as_deref().map(|cfg| &cfg.codex),
        &target,
        patch_mode,
        responses_websocket,
    );

    let response = CodexRelayCapabilitiesResponse {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        station_name: target.station_name,
        upstream_index: target.upstream_index,
        provider_id: target.provider_id,
        endpoint_id: target.endpoint_id,
        provider_endpoint_key: target.provider_endpoint_key,
        upstream_base_url: target.upstream.base_url,
        patch_mode,
        responses_websocket,
        model,
        expected,
        observed,
        recommendation,
        continuity,
        mismatches,
    };
    if let Err(error) = super::codex_relay_evidence::append_codex_relay_capabilities_evidence(
        &response,
        "proxy_service",
    ) {
        tracing::warn!("failed to write Codex relay capability evidence: {}", error);
    }
    Ok(response)
}

async fn run_capability_probe_cases(
    probe_client: &CodexRelayProbeClient,
    upstream: &crate::config::UpstreamConfig,
) -> Vec<CodexRelayProbeObservation> {
    let mut observations = Vec::with_capacity(codex_relay_probe_cases().len());
    for case in codex_relay_probe_cases() {
        observations.push(
            probe_client
                .probe_upstream_observation(upstream, &case.spec())
                .await,
        );
    }
    observations
}

fn build_observed_from_probe_observations(
    observations: &[CodexRelayProbeObservation],
) -> CodexRelayCapabilitiesObserved {
    CodexRelayCapabilitiesObserved {
        models: observation_for_kind(observations, CodexRelayProbeKind::Models)
            .result
            .clone(),
        responses: observation_for_kind(observations, CodexRelayProbeKind::Responses)
            .result
            .clone(),
        responses_compact: observation_for_kind(
            observations,
            CodexRelayProbeKind::ResponsesCompact,
        )
        .result
        .clone(),
    }
}

fn observation_for_kind(
    observations: &[CodexRelayProbeObservation],
    kind: CodexRelayProbeKind,
) -> &CodexRelayProbeObservation {
    observations
        .iter()
        .find(|observation| observation.result.kind == kind)
        .expect("Codex relay probe registry must include all observed response fields")
}

fn current_codex_switch_patch_mode() -> Option<CodexPatchMode> {
    crate::codex_integration::codex_switch_status()
        .ok()
        .and_then(|status| status.patch_mode)
}

fn current_codex_switch_responses_websocket() -> Option<bool> {
    crate::codex_integration::codex_switch_status()
        .ok()
        .and_then(|status| status.supports_websockets)
}

fn build_expected_profile(
    patch_mode: CodexPatchMode,
    responses_websocket: bool,
    model: Option<&str>,
    models_observation: &CodexRelayProbeObservation,
) -> CodexCapabilityProfile {
    let model_catalog = translated_models_catalog(models_observation, model).unwrap_or_else(|| {
        CodexModelCatalogProfile::unknown(models_observation.result.reason.clone())
    });
    CodexCapabilityProfile::for_input(CodexCapabilityProfileInput::from_patch_config(
        patch_mode,
        crate::codex_integration::CodexSwitchOptions {
            responses_websocket,
        },
        model_catalog,
    ))
}

fn translated_models_catalog(
    models_observation: &CodexRelayProbeObservation,
    model: Option<&str>,
) -> Option<CodexModelCatalogProfile> {
    let status = models_observation.status?;
    if !status.is_success() {
        return None;
    }
    let body = maybe_decode_models_response_body(
        "codex",
        "/models",
        &models_observation.headers,
        models_observation.body.clone(),
    );
    let value = serde_json::from_slice::<Value>(body.as_ref()).ok()?;
    Some(CodexModelCatalogProfile::from_models_response_json(
        &value, model,
    ))
}

fn build_mismatches(
    expected: &CodexCapabilityProfile,
    observed: &CodexRelayCapabilitiesObserved,
) -> Vec<CodexRelayCapabilityMismatch> {
    let mut out = Vec::new();
    push_endpoint_mismatch(
        &mut out,
        "responses",
        &CodexCapabilityDecision::supported("Codex model requests require a /responses endpoint"),
        &observed.responses,
    );
    push_endpoint_mismatch(
        &mut out,
        "remote_compaction_v1",
        &expected.remote_compaction_v1,
        &observed.responses_compact,
    );
    if observed.models.translation_required {
        out.push(CodexRelayCapabilityMismatch {
            capability: "model_catalog".to_string(),
            expected: "codex_models".to_string(),
            observed: "openai_data_list".to_string(),
            reason: "relay returned an OpenAI models list; helper model translation is disabled by default so Codex can keep using its bundled model metadata. Enable codex.client_patch.translate_models only if you intentionally want helper-synthesized model metadata.".to_string(),
        });
    }
    out
}

fn build_continuity_diagnostics(
    service_name: &str,
    view: Option<&ServiceViewV4>,
    target: &SelectedCodexRelayTarget,
    patch_mode: CodexPatchMode,
    responses_websocket: bool,
) -> CodexRelayContinuityDiagnostics {
    let fallback_domain = target
        .provider_endpoint_key
        .as_deref()
        .map(|key| format!("provider_endpoint:{key}"))
        .unwrap_or_else(|| {
            format!(
                "legacy:{}/{}/{}",
                service_name, target.station_name, target.upstream_index
            )
        });
    let mut selected_domain = CodexRelayContinuityDomainSummary {
        key: fallback_domain,
        explicit: false,
    };
    let mut same_domain_endpoint_count = 1usize;
    let mut configured_endpoint_count = 1usize;
    let mut affinity_policy = None;

    if let Some(view) = view {
        let routing = effective_v4_routing(view);
        affinity_policy = Some(routing_affinity_policy_label(routing.affinity_policy).to_string());
        if let Ok(template) = compile_v4_route_plan_template_for_compat_runtime(service_name, view)
        {
            let topology = template.continuity_topology();
            configured_endpoint_count = topology.configured_provider_endpoint_count().max(1);
            if let Some(provider_endpoint_key) = target.provider_endpoint_key.as_deref()
                && let Some(summary) = topology.selected_domain_summary(provider_endpoint_key)
            {
                selected_domain = domain_summary(&summary.domain);
                same_domain_endpoint_count = summary.same_domain_endpoint_count;
            }
        }
    }

    let official_relay = patch_mode.enables_official_relay_features();
    let can_state_bound_failover_within_domain =
        selected_domain.explicit && same_domain_endpoint_count > 1;
    let mut warnings = Vec::new();
    let mut recommendations = Vec::new();

    if official_relay && !selected_domain.explicit && configured_endpoint_count > 1 {
        warnings.push(
            "official relay preset is active with multiple configured provider endpoints, but the selected endpoint has no explicit continuity_domain".to_string(),
        );
        recommendations.push(
            "Set the same continuity_domain only on provider endpoints that intentionally share encrypted response state; otherwise keep provider-endpoint isolation.".to_string(),
        );
    }

    if can_state_bound_failover_within_domain {
        recommendations.push(format!(
            "State-bound compact may fail over across {same_domain_endpoint_count} endpoints in explicit continuity domain {}.",
            selected_domain.key
        ));
    } else {
        recommendations.push(
            "State-bound compact stays isolated to the selected provider endpoint unless a shared continuity_domain is configured.".to_string(),
        );
    }

    if matches!(
        affinity_policy.as_deref(),
        Some("preferred-group") | Some("off")
    ) && official_relay
        && configured_endpoint_count > 1
    {
        warnings.push(
            "official relay presets with multiple provider endpoints should use fallback-sticky or hard affinity when encrypted compact state matters".to_string(),
        );
    }

    if responses_websocket && !selected_domain.explicit && configured_endpoint_count > 1 {
        warnings.push(
            "Responses WebSocket compact uses the same state-bound continuity rules; do not enable cross-provider continuity without explicit continuity_domain".to_string(),
        );
    }

    CodexRelayContinuityDiagnostics {
        selected_domain,
        same_domain_endpoint_count,
        configured_endpoint_count,
        affinity_policy,
        can_state_bound_failover_within_domain,
        warnings,
        recommendations,
    }
}

fn domain_summary(domain: &ContinuityDomainKey) -> CodexRelayContinuityDomainSummary {
    CodexRelayContinuityDomainSummary {
        key: domain.stable_key(),
        explicit: domain.is_explicit(),
    }
}

fn routing_affinity_policy_label(policy: crate::config::RoutingAffinityPolicyV5) -> &'static str {
    match policy {
        crate::config::RoutingAffinityPolicyV5::Off => "off",
        crate::config::RoutingAffinityPolicyV5::PreferredGroup => "preferred-group",
        crate::config::RoutingAffinityPolicyV5::FallbackSticky => "fallback-sticky",
        crate::config::RoutingAffinityPolicyV5::Hard => "hard",
    }
}

fn build_recommendation(
    current_patch_mode: CodexPatchMode,
    expected: &CodexCapabilityProfile,
    observed: &CodexRelayCapabilitiesObserved,
) -> CodexPatchModeRecommendation {
    CodexPatchModeRecommendation::for_input(CodexPatchModeRecommendationInput {
        current_patch_mode,
        model_catalog: expected.model_catalog.clone(),
        responses: observed_support_to_capability_support(observed.responses.support),
        responses_compact: observed_support_to_capability_support(
            observed.responses_compact.support,
        ),
    })
}

fn observed_support_to_capability_support(
    support: super::CodexRelayProbeSupport,
) -> CodexCapabilitySupport {
    match support {
        super::CodexRelayProbeSupport::Supported => CodexCapabilitySupport::Supported,
        super::CodexRelayProbeSupport::Unsupported => CodexCapabilitySupport::Unsupported,
        super::CodexRelayProbeSupport::Unknown => CodexCapabilitySupport::Unknown,
    }
}

fn push_endpoint_mismatch(
    out: &mut Vec<CodexRelayCapabilityMismatch>,
    capability: &str,
    expected: &CodexCapabilityDecision,
    observed: &CodexRelayProbeResult,
) {
    let expected_label = support_label(expected.support);
    let observed_label = format!(
        "{} via {}",
        probe_support_label(observed.support),
        probe_confidence_label(observed.confidence)
    );
    if expected.support == crate::codex_capability_profile::CodexCapabilitySupport::Supported
        && observed.support != super::CodexRelayProbeSupport::Supported
    {
        out.push(CodexRelayCapabilityMismatch {
            capability: capability.to_string(),
            expected: expected_label.to_string(),
            observed: observed_label,
            reason: observed.reason.clone(),
        });
    }
}

fn support_label(support: crate::codex_capability_profile::CodexCapabilitySupport) -> &'static str {
    match support {
        crate::codex_capability_profile::CodexCapabilitySupport::Unknown => "unknown",
        crate::codex_capability_profile::CodexCapabilitySupport::Supported => "supported",
        crate::codex_capability_profile::CodexCapabilitySupport::Unsupported => "unsupported",
    }
}

fn probe_support_label(support: super::CodexRelayProbeSupport) -> &'static str {
    match support {
        super::CodexRelayProbeSupport::Supported => "supported",
        super::CodexRelayProbeSupport::Unsupported => "unsupported",
        super::CodexRelayProbeSupport::Unknown => "unknown",
    }
}

fn probe_confidence_label(confidence: super::CodexRelayProbeConfidence) -> &'static str {
    match confidence {
        super::CodexRelayProbeConfidence::SuccessStatus => "success_status",
        super::CodexRelayProbeConfidence::EndpointValidation => "endpoint_validation",
        super::CodexRelayProbeConfidence::ErrorClassification => "error_classification",
        super::CodexRelayProbeConfidence::Transport => "transport",
        super::CodexRelayProbeConfidence::Malformed => "malformed",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use reqwest::Client;

    use super::*;
    use crate::config::{
        ProviderConfigV4, ProxyConfigV4, RoutingConfigV4, ServiceViewV4, UpstreamAuth,
    };
    use crate::lb::LbState;

    fn probe_result(
        kind: CodexRelayProbeKind,
        support: super::super::CodexRelayProbeSupport,
    ) -> CodexRelayProbeResult {
        CodexRelayProbeResult {
            kind,
            support,
            confidence: super::super::CodexRelayProbeConfidence::SuccessStatus,
            status_code: Some(200),
            response_shape: Some("ok".to_string()),
            translation_required: false,
            error_class: None,
            reason: "ok".to_string(),
        }
    }

    fn observation(kind: CodexRelayProbeKind) -> CodexRelayProbeObservation {
        CodexRelayProbeObservation {
            result: probe_result(kind, super::super::CodexRelayProbeSupport::Supported),
            status: Some(StatusCode::OK),
            headers: axum::http::HeaderMap::new(),
            body: axum::body::Bytes::new(),
        }
    }

    #[test]
    fn codex_relay_capabilities_observed_shape_is_built_from_probe_registry() {
        let observations = codex_relay_probe_cases()
            .iter()
            .map(|case| observation(case.kind))
            .collect::<Vec<_>>();

        let observed = build_observed_from_probe_observations(&observations);

        assert_eq!(observed.models.kind, CodexRelayProbeKind::Models);
        assert_eq!(observed.responses.kind, CodexRelayProbeKind::Responses);
        assert_eq!(
            observed.responses_compact.kind,
            CodexRelayProbeKind::ResponsesCompact
        );
    }

    #[tokio::test]
    async fn codex_relay_capabilities_targets_route_graph_provider_id() {
        let v4 = ProxyConfigV4 {
            codex: ServiceViewV4 {
                providers: BTreeMap::from([
                    (
                        "input8".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "ciii".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(RoutingConfigV4::ordered_failover(vec![
                    "input8".to_string(),
                    "ciii".to_string(),
                ])),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4 runtime");
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        );

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("ciii".to_string()),
                model: Some("gpt-5.5".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("capabilities response");

        assert_eq!(response.station_name, "routing");
        assert_eq!(response.upstream_index, 1);
        assert_eq!(response.provider_id.as_deref(), Some("ciii"));
        assert_eq!(response.endpoint_id.as_deref(), Some("default"));
        assert_eq!(
            response.provider_endpoint_key.as_deref(),
            Some("codex/ciii/default")
        );
        assert_eq!(response.upstream_base_url, "http://127.0.0.1:10/v1");
    }

    #[tokio::test]
    async fn codex_relay_capabilities_recommends_explicit_continuity_domain_for_multi_relay() {
        let v4 = ProxyConfigV4 {
            codex: ServiceViewV4 {
                providers: BTreeMap::from([
                    (
                        "relay-a".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "relay-b".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(RoutingConfigV4::ordered_failover(vec![
                    "relay-a".to_string(),
                    "relay-b".to_string(),
                ])),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4 runtime");
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        );

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("relay-a".to_string()),
                patch_mode: Some(CodexPatchMode::OfficialRelayBridge),
                ..Default::default()
            },
        )
        .await
        .expect("capabilities response");

        assert_eq!(
            response.continuity.selected_domain.key,
            "provider_endpoint:codex/relay-a/default"
        );
        assert!(!response.continuity.selected_domain.explicit);
        assert_eq!(response.continuity.configured_endpoint_count, 2);
        assert_eq!(response.continuity.same_domain_endpoint_count, 1);
        assert!(!response.continuity.can_state_bound_failover_within_domain);
        assert!(
            response
                .continuity
                .warnings
                .iter()
                .any(|warning| warning.contains("no explicit continuity_domain"))
        );
    }

    #[tokio::test]
    async fn codex_relay_capabilities_does_not_infer_official_openai_domain_from_same_host() {
        let v4 = ProxyConfigV4 {
            codex: ServiceViewV4 {
                providers: BTreeMap::from([
                    (
                        "openai-a".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("https://api.openai.com/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "openai-b".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("https://api.openai.com/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(RoutingConfigV4::ordered_failover(vec![
                    "openai-a".to_string(),
                    "openai-b".to_string(),
                ])),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4 runtime");
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        );

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("openai-a".to_string()),
                patch_mode: Some(CodexPatchMode::OfficialRelayBridge),
                ..Default::default()
            },
        )
        .await
        .expect("capabilities response");

        assert_eq!(
            response.continuity.selected_domain.key,
            "provider_endpoint:codex/openai-a/default"
        );
        assert!(!response.continuity.selected_domain.explicit);
        assert_eq!(response.continuity.same_domain_endpoint_count, 1);
        assert!(!response.continuity.can_state_bound_failover_within_domain);
        assert!(
            response
                .continuity
                .warnings
                .iter()
                .any(|warning| warning.contains("no explicit continuity_domain"))
        );
    }

    #[tokio::test]
    async fn codex_relay_capabilities_reports_shared_explicit_continuity_domain() {
        let v4 = ProxyConfigV4 {
            codex: ServiceViewV4 {
                providers: BTreeMap::from([
                    (
                        "relay-a".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            continuity_domain: Some("relay-cluster".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "relay-b".to_string(),
                        ProviderConfigV4 {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            continuity_domain: Some("relay-cluster".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(RoutingConfigV4::ordered_failover(vec![
                    "relay-a".to_string(),
                    "relay-b".to_string(),
                ])),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4 runtime");
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        );

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("relay-a".to_string()),
                patch_mode: Some(CodexPatchMode::OfficialRelayBridge),
                ..Default::default()
            },
        )
        .await
        .expect("capabilities response");

        assert_eq!(
            response.continuity.selected_domain.key,
            "explicit:codex/relay-cluster"
        );
        assert!(response.continuity.selected_domain.explicit);
        assert_eq!(response.continuity.same_domain_endpoint_count, 2);
        assert!(response.continuity.can_state_bound_failover_within_domain);
        assert!(
            response
                .continuity
                .recommendations
                .iter()
                .any(|recommendation| recommendation.contains("may fail over across 2 endpoints"))
        );
    }
}

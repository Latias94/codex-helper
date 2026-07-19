use axum::http::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::codex_capability_profile::{CodexCapabilityDecision, CodexModelCatalogProfile};
use crate::config::{ServiceRouteConfig, effective_routing};
use crate::provider_catalog::{
    AccountFingerprint, CatalogReasoningEffort, ProviderAdapter, ProviderCatalogEpoch,
    ProviderCatalogScope,
};
use crate::routing_ir::compile_route_handshake_plan;
use crate::runtime_identity::ContinuityDomainKey;

use super::codex_relay_probe::CodexRelayProbeObservation;
use super::codex_relay_probe::codex_relay_probe_cases;
use super::codex_relay_target::{
    CodexRelayTargetSelection, SelectedCodexRelayTarget, select_codex_relay_target,
};
use super::models_compat::{ModelsTranslationScope, maybe_decode_models_response_body};
use super::{
    CodexRelayProbeClient, CodexRelayProbeKind, CodexRelayProbeResult, ProxyControlError,
    ProxyService,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexRelayCapabilitiesRequest {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesResponse {
    pub api_version: u32,
    pub service_name: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub provider_endpoint_key: String,
    pub model: Option<String>,
    pub expected: CodexRelayProviderContract,
    pub observed: CodexRelayCapabilitiesObserved,
    #[serde(default)]
    pub continuity: CodexRelayContinuityDiagnostics,
    pub mismatches: Vec<CodexRelayCapabilityMismatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayProviderContract {
    pub provider_adapter: ProviderAdapter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<String>,
    pub request_dialects: Vec<String>,
    pub model_catalog: CodexModelCatalogProfile,
    pub responses: CodexCapabilityDecision,
    pub remote_compaction_v1: CodexCapabilityDecision,
    pub hosted_image_generation: CodexCapabilityDecision,
    pub responses_websocket: CodexCapabilityDecision,
    pub ultra_maps_to_max: CodexCapabilityDecision,
    pub web_search: CodexCapabilityDecision,
    pub apply_patch: CodexCapabilityDecision,
    pub reasoning_summaries: CodexCapabilityDecision,
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

    let runtime_snapshot = proxy.config.capture().await;
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| {
            ProxyControlError::new(StatusCode::BAD_REQUEST, "no codex route graph is available")
        })?;
    let target = select_codex_relay_target(
        graph.as_ref(),
        CodexRelayTargetSelection {
            provider_id: payload.provider_id.as_deref(),
            endpoint_id: payload.endpoint_id.as_deref(),
        },
    )?;
    let model = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let credential = runtime_snapshot
        .credential_generation()
        .capture_bound(&target.provider_endpoint)
        .map_err(|_| {
            ProxyControlError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "selected Codex relay target has no captured credential binding",
            )
        })?;
    let probe_client = CodexRelayProbeClient::new(proxy.client.clone(), credential);
    let credential_readiness = probe_client.credential_readiness(&target.upstream);
    let observations =
        run_capability_probe_cases(&probe_client, &target.upstream, credential_readiness).await;
    let models_observation = observation_for_kind(&observations, CodexRelayProbeKind::Models);

    let expected = build_expected_provider_contract(
        runtime_snapshot.as_ref(),
        &target,
        model.as_deref(),
        models_observation,
    )?;
    let observed = build_observed_from_probe_observations(&observations);
    let config_source = runtime_snapshot.config();
    let configured_translate_models = config_source
        .codex
        .client_patch
        .unwrap_or_default()
        .translate_models;
    let mismatches = build_mismatches(&expected, &observed, configured_translate_models);
    let continuity = build_continuity_diagnostics(
        proxy.service_name,
        Some(&config_source.codex),
        &target,
        &expected,
        &observed,
    );
    let provider_endpoint_key = target.provider_endpoint.stable_key();
    let provider_id = target.provider_endpoint.provider_id;
    let endpoint_id = target.provider_endpoint.endpoint_id;

    let response = CodexRelayCapabilitiesResponse {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        provider_id,
        endpoint_id,
        provider_endpoint_key,
        model,
        expected,
        observed,
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
    credential_readiness: crate::credentials::CredentialReadinessCode,
) -> Vec<CodexRelayProbeObservation> {
    if !credential_readiness.is_routable() {
        return codex_relay_probe_cases()
            .iter()
            .map(|case| {
                super::codex_relay_probe::credential_observation(case.kind, credential_readiness)
            })
            .collect();
    }
    let mut observations = Vec::with_capacity(codex_relay_probe_cases().len());
    for case in codex_relay_probe_cases() {
        observations.push(
            probe_client
                .probe_upstream_observation_with_readiness(
                    upstream,
                    &case.spec(),
                    credential_readiness,
                )
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

fn build_expected_provider_contract(
    runtime_snapshot: &super::runtime_config::RuntimeSnapshot,
    target: &SelectedCodexRelayTarget,
    model: Option<&str>,
    models_observation: &CodexRelayProbeObservation,
) -> Result<CodexRelayProviderContract, ProxyControlError> {
    let endpoint = reqwest::Url::parse(&target.upstream.base_url).map_err(|error| {
        ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            format!("selected Codex relay upstream base URL is invalid: {error}"),
        )
    })?;
    let provider_adapter = ProviderAdapter::for_endpoint(&endpoint);
    let catalog_epoch =
        capture_provider_catalog_epoch(runtime_snapshot, target, provider_adapter, &endpoint);
    let catalog_model =
        model.and_then(|model| catalog_epoch.as_ref().and_then(|epoch| epoch.model(model)));
    let model_catalog = translated_models_catalog(models_observation, model).unwrap_or_else(|| {
        CodexModelCatalogProfile::unknown(models_observation.result.reason.clone())
    });
    let web_search = model_catalog.selected_web_search_support();
    let apply_patch = model_catalog.selected_apply_patch_support();
    let reasoning_summaries = model_catalog.selected_reasoning_summary_support();

    Ok(CodexRelayProviderContract {
        provider_adapter,
        catalog_revision: catalog_epoch
            .as_ref()
            .map(|epoch| epoch.revision().as_str().to_string()),
        request_dialects: vec![
            "responses_http".to_string(),
            "responses_compact".to_string(),
            "responses_websocket".to_string(),
        ],
        model_catalog,
        responses: adapter_responses_support(provider_adapter),
        remote_compaction_v1: adapter_remote_compaction_support(provider_adapter),
        hosted_image_generation: CodexCapabilityDecision::unknown(
            "hosted image generation is not inferred from Codex-owned auth or client presets; request tools are preserved for the selected provider",
        ),
        responses_websocket: catalog_websocket_support(model, catalog_model),
        ultra_maps_to_max: catalog_ultra_mapping_support(model, catalog_model),
        web_search,
        apply_patch,
        reasoning_summaries,
    })
}

fn capture_provider_catalog_epoch(
    runtime_snapshot: &super::runtime_config::RuntimeSnapshot,
    target: &SelectedCodexRelayTarget,
    adapter: ProviderAdapter,
    endpoint: &reqwest::Url,
) -> Option<ProviderCatalogEpoch> {
    if adapter != ProviderAdapter::OpenAiCodex {
        return None;
    }

    let credential_generation = runtime_snapshot.credential_generation();
    let credential = credential_generation
        .capture_bound(&target.provider_endpoint)
        .ok()?;
    if !credential.is_available() {
        return None;
    }
    let credential_scope = credential_generation
        .credential_scope_for_route_digest(&target.provider_endpoint)
        .ok()?;
    let account_fingerprint = credential_scope
        .map(AccountFingerprint::from_credential_scope)
        .unwrap_or_else(AccountFingerprint::unscoped);
    let route_scope = target.provider_endpoint.stable_key();
    let scope = ProviderCatalogScope::new(
        adapter,
        endpoint.as_str(),
        route_scope,
        account_fingerprint,
        runtime_snapshot.digest(),
    )
    .ok()?;
    runtime_snapshot
        .provider_catalog()
        .capture_epoch(scope)
        .ok()
}

fn adapter_responses_support(adapter: ProviderAdapter) -> CodexCapabilityDecision {
    match adapter {
        ProviderAdapter::OpenAiCodex => CodexCapabilityDecision::supported(
            "captured provider endpoint uses the official OpenAI Codex adapter",
        ),
        ProviderAdapter::AwsBedrock | ProviderAdapter::OpenAiCompatible => {
            CodexCapabilityDecision::unknown(
                "the selected adapter does not provide an authoritative Responses contract; inspect the endpoint probe",
            )
        }
    }
}

fn adapter_remote_compaction_support(adapter: ProviderAdapter) -> CodexCapabilityDecision {
    match adapter {
        ProviderAdapter::OpenAiCodex => CodexCapabilityDecision::supported(
            "the captured official OpenAI Codex contract supports Responses compact",
        ),
        ProviderAdapter::AwsBedrock | ProviderAdapter::OpenAiCompatible => {
            CodexCapabilityDecision::unknown(
                "Responses compact is not inferred for a compatible relay; inspect the endpoint probe",
            )
        }
    }
}

fn catalog_websocket_support(
    requested_model: Option<&str>,
    model: Option<&crate::provider_catalog::ProviderModelCapabilities>,
) -> CodexCapabilityDecision {
    match (requested_model, model) {
        (_, Some(model)) if model.prefers_websockets() => CodexCapabilityDecision::supported(
            "the captured provider catalog model prefers Responses WebSocket transport",
        ),
        (_, Some(_)) => CodexCapabilityDecision::unsupported(
            "the captured provider catalog model does not advertise Responses WebSocket transport",
        ),
        (Some(model), None) => CodexCapabilityDecision::unknown(format!(
            "model {model:?} has no authoritative contract in the captured provider catalog"
        )),
        (None, None) => CodexCapabilityDecision::unknown(
            "no model was selected, so WebSocket support cannot be read from the provider catalog",
        ),
    }
}

fn catalog_ultra_mapping_support(
    requested_model: Option<&str>,
    model: Option<&crate::provider_catalog::ProviderModelCapabilities>,
) -> CodexCapabilityDecision {
    let Some(model) = model else {
        return CodexCapabilityDecision::unknown(match requested_model {
            Some(model) => format!(
                "model {model:?} has no authoritative ultra mapping in the captured provider catalog"
            ),
            None => "no model was selected, so ultra mapping cannot be resolved".to_string(),
        });
    };
    let efforts = model.supported_reasoning_efforts();
    if efforts.contains(&CatalogReasoningEffort::Ultra)
        && efforts.contains(&CatalogReasoningEffort::Max)
    {
        CodexCapabilityDecision::supported(
            "the captured provider model contract maps Codex ultra intent to upstream max effort",
        )
    } else {
        CodexCapabilityDecision::unsupported(
            "the captured provider model contract does not authorize the ultra-to-max mapping",
        )
    }
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
        ModelsTranslationScope::Disabled,
    );
    let value = serde_json::from_slice::<Value>(body.as_ref()).ok()?;
    Some(CodexModelCatalogProfile::from_models_response_json(
        &value, model,
    ))
}

fn build_mismatches(
    expected: &CodexRelayProviderContract,
    observed: &CodexRelayCapabilitiesObserved,
    configured_translate_models: bool,
) -> Vec<CodexRelayCapabilityMismatch> {
    let mut out = Vec::new();
    push_endpoint_mismatch(
        &mut out,
        "responses",
        &expected.responses,
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
            reason: model_catalog_translation_reason(configured_translate_models).to_string(),
        });
    }
    out
}

fn model_catalog_translation_reason(configured_translate_models: bool) -> &'static str {
    if configured_translate_models {
        "relay returned an OpenAI models list; server-default model translation is enabled and uses the selected provider's captured catalog, while a client runtime patch may override that default"
    } else {
        "relay returned an OpenAI models list; server-default model translation is disabled, but a client runtime patch may enable translation using the selected provider's captured catalog"
    }
}

fn build_continuity_diagnostics(
    service_name: &str,
    view: Option<&ServiceRouteConfig>,
    target: &SelectedCodexRelayTarget,
    expected: &CodexRelayProviderContract,
    observed: &CodexRelayCapabilitiesObserved,
) -> CodexRelayContinuityDiagnostics {
    let provider_endpoint_key = target.provider_endpoint.stable_key();
    let fallback_domain = format!("provider_endpoint:{provider_endpoint_key}");
    let mut selected_domain = CodexRelayContinuityDomainSummary {
        key: fallback_domain,
        explicit: false,
    };
    let mut same_domain_endpoint_count = 1usize;
    let mut configured_endpoint_count = 1usize;
    let mut affinity_policy = None;

    if let Some(view) = view {
        let routing = effective_routing(view);
        affinity_policy = Some(routing_affinity_policy_label(routing.affinity_policy).to_string());
        if let Ok(template) = compile_route_handshake_plan(service_name, view) {
            let topology = template.continuity_topology();
            configured_endpoint_count = topology.configured_provider_endpoint_count().max(1);
            if let Some(summary) = topology.selected_domain_summary(provider_endpoint_key.as_str())
            {
                selected_domain = domain_summary(&summary.domain);
                same_domain_endpoint_count = summary.same_domain_endpoint_count;
            }
        }
    }

    let remote_compaction_available = expected.remote_compaction_v1.is_supported()
        || observed.responses_compact.support == super::CodexRelayProbeSupport::Supported;
    let responses_websocket_available = expected.responses_websocket.is_supported();
    let can_state_bound_failover_within_domain =
        selected_domain.explicit && same_domain_endpoint_count > 1;
    let mut warnings = Vec::new();
    let mut recommendations = Vec::new();

    if remote_compaction_available && !selected_domain.explicit && configured_endpoint_count > 1 {
        warnings.push(
            "Responses compact is available with multiple configured provider endpoints, but the selected endpoint has no explicit continuity_domain".to_string(),
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
    ) && remote_compaction_available
        && configured_endpoint_count > 1
    {
        warnings.push(
            "remote compaction with multiple provider endpoints should use fallback-sticky or hard affinity when encrypted compact state matters".to_string(),
        );
    }

    if responses_websocket_available && !selected_domain.explicit && configured_endpoint_count > 1 {
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

fn routing_affinity_policy_label(policy: crate::config::RouteAffinityPolicy) -> &'static str {
    match policy {
        crate::config::RouteAffinityPolicy::Off => "off",
        crate::config::RouteAffinityPolicy::PreferredGroup => "preferred-group",
        crate::config::RouteAffinityPolicy::FallbackSticky => "fallback-sticky",
        crate::config::RouteAffinityPolicy::Hard => "hard",
    }
}

fn push_endpoint_mismatch(
    out: &mut Vec<CodexRelayCapabilityMismatch>,
    capability: &str,
    expected: &CodexCapabilityDecision,
    observed: &CodexRelayProbeResult,
) {
    if observed.confidence == super::CodexRelayProbeConfidence::Credential {
        return;
    }
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
        super::CodexRelayProbeConfidence::Credential => "credential",
        super::CodexRelayProbeConfidence::Transport => "transport",
        super::CodexRelayProbeConfidence::Malformed => "malformed",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use reqwest::Client;

    use super::*;
    use crate::codex_capability_profile::CodexCapabilitySupport;
    use crate::config::{
        HelperConfig, ProviderConfig, RouteGraphConfig, ServiceRouteConfig, UpstreamAuth,
    };

    fn probe_result(
        kind: CodexRelayProbeKind,
        support: super::super::CodexRelayProbeSupport,
    ) -> CodexRelayProbeResult {
        CodexRelayProbeResult {
            kind,
            support,
            confidence: super::super::CodexRelayProbeConfidence::SuccessStatus,
            credential_readiness: None,
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
    fn codex_relay_capabilities_request_accepts_provider_target_fields() {
        let request = serde_json::from_value::<CodexRelayCapabilitiesRequest>(serde_json::json!({
            "provider_id": "relay",
            "endpoint_id": "primary",
            "model": "gpt-5.6-sol"
        }))
        .expect("provider target should deserialize");

        assert_eq!(request.provider_id.as_deref(), Some("relay"));
        assert_eq!(request.endpoint_id.as_deref(), Some("primary"));
        assert_eq!(request.model.as_deref(), Some("gpt-5.6-sol"));
    }

    #[test]
    fn model_catalog_mismatch_explains_server_and_client_translation_precedence() {
        let enabled = model_catalog_translation_reason(true);
        assert!(enabled.contains("server-default model translation is enabled"));
        assert!(enabled.contains("client runtime patch may override"));
        assert!(enabled.contains("selected provider's captured catalog"));

        let disabled = model_catalog_translation_reason(false);
        assert!(disabled.contains("server-default model translation is disabled"));
        assert!(disabled.contains("client runtime patch may enable"));
        assert!(disabled.contains("selected provider's captured catalog"));
    }

    #[test]
    fn codex_relay_capabilities_request_rejects_removed_fields() {
        for field in [
            ("station_name", serde_json::json!("legacy-station")),
            ("upstream_index", serde_json::json!(1)),
            ("patch_preset", serde_json::json!("official-relay")),
            ("patch_mode", serde_json::json!("official_relay_bridge")),
            ("compaction", serde_json::json!("remote_v1")),
            ("responses_websocket", serde_json::json!(true)),
        ] {
            let payload = serde_json::Value::Object(serde_json::Map::from_iter([(
                field.0.to_string(),
                field.1,
            )]));
            let error = serde_json::from_value::<CodexRelayCapabilitiesRequest>(payload)
                .expect_err("removed client preset field should be rejected");

            assert!(
                error.to_string().contains("unknown field"),
                "unexpected error for {}: {error}",
                field.0
            );
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
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "input8".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "ciii".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "input8".to_string(),
                    "ciii".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");

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

        assert_eq!(response.provider_id, "ciii");
        assert_eq!(response.endpoint_id, "default");
        assert_eq!(response.provider_endpoint_key, "codex/ciii/default");
        let serialized = serde_json::to_value(&response).expect("serialize response");
        assert!(serialized.get("station_name").is_none());
        assert!(serialized.get("upstream_index").is_none());
        assert!(serialized.get("upstream_base_url").is_none());
        for field in ["provider_id", "endpoint_id", "provider_endpoint_key"] {
            let mut missing = serialized.clone();
            missing
                .as_object_mut()
                .expect("response object")
                .remove(field);
            assert!(
                serde_json::from_value::<CodexRelayCapabilitiesResponse>(missing).is_err(),
                "missing canonical field {field} should fail"
            );
        }
    }

    #[tokio::test]
    async fn credential_blocked_capabilities_do_not_probe_or_report_mismatches() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_route = hits.clone();
        let app = axum::Router::new().fallback(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "models": [] }))
            }
        });
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve probe target");
        });
        let missing_reference = format!(
            "CODEX_HELPER_TEST_MISSING_CAPABILITY_AUTH_{}",
            uuid::Uuid::new_v4().simple()
        );
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "openai".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://api.openai.com:{}/v1", addr.port())),
                        inline_auth: UpstreamAuth {
                            auth_token_env: Some(missing_reference.clone()),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "openai".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve("api.openai.com", addr)
            .build()
            .expect("build capability client");
        let proxy = ProxyService::new(client, Arc::new(source), "codex");

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("openai".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("credential blockage should be reported as observations");

        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            response.expected.provider_adapter,
            ProviderAdapter::OpenAiCodex
        );
        assert!(response.mismatches.is_empty());
        for result in [
            &response.observed.models,
            &response.observed.responses,
            &response.observed.responses_compact,
        ] {
            assert_eq!(
                result.confidence,
                super::super::CodexRelayProbeConfidence::Credential
            );
            assert_eq!(
                result.credential_readiness,
                Some(crate::credentials::CredentialReadinessCode::Missing)
            );
            assert!(!result.reason.contains(&missing_reference));
        }

        handle.abort();
    }

    #[tokio::test]
    async fn compatible_relay_without_probe_does_not_infer_continuity_risk() {
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "relay-a".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "relay-b".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "relay-a".to_string(),
                    "relay-b".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("relay-a".to_string()),
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
        assert!(response.continuity.warnings.is_empty());
    }

    #[tokio::test]
    async fn codex_relay_capabilities_reports_compatible_provider_contract() {
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "relay-a".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "relay-b".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "relay-a".to_string(),
                    "relay-b".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("relay-a".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("capabilities response");

        assert_eq!(
            response.expected.provider_adapter,
            ProviderAdapter::OpenAiCompatible
        );
        assert_eq!(response.expected.catalog_revision, None);
        assert_eq!(
            response.expected.remote_compaction_v1.support,
            CodexCapabilitySupport::Unknown
        );
        assert!(
            response.continuity.warnings.is_empty(),
            "compatible relays without a successful probe must not imply compact continuity"
        );
        assert!(
            !response
                .mismatches
                .iter()
                .any(|mismatch| mismatch.capability == "remote_compaction_v1"),
            "an unknown provider contract must not claim a compact mismatch"
        );
    }

    #[tokio::test]
    async fn official_provider_contract_uses_captured_catalog() {
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([(
                    "openai".to_string(),
                    ProviderConfig {
                        base_url: Some("https://api.openai.com/v1".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "openai".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
        let runtime_snapshot = proxy.config.capture().await;
        let graph = runtime_snapshot
            .route_graph("codex")
            .expect("compiled codex graph");
        let target = select_codex_relay_target(
            graph.as_ref(),
            CodexRelayTargetSelection {
                provider_id: Some("openai"),
                endpoint_id: None,
            },
        )
        .expect("select official provider target");
        let models_observation = observation(CodexRelayProbeKind::Models);

        let contract = build_expected_provider_contract(
            runtime_snapshot.as_ref(),
            &target,
            Some("gpt-5.6-sol"),
            &models_observation,
        )
        .expect("build official provider contract");

        assert_eq!(contract.provider_adapter, ProviderAdapter::OpenAiCodex);
        assert!(contract.catalog_revision.is_some());
        assert_eq!(
            contract.responses.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            contract.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            contract.responses_websocket.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            contract.ultra_maps_to_max.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[tokio::test]
    async fn codex_relay_capabilities_does_not_infer_official_openai_domain_from_same_host() {
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "openai-a".to_string(),
                        ProviderConfig {
                            base_url: Some("https://api.openai.com/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "openai-b".to_string(),
                        ProviderConfig {
                            base_url: Some("https://api.openai.com/v1".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "openai-a".to_string(),
                    "openai-b".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("openai-a".to_string()),
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
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "relay-a".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:9/v1".to_string()),
                            continuity_domain: Some("relay-cluster".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "relay-b".to_string(),
                        ProviderConfig {
                            base_url: Some("http://127.0.0.1:10/v1".to_string()),
                            continuity_domain: Some("relay-cluster".to_string()),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "relay-a".to_string(),
                    "relay-b".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");

        let response = codex_relay_capabilities_for_proxy(
            &proxy,
            CodexRelayCapabilitiesRequest {
                provider_id: Some("relay-a".to_string()),
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

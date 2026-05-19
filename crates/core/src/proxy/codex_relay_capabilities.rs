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

use super::codex_relay_probe::CodexRelayProbeObservation;
use super::codex_relay_target::{CodexRelayTargetSelection, select_codex_relay_target};
use super::models_compat::maybe_decode_models_response_body;
use super::{
    CodexRelayProbeClient, CodexRelayProbeKind, CodexRelayProbeResult, CodexRelayProbeSpec,
    ProxyControlError, ProxyService,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesRequest {
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub upstream_index: Option<usize>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub patch_mode: Option<CodexPatchMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexRelayCapabilitiesResponse {
    pub api_version: u32,
    pub service_name: String,
    pub station_name: String,
    pub upstream_index: usize,
    pub upstream_base_url: String,
    pub patch_mode: CodexPatchMode,
    pub model: Option<String>,
    pub expected: CodexCapabilityProfile,
    pub observed: CodexRelayCapabilitiesObserved,
    pub recommendation: CodexPatchModeRecommendation,
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
        },
    )?;
    let patch_mode = payload
        .patch_mode
        .or_else(current_codex_switch_patch_mode)
        .or_else(|| crate::config::codex_client_patch_mode_from_config_file().ok())
        .unwrap_or_default();
    let model = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let probe_client = CodexRelayProbeClient::new(proxy.client.clone());
    let models_observation = probe_client
        .probe_upstream_observation(
            &target.upstream,
            &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::Models),
        )
        .await;
    let responses = probe_client
        .probe_upstream(
            &target.upstream,
            &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::Responses),
        )
        .await;
    let responses_compact = probe_client
        .probe_upstream(
            &target.upstream,
            &CodexRelayProbeSpec::for_kind(CodexRelayProbeKind::ResponsesCompact),
        )
        .await;

    let expected = build_expected_profile(patch_mode, model.as_deref(), &models_observation);
    let observed = CodexRelayCapabilitiesObserved {
        models: models_observation.result,
        responses,
        responses_compact,
    };
    let recommendation = build_recommendation(patch_mode, &expected, &observed);
    let mismatches = build_mismatches(&expected, &observed);

    let response = CodexRelayCapabilitiesResponse {
        api_version: 1,
        service_name: proxy.service_name.to_string(),
        station_name: target.station_name,
        upstream_index: target.upstream_index,
        upstream_base_url: target.upstream.base_url,
        patch_mode,
        model,
        expected,
        observed,
        recommendation,
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

fn current_codex_switch_patch_mode() -> Option<CodexPatchMode> {
    crate::codex_integration::codex_switch_status()
        .ok()
        .and_then(|status| status.patch_mode)
}

fn build_expected_profile(
    patch_mode: CodexPatchMode,
    model: Option<&str>,
    models_observation: &CodexRelayProbeObservation,
) -> CodexCapabilityProfile {
    let model_catalog = translated_models_catalog(models_observation, model).unwrap_or_else(|| {
        CodexModelCatalogProfile::unknown(models_observation.result.reason.clone())
    });
    CodexCapabilityProfile::for_input(CodexCapabilityProfileInput::from_patch_mode(
        patch_mode,
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
            reason: "relay returned an OpenAI models list; helper translation is required before Codex can see model metadata".to_string(),
        });
    }
    out
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

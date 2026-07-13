use std::sync::Arc;

use owo_colors::OwoColorize;

use crate::cli_types::CodexCommand;
use crate::config::load_config_with_source;
use crate::proxy::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse,
    CodexRelayEvidenceFilters, CodexRelayEvidenceKind, CodexRelayLiveSmokeCase,
    CodexRelayLiveSmokeRequest, CodexRelayLiveSmokeResponse, ProxyService,
    codex_relay_evidence_path, read_recent_codex_relay_evidence,
};
use crate::{CliError, CliResult};
use codex_helper_core::codex_capability_profile::{
    CodexCapabilityDecision, CodexCapabilitySupport,
};

pub(crate) async fn handle_codex_cmd(cmd: CodexCommand) -> CliResult<()> {
    match cmd {
        CodexCommand::Capabilities {
            provider,
            endpoint,
            model,
            json,
        } => {
            let proxy = build_codex_proxy_for_cli().await?;
            let response = proxy
                .codex_relay_capabilities(CodexRelayCapabilitiesRequest {
                    provider_id: provider,
                    endpoint_id: endpoint,
                    model,
                })
                .await
                .map_err(|err| CliError::Other(err.to_string()))?;
            if json {
                print_json(&response)?;
            } else {
                print_capabilities_text(&response);
            }
        }
        CodexCommand::LiveSmoke {
            acknowledgement,
            provider,
            endpoint,
            model,
            image,
            compact_v2,
            websocket,
            service_tier,
            json,
        } => {
            let proxy = build_codex_proxy_for_cli().await?;
            let cases = live_smoke_cases(image, compact_v2, websocket);
            let response = proxy
                .codex_relay_live_smoke(CodexRelayLiveSmokeRequest {
                    acknowledgement: Some(acknowledgement),
                    provider_id: provider,
                    endpoint_id: endpoint,
                    model: Some(model),
                    cases,
                    service_tier,
                })
                .await
                .map_err(|err| CliError::Other(err.to_string()))?;
            if json {
                print_json(&response)?;
            } else {
                print_live_smoke_text(&response);
            }
        }
        CodexCommand::Evidence {
            limit,
            kind,
            provider,
            model,
            json,
        } => {
            let filters = CodexRelayEvidenceFilters {
                kind: kind.map(Into::into),
                provider_id: provider,
                model,
            };
            let entries = read_recent_codex_relay_evidence(limit, &filters)
                .map_err(|err| CliError::Usage(err.to_string()))?;
            if json {
                print_json(&entries)?;
            } else {
                print_evidence_text(&entries);
            }
        }
    }
    Ok(())
}

fn live_smoke_cases(
    image: bool,
    compact_v2: bool,
    websocket: bool,
) -> Vec<CodexRelayLiveSmokeCase> {
    let mut cases = Vec::new();
    if image {
        cases.push(CodexRelayLiveSmokeCase::HostedImageGeneration);
    }
    if compact_v2 {
        cases.push(CodexRelayLiveSmokeCase::RemoteCompactionV2);
    }
    if websocket {
        cases.push(CodexRelayLiveSmokeCase::ResponsesWebSocket);
    }
    if cases.is_empty() {
        cases.push(CodexRelayLiveSmokeCase::ResponsesCompact);
    }
    cases
}

async fn build_codex_proxy_for_cli() -> CliResult<ProxyService> {
    let loaded = load_config_with_source()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    crate::runtime_host::validate_service_has_upstream("codex", &loaded.source)
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    ProxyService::new_ephemeral_diagnostic(Arc::new(loaded.source), "codex")
        .map_err(|err| CliError::Other(err.to_string()))
}

fn print_json<T: serde::Serialize>(value: &T) -> CliResult<()> {
    let text =
        serde_json::to_string_pretty(value).map_err(|err| CliError::Other(err.to_string()))?;
    println!("{text}");
    Ok(())
}

fn print_capabilities_text(response: &CodexRelayCapabilitiesResponse) {
    println!("{}", "Codex relay capability diagnostics".bold());
    println!(
        "Target: provider={} endpoint={} key={}",
        response.provider_id, response.endpoint_id, response.provider_endpoint_key
    );
    println!(
        "Provider contract: adapter={:?}; catalog_revision={}; model={}; request_dialects={}",
        response.expected.provider_adapter,
        response
            .expected
            .catalog_revision
            .as_deref()
            .unwrap_or("<none>"),
        response.model.as_deref().unwrap_or("<none>"),
        if response.expected.request_dialects.is_empty() {
            "<none>".to_string()
        } else {
            response.expected.request_dialects.join(", ")
        }
    );
    println!(
        "Model catalog: shape={:?}; selection={:?}; translation_required={}; reason={}",
        response.expected.model_catalog.shape,
        response.expected.model_catalog.selection,
        response.expected.model_catalog.translation_required,
        response.expected.model_catalog.reason
    );
    println!("Expected capabilities:");
    print_capability_decision("responses", &response.expected.responses);
    print_capability_decision(
        "remote_compaction_v1",
        &response.expected.remote_compaction_v1,
    );
    print_capability_decision(
        "hosted_image_generation",
        &response.expected.hosted_image_generation,
    );
    print_capability_decision(
        "responses_websocket",
        &response.expected.responses_websocket,
    );
    print_capability_decision("ultra_maps_to_max", &response.expected.ultra_maps_to_max);
    print_capability_decision("web_search", &response.expected.web_search);
    print_capability_decision("apply_patch", &response.expected.apply_patch);
    print_capability_decision(
        "reasoning_summaries",
        &response.expected.reasoning_summaries,
    );
    println!(
        "Observed: models={:?}/{:?}, responses={:?}/{:?}, compact={:?}/{:?}",
        response.observed.models.support,
        response.observed.models.confidence,
        response.observed.responses.support,
        response.observed.responses.confidence,
        response.observed.responses_compact.support,
        response.observed.responses_compact.confidence
    );
    println!(
        "Continuity: domain={} explicit={} same_domain_endpoints={} configured_endpoints={} affinity={} state_bound_failover={}",
        response.continuity.selected_domain.key,
        response.continuity.selected_domain.explicit,
        response.continuity.same_domain_endpoint_count,
        response.continuity.configured_endpoint_count,
        response
            .continuity
            .affinity_policy
            .as_deref()
            .unwrap_or("<unknown>"),
        response.continuity.can_state_bound_failover_within_domain
    );
    for warning in &response.continuity.warnings {
        println!("{} {}", "Continuity warning:".yellow(), warning);
    }
    for recommendation in &response.continuity.recommendations {
        println!("Continuity recommendation: {recommendation}");
    }
    if response.mismatches.is_empty() {
        println!("Mismatches: none");
    } else {
        println!("Mismatches: {}", response.mismatches.len());
    }
    for mismatch in &response.mismatches {
        println!(
            "{} {}: expected={}, observed={}, reason={}",
            "Mismatch".yellow(),
            mismatch.capability,
            mismatch.expected,
            mismatch.observed,
            mismatch.reason
        );
    }
    println!("Evidence: {:?}", codex_relay_evidence_path());
}

fn print_capability_decision(label: &str, decision: &CodexCapabilityDecision) {
    println!(
        "  {label}: {} ({})",
        support_label(decision.support),
        decision.reason
    );
}

fn support_label(support: CodexCapabilitySupport) -> &'static str {
    match support {
        CodexCapabilitySupport::Unknown => "unknown",
        CodexCapabilitySupport::Supported => "supported",
        CodexCapabilitySupport::Unsupported => "unsupported",
    }
}

fn print_live_smoke_text(response: &CodexRelayLiveSmokeResponse) {
    println!("{}", "Codex relay live smoke".bold());
    println!(
        "Target: provider={} endpoint={} key={}",
        response.provider_id, response.endpoint_id, response.provider_endpoint_key
    );
    println!(
        "Model: requested={}, upstream={}",
        response.requested_model, response.upstream_model
    );
    for result in &response.results {
        println!(
            "{:?}: {:?}/{:?} status={:?} items={} compaction_items={} completed={} image_call={} image_result={} reason={}",
            result.case,
            result.outcome,
            result.confidence,
            result.status_code,
            result.output_items_seen,
            result.compaction_output_items_seen,
            result.response_completed_seen,
            result.image_generation_call_seen,
            result.image_result_present,
            result.reason
        );
    }
    for warning in &response.warnings {
        println!("{} {}", "Warning:".yellow(), warning);
    }
    println!("Evidence: {:?}", codex_relay_evidence_path());
    println!(
        "Live smoke acknowledgement required by API/CLI: {}",
        CODEX_RELAY_LIVE_SMOKE_ACK
    );
}

fn print_evidence_text(entries: &[crate::proxy::CodexRelayEvidenceEntry]) {
    let path = codex_relay_evidence_path();
    println!("{}", format!("Codex relay evidence from {:?}", path).bold());
    if entries.is_empty() {
        println!("No evidence records matched.");
        return;
    }
    for entry in entries {
        println!(
            "{} {} {:?} provider={} endpoint={} key={} model={} source={}",
            entry.timestamp_ms,
            entry.evidence_id,
            entry.kind,
            entry.provider_id,
            entry.endpoint_id,
            entry.provider_endpoint_key,
            entry.model.as_deref().unwrap_or("-"),
            entry.source
        );
        match entry.kind {
            CodexRelayEvidenceKind::CapabilityDiagnostics => {
                let provider_adapter = entry
                    .payload
                    .pointer("/expected/provider_adapter")
                    .and_then(|value| value.as_str());
                let mismatch_count = entry
                    .payload
                    .get("mismatches")
                    .and_then(|value| value.as_array())
                    .map(Vec::len);
                if provider_adapter.is_some() || mismatch_count.is_some() {
                    println!(
                        "  provider_adapter={} mismatches={}",
                        provider_adapter.unwrap_or("unknown"),
                        mismatch_count
                            .map(|count| count.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    );
                }
            }
            CodexRelayEvidenceKind::LiveSmoke => {
                if let Some(results) = entry
                    .payload
                    .get("results")
                    .and_then(|value| value.as_array())
                {
                    let summary = results
                        .iter()
                        .filter_map(|result| {
                            let case = result.get("case")?.as_str()?;
                            let outcome = result.get("outcome")?.as_str()?;
                            Some(format!("{case}:{outcome}"))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !summary.is_empty() {
                        println!("  results={summary}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_smoke_cases_defaults_to_compact() {
        assert_eq!(
            live_smoke_cases(false, false, false),
            vec![CodexRelayLiveSmokeCase::ResponsesCompact]
        );
    }

    #[test]
    fn live_smoke_cases_websocket_flag_runs_websocket_only() {
        assert_eq!(
            live_smoke_cases(false, false, true),
            vec![CodexRelayLiveSmokeCase::ResponsesWebSocket]
        );
    }

    #[test]
    fn live_smoke_cases_image_flag_runs_image_only() {
        assert_eq!(
            live_smoke_cases(true, false, false),
            vec![CodexRelayLiveSmokeCase::HostedImageGeneration]
        );
    }

    #[test]
    fn live_smoke_cases_compact_v2_flag_runs_compact_v2_only() {
        assert_eq!(
            live_smoke_cases(false, true, false),
            vec![CodexRelayLiveSmokeCase::RemoteCompactionV2]
        );
    }

    #[test]
    fn live_smoke_cases_combines_explicit_optional_smokes_without_compact() {
        assert_eq!(
            live_smoke_cases(true, true, true),
            vec![
                CodexRelayLiveSmokeCase::HostedImageGeneration,
                CodexRelayLiveSmokeCase::RemoteCompactionV2,
                CodexRelayLiveSmokeCase::ResponsesWebSocket,
            ]
        );
    }
}

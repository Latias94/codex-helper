use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use owo_colors::OwoColorize;
use reqwest::Client;

use crate::cli_types::CodexCommand;
use crate::config::{ServiceKind, load_or_bootstrap_for_service_with_v4_source};
use crate::proxy::{
    CODEX_RELAY_LIVE_SMOKE_ACK, CodexRelayCapabilitiesRequest, CodexRelayCapabilitiesResponse,
    CodexRelayEvidenceFilters, CodexRelayEvidenceKind, CodexRelayLiveSmokeCase,
    CodexRelayLiveSmokeRequest, CodexRelayLiveSmokeResponse, ProxyService,
    codex_relay_evidence_path, read_recent_codex_relay_evidence,
};
use crate::{CliError, CliResult};
use codex_helper_core::codex_capability_profile::CodexCapabilitySupport;

pub(crate) async fn handle_codex_cmd(cmd: CodexCommand) -> CliResult<()> {
    match cmd {
        CodexCommand::Capabilities {
            station,
            provider,
            endpoint,
            upstream_index,
            model,
            preset,
            json,
        } => {
            let proxy = build_codex_proxy_for_cli().await?;
            let response = proxy
                .codex_relay_capabilities(CodexRelayCapabilitiesRequest {
                    station_name: station,
                    provider_id: provider,
                    endpoint_id: endpoint,
                    upstream_index,
                    model,
                    patch_mode: preset.map(Into::into),
                    responses_websocket: None,
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
            station,
            provider,
            endpoint,
            upstream_index,
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
                    station_name: station,
                    provider_id: provider,
                    endpoint_id: endpoint,
                    upstream_index,
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
            station,
            model,
            json,
        } => {
            let filters = CodexRelayEvidenceFilters {
                kind: kind.map(Into::into),
                station_name: station,
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
    let loaded = load_or_bootstrap_for_service_with_v4_source(ServiceKind::Codex)
        .await
        .map_err(|err| CliError::ProxyConfig(err.to_string()))?;
    let cfg = Arc::new(loaded.runtime);
    if cfg.codex.configs.is_empty() || cfg.codex.active_station().is_none() {
        return Err(CliError::ProxyConfig(
            "未找到任何可用的 Codex 上游配置，请先运行 `codex-helper config init` 或 `codex-helper provider add`".to_string(),
        ));
    }
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| CliError::Other(err.to_string()))?;
    Ok(ProxyService::new_with_v4_source(
        client,
        cfg,
        loaded.v4.map(Arc::new),
        "codex",
        Arc::new(Mutex::new(HashMap::new())),
    ))
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
        "Target: {} {}",
        format_target_identity(
            &response.station_name,
            response.upstream_index,
            response.provider_endpoint_key.as_deref()
        ),
        response.upstream_base_url
    );
    println!(
        "Preset: {}; Responses WebSocket: {}; model: {}",
        response.patch_mode.as_preset_str(),
        response.responses_websocket,
        response.model.as_deref().unwrap_or("<none>")
    );
    println!(
        "Expected: compact={} ({:?}); continuity identity={} state_sharing={} ({:?})",
        support_label(response.expected.remote_compaction_v1.support),
        response.expected.provider_identity,
        support_label(
            response
                .expected
                .continuity
                .identity_sets_compact_path
                .support
        ),
        support_label(response.expected.continuity.state_sharing.support),
        response.expected.continuity.state_sharing.support
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
    println!(
        "Recommendation: {} ({:?})",
        response
            .recommendation
            .recommended_patch_mode
            .as_preset_str(),
        response.recommendation.confidence
    );
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
        "Target: {} {}",
        format_target_identity(
            &response.station_name,
            response.upstream_index,
            response.provider_endpoint_key.as_deref()
        ),
        response.upstream_base_url
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

fn format_target_identity(
    station_name: &str,
    upstream_index: usize,
    provider_endpoint_key: Option<&str>,
) -> String {
    match provider_endpoint_key {
        Some(provider_endpoint_key) => {
            format!("{provider_endpoint_key} via {station_name}[{upstream_index}]")
        }
        None => format!("{station_name}[{upstream_index}]"),
    }
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
            "{} {} {:?} station={} upstream={} model={} source={}",
            entry.timestamp_ms,
            entry.evidence_id,
            entry.kind,
            entry
                .provider_endpoint_key
                .as_deref()
                .unwrap_or(entry.station_name.as_str()),
            entry.upstream_index,
            entry.model.as_deref().unwrap_or("-"),
            entry.source
        );
        match entry.kind {
            CodexRelayEvidenceKind::CapabilityDiagnostics => {
                if let Some(recommended) = entry
                    .payload
                    .get("recommendation")
                    .and_then(|value| value.get("recommended_patch_mode"))
                    .and_then(|value| value.as_str())
                {
                    println!("  recommendation={recommended}");
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

    #[test]
    fn format_target_identity_includes_provider_endpoint_when_available() {
        assert_eq!(
            format_target_identity("routing", 9, Some("codex/ciii/default")),
            "codex/ciii/default via routing[9]"
        );
        assert_eq!(format_target_identity("input", 0, None), "input[0]");
    }
}

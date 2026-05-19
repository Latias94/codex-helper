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

pub(crate) async fn handle_codex_cmd(cmd: CodexCommand) -> CliResult<()> {
    match cmd {
        CodexCommand::Capabilities {
            station,
            upstream_index,
            model,
            preset,
            json,
        } => {
            let proxy = build_codex_proxy_for_cli().await?;
            let response = proxy
                .codex_relay_capabilities(CodexRelayCapabilitiesRequest {
                    station_name: station,
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
            upstream_index,
            model,
            image,
            service_tier,
            json,
        } => {
            let proxy = build_codex_proxy_for_cli().await?;
            let mut cases = vec![CodexRelayLiveSmokeCase::ResponsesCompact];
            if image {
                cases.push(CodexRelayLiveSmokeCase::HostedImageGeneration);
            }
            let response = proxy
                .codex_relay_live_smoke(CodexRelayLiveSmokeRequest {
                    acknowledgement: Some(acknowledgement),
                    station_name: station,
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
        "Target: {}[{}] {}",
        response.station_name, response.upstream_index, response.upstream_base_url
    );
    println!(
        "Patch preset: {}; Responses WebSocket: {}; model: {}",
        response.patch_mode.as_preset_str(),
        response.responses_websocket,
        response.model.as_deref().unwrap_or("<none>")
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

fn print_live_smoke_text(response: &CodexRelayLiveSmokeResponse) {
    println!("{}", "Codex relay live smoke".bold());
    println!(
        "Target: {}[{}] {}",
        response.station_name, response.upstream_index, response.upstream_base_url
    );
    println!(
        "Model: requested={}, upstream={}",
        response.requested_model, response.upstream_model
    );
    for result in &response.results {
        println!(
            "{:?}: {:?}/{:?} status={:?} items={} image_call={} image_result={} reason={}",
            result.case,
            result.outcome,
            result.confidence,
            result.status_code,
            result.output_items_seen,
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
            "{} {} {:?} station={} upstream={} model={} source={}",
            entry.timestamp_ms,
            entry.evidence_id,
            entry.kind,
            entry.station_name,
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

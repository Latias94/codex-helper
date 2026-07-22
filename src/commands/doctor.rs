use crate::CliResult;
use crate::config::load_config;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::doctor::{
    ConfigurationServiceStatusSnapshot, ConfigurationStatusSnapshot, DoctorLang, DoctorStatus,
    configuration_status_snapshot, run_doctor,
};
use codex_helper_core::credentials::CredentialSourceCapabilities;
use owo_colors::OwoColorize;

pub async fn handle_status_cmd(
    json: bool,
    codex: &OperatorReadModel,
    claude: &OperatorReadModel,
) -> CliResult<()> {
    let config = load_config()
        .await
        .map_err(|error| crate::CliError::Configuration(error.to_string()))?;
    let configuration = configuration_status_snapshot(&config)
        .map_err(|error| crate::CliError::Configuration(error.to_string()))?;
    if json {
        let payload = status_payload(&configuration, codex, claude);
        let text = serde_json::to_string_pretty(&payload)
            .map_err(|error| crate::CliError::Other(error.to_string()))?;
        println!("{text}");
        return Ok(());
    }

    println!("{}", "codex-helper status".bold());
    println!("{}", "===================".bold());

    print_configuration_status(&configuration);
    print_operator_status("Codex", codex);
    print_operator_status("Claude", claude);

    Ok(())
}

fn status_payload(
    configuration: &ConfigurationStatusSnapshot,
    codex: &OperatorReadModel,
    claude: &OperatorReadModel,
) -> serde_json::Value {
    serde_json::json!({
        "api_version": 1,
        "source": "operator_read_model",
        "configuration": configuration,
        "codex": codex,
        "claude": claude,
    })
}

fn print_configuration_status(configuration: &ConfigurationStatusSnapshot) {
    println!("{}", "Canonical configuration:".bold());
    println!("  version: {}", configuration.config_version);
    print_configuration_service(&configuration.codex);
    print_configuration_service(&configuration.claude);
}

fn print_configuration_service(service: &ConfigurationServiceStatusSnapshot) {
    println!("  {}:", service.service_name);
    println!(
        "    default profile: {}",
        service.default_profile.as_deref().unwrap_or("<none>")
    );
    if let Some(client_patch) = service.client_patch.as_ref() {
        println!(
            "    client patch: preset={}, responses_websocket={}, compaction={}, translate_models={}, hosted_image_generation={}",
            client_patch.preset,
            client_patch.responses_websocket,
            client_patch.compaction,
            client_patch.translate_models,
            client_patch.hosted_image_generation,
        );
    }
    if service.providers.is_empty() {
        println!("    providers: <none>");
        return;
    }
    println!("    providers:");
    for provider in &service.providers {
        let state = if provider.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let route_state = if provider.in_route_graph {
            "in-route"
        } else {
            "configured-only"
        };
        println!("      {} [{state}; {route_state}]", provider.provider_id);
        for endpoint in &provider.endpoints {
            let endpoint_state = if endpoint.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let route = endpoint
                .route_order
                .map(|order| format!("route-order={order}"))
                .unwrap_or_else(|| "configured-only".to_string());
            println!(
                "        {} [{endpoint_state}; {route}]",
                endpoint.endpoint_id
            );
        }
    }
}

fn print_operator_status(label: &str, model: &OperatorReadModel) {
    let status = match model.status {
        OperatorReadStatus::Ready => "ready".green().to_string(),
        OperatorReadStatus::Stale => "stale".yellow().to_string(),
        OperatorReadStatus::Disconnected => "disconnected".yellow().to_string(),
        OperatorReadStatus::AuthRequired => "auth_required".yellow().to_string(),
    };
    println!("{} {status}", format!("{label} runtime:").bold());

    let Some(data) = model.data.as_ref() else {
        if let Some(issue) = model.issue {
            println!("  issue: {issue:?}");
        }
        return;
    };

    println!("  captured_at_ms: {}", model.captured_at_ms);
    println!(
        "  default profile: {}",
        data.summary
            .runtime
            .default_profile
            .as_deref()
            .unwrap_or("<none>")
    );
    println!(
        "  active requests: {}, recent requests: {}, providers: {}",
        data.summary.counts.active_requests,
        data.summary.counts.recent_requests,
        data.summary.counts.providers
    );
    for provider in &data.summary.providers {
        let state = if provider.effective_enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!(
            "    {} [{state}; routable endpoints: {}/{}]",
            provider.name,
            provider.routable_endpoints,
            provider.endpoints.len()
        );
    }
}

pub async fn handle_doctor_cmd(json: bool) -> CliResult<()> {
    let report = run_doctor(
        DoctorLang::Zh,
        CredentialSourceCapabilities::platform_native(),
    )
    .await;
    if json {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|error| crate::CliError::Other(error.to_string()))?;
        println!("{text}");
        return Ok(());
    }

    println!("{}", "codex-helper doctor".bold());
    println!("{}", "===================".bold());
    if let Some(configuration) = report.configuration.as_ref() {
        print_configuration_status(configuration);
    }
    for check in report.checks {
        match check.status {
            DoctorStatus::Ok => println!("{}   {}", "[OK]".green(), check.message),
            DoctorStatus::Info => println!("{} {}", "[INFO]".cyan(), check.message),
            DoctorStatus::Warn => println!("{} {}", "[WARN]".yellow(), check.message),
            DoctorStatus::Fail => println!("{} {}", "[FAIL]".red(), check.message),
        }
    }

    Ok(())
}

/// 辅助函数：对长字符串做安全截断，供 session 输出使用。
pub fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= max_chars {
            break;
        }
        result.push(ch);
        count += 1;
    }
    if count < s.chars().count() {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HelperConfig, ProviderConfig, RouteGraphConfig};
    use crate::dashboard_core::{ApiV1OperatorSummary, OperatorReadData, OperatorRevisionBundle};

    fn configuration() -> ConfigurationStatusSnapshot {
        let mut config = HelperConfig::default();
        config.codex.default_profile = Some("work".to_string());
        config.codex.providers.insert(
            "relay".to_string(),
            ProviderConfig {
                base_url: Some("https://relay.example/v1".to_string()),
                ..ProviderConfig::default()
            },
        );
        config.codex.routing = Some(RouteGraphConfig::ordered_failover(vec![
            "relay".to_string(),
        ]));
        configuration_status_snapshot(&config).expect("configuration snapshot")
    }

    fn ready_operator_model(service_name: &str) -> OperatorReadModel {
        OperatorReadModel::ready(
            service_name,
            1_700_000_000_000,
            OperatorRevisionBundle {
                runtime_revision: 7,
                runtime_digest: "runtime-7".to_string(),
                route_digest: "route-7".to_string(),
                catalog_revision: "catalog-7".to_string(),
                pricing_revision: "pricing-7".to_string(),
                operator_pricing_revision: "operator-pricing-7".to_string(),
                policy_revision: 8,
                ledger_revision: "operator-ledger-v1:test-store:9".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: service_name.to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    credential_readiness: None,
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                routing: None,
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                quota_analytics: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                service_status: None,
                provider_balances: Vec::new(),
            },
        )
    }

    #[test]
    fn status_payload_keeps_configuration_when_both_runtimes_are_offline() {
        let payload = status_payload(
            &configuration(),
            &OperatorReadModel::disconnected("codex"),
            &OperatorReadModel::disconnected("claude"),
        );

        assert_eq!(payload["source"], "operator_read_model");
        assert_eq!(payload["configuration"]["codex"]["default_profile"], "work");
        assert_eq!(
            payload["configuration"]["codex"]["client_patch"]["preset"],
            "default"
        );
        assert_eq!(
            payload["configuration"]["codex"]["client_patch"]["hosted_image_generation"],
            "auto"
        );
        assert_eq!(
            payload["configuration"]["codex"]["providers"][0]["provider_id"],
            "relay"
        );
        assert_eq!(payload["codex"]["status"], "disconnected");
        assert_eq!(payload["claude"]["status"], "disconnected");
    }

    #[test]
    fn status_payload_keeps_configuration_as_a_separate_online_runtime_layer() {
        let payload = status_payload(
            &configuration(),
            &ready_operator_model("codex"),
            &ready_operator_model("claude"),
        );

        assert_eq!(
            payload["configuration"]["config_version"],
            crate::config::CURRENT_CONFIG_VERSION
        );
        assert_eq!(payload["codex"]["status"], "ready");
        assert_eq!(payload["claude"]["status"], "ready");
        assert_eq!(payload["codex"]["data"]["summary"]["service_name"], "codex");
    }
}

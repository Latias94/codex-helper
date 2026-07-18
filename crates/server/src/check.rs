use std::fmt::Write as _;

use anyhow::Result;
use codex_helper_core::config::{HelperConfig, ServiceKind};
use codex_helper_core::credentials::{
    CredentialAggregateReadiness, CredentialReadinessCode, CredentialSourceCapabilities,
};
use codex_helper_core::service_readiness::{
    ServiceCredentialReadinessReport, evaluate_service_credential_readiness_without_runtime_store,
};
use serde::Serialize;

const CHECK_SCHEMA_VERSION: u32 = 1;
const NATIVE_GUIDANCE: &str = "native credentials are unavailable in codex-helper-server; use an environment variable (*_env) or an absolute secret_file reference";

#[derive(Debug)]
pub(crate) struct CheckEvaluation {
    report: ServiceCredentialReadinessReport,
    guidance: Vec<&'static str>,
}

#[derive(Serialize)]
struct CheckJson<'a> {
    schema_version: u32,
    #[serde(flatten)]
    report: &'a ServiceCredentialReadinessReport,
    guidance: &'a [&'static str],
}

impl CheckEvaluation {
    pub(crate) fn succeeded(&self) -> bool {
        self.report.aggregate != CredentialAggregateReadiness::Blocked
    }

    pub(crate) fn render_text(&self) -> String {
        let mut output = String::new();
        writeln!(
            output,
            "credential check: service={} readiness={}",
            service_name(self.report.service),
            self.report.aggregate.as_str()
        )
        .expect("writing to String cannot fail");
        for endpoint in &self.report.endpoints {
            writeln!(
                output,
                "endpoint provider={} endpoint={} readiness={}",
                quoted(&endpoint.provider_id),
                quoted(&endpoint.endpoint_id),
                endpoint.code.as_str()
            )
            .expect("writing to String cannot fail");
            for detail in &endpoint.details {
                let kind = detail.kind.map_or("unknown", |kind| kind.as_str());
                let source = detail.source_kind.as_deref().unwrap_or("unknown");
                let reference = detail
                    .reference
                    .as_deref()
                    .map_or_else(|| "<none>".to_string(), quoted);
                writeln!(
                    output,
                    "  credential kind={kind} source={source} reference={reference} readiness={}",
                    detail.code.as_str()
                )
                .expect("writing to String cannot fail");
            }
        }
        for guidance in &self.guidance {
            writeln!(output, "guidance: {guidance}").expect("writing to String cannot fail");
        }
        output.pop();
        output
    }

    pub(crate) fn render_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&CheckJson {
            schema_version: CHECK_SCHEMA_VERSION,
            report: &self.report,
            guidance: &self.guidance,
        })?)
    }
}

pub(crate) fn evaluate(config: &HelperConfig, service: ServiceKind) -> Result<CheckEvaluation> {
    let report = evaluate_service_credential_readiness_without_runtime_store(
        config,
        service,
        CredentialSourceCapabilities::server(),
    )?;
    let mut guidance = Vec::new();
    if report.endpoints.iter().any(|endpoint| {
        endpoint.details.iter().any(|detail| {
            detail.code == CredentialReadinessCode::Unsupported
                && detail.source_kind.as_deref() == Some("native")
        })
    }) {
        guidance.push(NATIVE_GUIDANCE);
    }
    Ok(CheckEvaluation { report, guidance })
}

fn service_name(service: ServiceKind) -> &'static str {
    match service {
        ServiceKind::Codex => "codex",
        ServiceKind::Claude => "claude",
    }
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::ErrorKind;
    use std::net::TcpListener;

    use codex_helper_core::config::{
        CredentialRef, ProviderConfig, RouteGraphConfig, ServiceRouteConfig, UpstreamAuth,
    };

    use super::*;

    fn config_with_providers(providers: Vec<(&str, String, UpstreamAuth)>) -> HelperConfig {
        let order = providers
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        let providers = providers
            .into_iter()
            .map(|(name, base_url, auth)| {
                (
                    name.to_string(),
                    ProviderConfig {
                        base_url: Some(base_url),
                        auth,
                        ..ProviderConfig::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        HelperConfig {
            codex: ServiceRouteConfig {
                providers,
                routing: Some(RouteGraphConfig::ordered_failover(order)),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    #[test]
    fn check_resolves_environment_and_file_without_contacting_upstream() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream sentinel");
        listener
            .set_nonblocking(true)
            .expect("make upstream sentinel nonblocking");
        let upstream = format!("http://{}/v1", listener.local_addr().unwrap());
        let environment = unique_environment_name("CHECK");
        // SAFETY: the variable name is unique to this test and is removed before returning.
        unsafe { std::env::set_var(&environment, "environment-check-token") };
        let secret_dir = tempfile::tempdir().expect("create secret directory");
        let secret_path = secret_dir.path().join("provider-token");
        std::fs::write(&secret_path, "file-check-token\n").expect("write mounted secret fixture");
        let config = config_with_providers(vec![
            (
                "environment",
                upstream.clone(),
                UpstreamAuth {
                    auth_token_env: Some(environment.clone()),
                    ..UpstreamAuth::default()
                },
            ),
            (
                "file",
                upstream,
                UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::SecretFile {
                        path: secret_path.to_string_lossy().into_owned(),
                    }),
                    ..UpstreamAuth::default()
                },
            ),
        ]);

        let evaluation = evaluate(&config, ServiceKind::Codex).expect("evaluate check");
        // SAFETY: this removes only the unique variable created above.
        unsafe { std::env::remove_var(&environment) };

        assert!(evaluation.succeeded());
        assert!(evaluation.render_text().contains("readiness=ready"));
        let error = listener
            .accept()
            .expect_err("offline check must not contact configured upstream");
        assert_eq!(error.kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn check_reports_degraded_and_blocked_outcomes() {
        let secret_dir = tempfile::tempdir().expect("create secret directory");
        let secret_path = secret_dir.path().join("provider-token");
        std::fs::write(&secret_path, "file-check-token").expect("write mounted secret fixture");
        let missing = unique_environment_name("MISSING");
        let ready = UpstreamAuth {
            auth_token_ref: Some(CredentialRef::SecretFile {
                path: secret_path.to_string_lossy().into_owned(),
            }),
            ..UpstreamAuth::default()
        };
        let unavailable = UpstreamAuth {
            auth_token_env: Some(missing),
            ..UpstreamAuth::default()
        };

        let degraded = evaluate(
            &config_with_providers(vec![
                ("ready", "https://ready.example/v1".to_string(), ready),
                (
                    "missing",
                    "https://missing.example/v1".to_string(),
                    unavailable.clone(),
                ),
            ]),
            ServiceKind::Codex,
        )
        .expect("evaluate degraded check");
        assert!(degraded.succeeded());
        assert!(degraded.render_text().contains("readiness=degraded"));

        let blocked = evaluate(
            &config_with_providers(vec![(
                "missing",
                "https://missing.example/v1".to_string(),
                unavailable,
            )]),
            ServiceKind::Codex,
        )
        .expect("evaluate blocked check");
        assert!(!blocked.succeeded());
        assert!(blocked.render_text().contains("readiness=blocked"));
    }

    #[test]
    fn native_reference_is_unsupported_with_safe_guidance() {
        let reference = "relay.primary";
        let evaluation = evaluate(
            &config_with_providers(vec![(
                "native\nprovider",
                "https://native.example/v1".to_string(),
                UpstreamAuth {
                    auth_token_ref: Some(CredentialRef::Native {
                        name: reference.to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
            )]),
            ServiceKind::Codex,
        )
        .expect("evaluate native reference");

        assert!(!evaluation.succeeded());
        let text = evaluation.render_text();
        assert!(text.contains("readiness=unsupported"));
        assert!(text.contains("environment variable"));
        assert!(text.contains("secret_file"));
        assert!(text.contains("native\\nprovider"));
        assert!(!text.contains("native\nprovider"));

        let json: serde_json::Value =
            serde_json::from_str(&evaluation.render_json().unwrap()).expect("parse check JSON");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["aggregate"], "blocked");
        assert_eq!(json["endpoints"][0]["details"][0]["reference"], reference);
        assert_eq!(json["guidance"].as_array().unwrap().len(), 1);
    }

    fn unique_environment_name(label: &str) -> String {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after Unix epoch")
            .as_nanos();
        format!("CODEX_HELPER_SERVER_{label}_{}_{nonce}", std::process::id())
    }
}

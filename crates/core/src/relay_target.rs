use anyhow::{Result, anyhow};

use crate::codex_integration::{CodexPatchMode, CodexSwitchOptions};
use crate::config::{ProxyConfig, RelayTargetConfig, ServiceKind};
use crate::control_plane_client::normalize_base_url;
use crate::proxy::{
    admin_base_url_from_proxy_base_url, local_admin_base_url_for_proxy_port, local_proxy_base_url,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRelayTarget {
    pub name: String,
    pub service: ServiceKind,
    pub proxy_url: String,
    pub admin_url: Option<String>,
    pub admin_token_env: Option<String>,
    pub client_preset: CodexPatchMode,
    pub client_options: CodexSwitchOptions,
    pub built_in_local: bool,
}

pub fn default_proxy_port_for_service_kind(service: ServiceKind) -> u16 {
    match service {
        ServiceKind::Codex => 3211,
        ServiceKind::Claude => 3210,
    }
}

impl ResolvedRelayTarget {
    pub fn is_local(&self) -> bool {
        self.built_in_local
    }
}

pub fn resolve_relay_target(cfg: &ProxyConfig, name: &str) -> Result<ResolvedRelayTarget> {
    let name = normalize_relay_target_name(name)?;
    if name == "local" {
        let service = cfg.default_service.unwrap_or(ServiceKind::Codex);
        let port = default_proxy_port_for_service_kind(service);
        return Ok(ResolvedRelayTarget {
            name,
            service,
            proxy_url: local_proxy_base_url(port),
            admin_url: Some(local_admin_base_url_for_proxy_port(port)),
            admin_token_env: None,
            client_preset: cfg
                .relay_targets
                .get("local")
                .and_then(|target| target.client_preset)
                .unwrap_or(CodexPatchMode::Default),
            client_options: CodexSwitchOptions {
                responses_websocket: cfg
                    .relay_targets
                    .get("local")
                    .and_then(|target| target.responses_websocket)
                    .unwrap_or(false),
            },
            built_in_local: true,
        });
    }

    let target = cfg
        .relay_targets
        .get(&name)
        .ok_or_else(|| anyhow!("relay target '{name}' is not configured"))?;
    resolve_configured_relay_target(&name, target)
}

pub fn relay_target_names(cfg: &ProxyConfig) -> Vec<String> {
    let mut names = vec!["local".to_string()];
    for name in cfg.relay_targets.keys() {
        if name != "local" {
            names.push(name.clone());
        }
    }
    names
}

pub fn relay_target_config_from_args(
    service: Option<ServiceKind>,
    proxy_url: String,
    admin_url: Option<String>,
    admin_token_env: Option<String>,
    client_preset: Option<CodexPatchMode>,
    responses_websocket: Option<bool>,
) -> Result<RelayTargetConfig> {
    let proxy_url = normalize_base_url(&proxy_url)
        .ok_or_else(|| anyhow!("relay proxy URL must start with http:// or https://"))?;
    let admin_url = match admin_url {
        Some(url) => Some(
            normalize_base_url(&url)
                .ok_or_else(|| anyhow!("relay admin URL must start with http:// or https://"))?,
        ),
        None => admin_base_url_from_proxy_base_url(&proxy_url),
    };
    Ok(RelayTargetConfig {
        service,
        proxy_url,
        admin_url,
        admin_token_env: admin_token_env
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        client_preset,
        responses_websocket,
    })
}

fn resolve_configured_relay_target(
    name: &str,
    target: &RelayTargetConfig,
) -> Result<ResolvedRelayTarget> {
    let proxy_url = normalize_base_url(&target.proxy_url)
        .ok_or_else(|| anyhow!("relay target '{name}' has an invalid proxy_url"))?;
    let admin_url = match target.admin_url.as_deref() {
        Some(url) => Some(
            normalize_base_url(url)
                .ok_or_else(|| anyhow!("relay target '{name}' has an invalid admin_url"))?,
        ),
        None => admin_base_url_from_proxy_base_url(&proxy_url),
    };
    Ok(ResolvedRelayTarget {
        name: name.to_string(),
        service: target.service.unwrap_or(ServiceKind::Codex),
        proxy_url,
        admin_url,
        admin_token_env: target.admin_token_env.clone(),
        client_preset: target.client_preset.unwrap_or(CodexPatchMode::Default),
        client_options: CodexSwitchOptions {
            responses_websocket: target.responses_websocket.unwrap_or(false),
        },
        built_in_local: false,
    })
}

pub fn normalize_relay_target_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("relay target name is required"));
    }
    if name
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
    {
        return Err(anyhow!(
            "relay target name may contain only ASCII letters, numbers, '-' and '_'"
        ));
    }
    Ok(name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn relay_target_resolves_builtin_local_from_default_service() {
        let cfg = ProxyConfig {
            default_service: Some(ServiceKind::Codex),
            ..ProxyConfig::default()
        };

        let target = resolve_relay_target(&cfg, "local").expect("local target");

        assert_eq!(target.proxy_url, "http://127.0.0.1:3211");
        assert_eq!(target.admin_url.as_deref(), Some("http://127.0.0.1:4211"));
        assert!(target.is_local());
    }

    #[test]
    fn relay_target_resolves_named_remote() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "nas".to_string(),
            RelayTargetConfig {
                service: Some(ServiceKind::Codex),
                proxy_url: "http://nas.local:3211/".to_string(),
                admin_url: Some("http://nas.local:4211/".to_string()),
                admin_token_env: Some("NAS_TOKEN".to_string()),
                client_preset: Some(CodexPatchMode::ChatGptBridge),
                responses_websocket: Some(true),
            },
        );
        let cfg = ProxyConfig {
            relay_targets: targets,
            ..ProxyConfig::default()
        };

        let target = resolve_relay_target(&cfg, "nas").expect("nas target");

        assert_eq!(target.proxy_url, "http://nas.local:3211");
        assert_eq!(target.admin_url.as_deref(), Some("http://nas.local:4211"));
        assert_eq!(target.admin_token_env.as_deref(), Some("NAS_TOKEN"));
        assert_eq!(target.client_preset, CodexPatchMode::ChatGptBridge);
        assert!(target.client_options.responses_websocket);
        assert!(!target.is_local());
    }

    #[test]
    fn relay_target_name_rejects_whitespace() {
        let err = normalize_relay_target_name("nas home")
            .expect_err("target names with spaces should be rejected");

        assert!(err.to_string().contains("may contain only ASCII"));
    }
}

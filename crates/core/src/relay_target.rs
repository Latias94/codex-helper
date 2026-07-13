use anyhow::{Context, Result, anyhow};

use crate::config::{HelperConfig, RelayTargetConfig, ServiceKind};
use crate::control_plane_client::{
    configured_local_admin_token_env, is_loopback_control_plane_base_url,
    normalize_admin_token_env, normalize_base_url, normalize_control_plane_base_url,
    require_remote_admin_token,
};
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

pub fn resolve_relay_target(cfg: &HelperConfig, name: &str) -> Result<ResolvedRelayTarget> {
    let name = normalize_relay_target_name(name)?;
    if name == "local" {
        let service = cfg.default_service.unwrap_or(ServiceKind::Codex);
        let port = default_proxy_port_for_service_kind(service);
        return Ok(ResolvedRelayTarget {
            name,
            service,
            proxy_url: local_proxy_base_url(port),
            admin_url: Some(local_admin_base_url_for_proxy_port(port)),
            admin_token_env: configured_local_admin_token_env().map(str::to_string),
            built_in_local: true,
        });
    }

    let target = cfg
        .relay_targets
        .get(&name)
        .ok_or_else(|| anyhow!("relay target '{name}' is not configured"))?;
    resolve_configured_relay_target(&name, target)
}

pub fn relay_target_names(cfg: &HelperConfig) -> Vec<String> {
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
) -> Result<RelayTargetConfig> {
    let proxy_url = normalize_base_url(&proxy_url)
        .ok_or_else(|| anyhow!("relay proxy URL must start with http:// or https://"))?;
    let admin_url = match admin_url {
        Some(url) => Some(normalize_control_plane_base_url(&url)?),
        None if is_loopback_control_plane_base_url(&proxy_url) => {
            admin_base_url_from_proxy_base_url(&proxy_url)
                .map(|url| normalize_control_plane_base_url(&url))
                .transpose()?
        }
        None => anyhow::bail!("remote relay proxy URL requires an explicit admin_url"),
    };
    let admin_token_env = normalize_admin_token_env(admin_token_env.as_deref())?;
    if let Some(admin_url) = admin_url.as_deref() {
        require_remote_admin_token(admin_url, admin_token_env.as_deref())?;
    }
    Ok(RelayTargetConfig {
        service,
        proxy_url,
        admin_url,
        admin_token_env,
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
            normalize_control_plane_base_url(url)
                .with_context(|| format!("relay target '{name}' has an invalid admin_url"))?,
        ),
        None if is_loopback_control_plane_base_url(&proxy_url) => {
            admin_base_url_from_proxy_base_url(&proxy_url)
                .map(|url| {
                    normalize_control_plane_base_url(&url).with_context(|| {
                        format!("relay target '{name}' has an invalid derived admin_url")
                    })
                })
                .transpose()?
        }
        None => {
            anyhow::bail!("relay target '{name}' remote proxy URL requires an explicit admin_url")
        }
    };
    let admin_token_env = normalize_admin_token_env(target.admin_token_env.as_deref())
        .with_context(|| format!("relay target '{name}' has an invalid admin_token_env"))?;
    if let Some(admin_url) = admin_url.as_deref() {
        require_remote_admin_token(admin_url, admin_token_env.as_deref())
            .with_context(|| format!("relay target '{name}' requires a valid admin_token_env"))?;
    }
    Ok(ResolvedRelayTarget {
        name: name.to_string(),
        service: target.service.unwrap_or(ServiceKind::Codex),
        proxy_url,
        admin_url,
        admin_token_env,
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
        let cfg = HelperConfig {
            default_service: Some(ServiceKind::Codex),
            ..HelperConfig::default()
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
                admin_url: Some("https://nas.example:4211/".to_string()),
                admin_token_env: Some("NAS_TOKEN".to_string()),
            },
        );
        let cfg = HelperConfig {
            relay_targets: targets,
            ..HelperConfig::default()
        };

        let target = resolve_relay_target(&cfg, "nas").expect("nas target");

        assert_eq!(target.proxy_url, "http://nas.local:3211");
        assert_eq!(
            target.admin_url.as_deref(),
            Some("https://nas.example:4211")
        );
        assert_eq!(target.admin_token_env.as_deref(), Some("NAS_TOKEN"));
        assert!(!target.is_local());
    }

    #[test]
    fn relay_target_rejects_remote_http_admin_url() {
        let err = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "http://nas.example:3211".to_string(),
            Some("http://nas.example:4211".to_string()),
            Some("NAS_TOKEN".to_string()),
        )
        .expect_err("remote admin HTTP must be rejected");

        assert!(err.to_string().contains("HTTPS"), "unexpected: {err}");
    }

    #[test]
    fn relay_target_rejects_explicit_remote_admin_without_token() {
        let err = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "http://127.0.0.1:3211".to_string(),
            Some("https://relay.example:4211".to_string()),
            None,
        )
        .expect_err("explicit remote admin URL must require a token environment variable");

        assert!(
            err.to_string().contains("admin_token_env"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_rejects_remote_proxy_without_explicit_admin_url() {
        let err = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "https://relay.example:3211".to_string(),
            None,
            Some("RELAY_ADMIN_TOKEN".to_string()),
        )
        .expect_err("remote proxy must require an explicit admin authority");

        assert!(
            err.to_string().contains("explicit admin_url"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_resolution_rejects_remote_proxy_without_explicit_admin_url() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "nas".to_string(),
            RelayTargetConfig {
                proxy_url: "https://relay.example:3211".to_string(),
                admin_token_env: Some("RELAY_ADMIN_TOKEN".to_string()),
                ..RelayTargetConfig::default()
            },
        );
        let cfg = HelperConfig {
            relay_targets: targets,
            ..HelperConfig::default()
        };

        let err = resolve_relay_target(&cfg, "nas")
            .expect_err("loaded remote relay target must pin its admin authority");

        assert!(
            err.to_string().contains("explicit admin_url"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_rejects_discovered_remote_admin_without_token() {
        let discovered_admin_url = "https://relay.example:4211".to_string();
        let err = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "https://relay.example:3211".to_string(),
            Some(discovered_admin_url),
            None,
        )
        .expect_err("discovered remote admin URL must require a token environment variable");

        assert!(
            err.to_string().contains("admin_token_env"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_rejects_invalid_remote_admin_token_env() {
        let err = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "https://relay.example:3211".to_string(),
            Some("https://relay.example:4211".to_string()),
            Some("Bearer token".to_string()),
        )
        .expect_err("remote admin token must name an environment variable");

        assert!(
            err.to_string().contains("valid environment variable name"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_accepts_remote_admin_with_valid_token_env() {
        let target = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "https://relay.example:3211".to_string(),
            Some("https://relay.example:4211".to_string()),
            Some(" RELAY_ADMIN_TOKEN ".to_string()),
        )
        .expect("remote HTTPS admin with a valid token environment variable");

        assert_eq!(
            target.admin_url.as_deref(),
            Some("https://relay.example:4211")
        );
        assert_eq!(target.admin_token_env.as_deref(), Some("RELAY_ADMIN_TOKEN"));
    }

    #[test]
    fn relay_target_resolution_revalidates_remote_admin_token() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "nas".to_string(),
            RelayTargetConfig {
                proxy_url: "https://relay.example:3211".to_string(),
                admin_url: Some("https://relay.example:4211".to_string()),
                ..RelayTargetConfig::default()
            },
        );
        let cfg = HelperConfig {
            relay_targets: targets,
            ..HelperConfig::default()
        };

        let err = resolve_relay_target(&cfg, "nas")
            .expect_err("loaded relay config must enforce the remote token boundary");

        assert!(
            err.to_string().contains("admin_token_env"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn relay_target_allows_loopback_admin_without_token() {
        let target = relay_target_config_from_args(
            Some(ServiceKind::Codex),
            "http://127.0.0.1:3211".to_string(),
            None,
            None,
        )
        .expect("loopback relay admin must not require a token");

        assert_eq!(target.admin_url.as_deref(), Some("http://127.0.0.1:4211"));
        assert!(target.admin_token_env.is_none());
    }

    #[test]
    fn relay_target_name_rejects_whitespace() {
        let err = normalize_relay_target_name("nas home")
            .expect_err("target names with spaces should be rejected");

        assert!(err.to_string().contains("may contain only ASCII"));
    }
}

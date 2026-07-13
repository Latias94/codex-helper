use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::control_plane_client::ControlPlaneEndpoint;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct FleetNodeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admin_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_token_env: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

impl FleetNodeConfig {
    pub fn endpoints(&self) -> Vec<&str> {
        self.admin_url
            .as_deref()
            .into_iter()
            .chain(self.admin_urls.iter().map(String::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect()
    }

    pub fn display_label(&self, id: &str) -> String {
        self.label
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(id)
            .to_string()
    }

    pub fn validate(&self, node_id: &str) -> Result<()> {
        let endpoints = self.endpoints();
        if self.enabled && endpoints.is_empty() {
            bail!("fleet node '{node_id}' is enabled but has no admin_url or admin_urls");
        }

        if let Some(env_name) = self.admin_token_env.as_deref() {
            validate_env_var_name(env_name).with_context(|| {
                format!("fleet node '{node_id}' admin_token_env must be a valid environment variable name")
            })?;
        }

        for endpoint_url in endpoints {
            let endpoint =
                ControlPlaneEndpoint::new(endpoint_url, self.admin_token_env.as_deref())?;
            validate_endpoint_auth(
                node_id,
                self.enabled,
                self.admin_token_env.as_deref(),
                &endpoint,
            )?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FleetRegistryConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nodes: BTreeMap<String, FleetNodeConfig>,
}

impl FleetRegistryConfig {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn enabled_nodes(&self) -> impl Iterator<Item = (&String, &FleetNodeConfig)> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.enabled && !node.endpoints().is_empty())
    }

    pub fn validate(&self) -> Result<()> {
        for (node_id, node) in &self.nodes {
            node.validate(node_id)?;
        }
        Ok(())
    }
}

fn validate_endpoint_auth(
    node_id: &str,
    enabled: bool,
    admin_token_env: Option<&str>,
    endpoint: &ControlPlaneEndpoint,
) -> Result<()> {
    let url = Url::parse(endpoint.admin_base_url())
        .with_context(|| format!("fleet node '{node_id}' has invalid admin_url"))?;
    if is_loopback_host(&url) {
        return Ok(());
    }

    if !enabled {
        return Ok(());
    }

    if admin_token_env.is_none() {
        bail!(
            "fleet node '{node_id}' points to a non-loopback admin_url but does not set admin_token_env; use HTTPS or a trusted encrypted tunnel and configure admin_token_env"
        );
    }

    Ok(())
}

fn validate_env_var_name(value: &str) -> Result<()> {
    let value = value.trim();
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        bail!("environment variable name is empty");
    };
    if !(first == '_' || first.is_ascii_uppercase()) {
        bail!("environment variable name must start with an ASCII uppercase letter or underscore");
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit()) {
        bail!(
            "environment variable name must contain only ASCII uppercase letters, digits, or underscores"
        );
    }
    Ok(())
}

fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost"))
        || url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|addr| addr.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_registry_accepts_loopback_admin_url_without_token_env() {
        let cfg = FleetRegistryConfig {
            nodes: BTreeMap::from([(
                "local".to_string(),
                FleetNodeConfig {
                    label: Some("Local".to_string()),
                    admin_url: Some("http://127.0.0.1:4211".to_string()),
                    admin_urls: Vec::new(),
                    admin_token_env: None,
                    enabled: true,
                },
            )]),
        };

        cfg.validate().expect("loopback fleet node should validate");
    }

    #[test]
    fn fleet_registry_rejects_remote_http_without_token_env() {
        let cfg = FleetRegistryConfig {
            nodes: BTreeMap::from([(
                "remote".to_string(),
                FleetNodeConfig {
                    label: Some("Remote".to_string()),
                    admin_url: Some("http://nas.example.com:4211".to_string()),
                    admin_urls: Vec::new(),
                    admin_token_env: None,
                    enabled: true,
                },
            )]),
        };

        let err = cfg
            .validate()
            .expect_err("remote fleet node should require token auth");
        assert!(err.to_string().contains("HTTPS"), "unexpected: {err}");
    }

    #[test]
    fn fleet_registry_rejects_remote_http_with_token_env() {
        let cfg = FleetRegistryConfig {
            nodes: BTreeMap::from([(
                "remote".to_string(),
                FleetNodeConfig {
                    label: Some("Remote".to_string()),
                    admin_url: Some("http://nas.example.com:4211".to_string()),
                    admin_urls: Vec::new(),
                    admin_token_env: Some("NAS_TOKEN".to_string()),
                    enabled: true,
                },
            )]),
        };

        let err = cfg
            .validate()
            .expect_err("token auth must not make remote HTTP trustworthy");
        assert!(err.to_string().contains("HTTPS"), "unexpected: {err}");
    }

    #[test]
    fn fleet_registry_rejects_invalid_admin_token_env_name() {
        let cfg = FleetRegistryConfig {
            nodes: BTreeMap::from([(
                "remote".to_string(),
                FleetNodeConfig {
                    label: Some("Remote".to_string()),
                    admin_url: Some("https://nas.example.com:4211".to_string()),
                    admin_urls: Vec::new(),
                    admin_token_env: Some("Bearer abc".to_string()),
                    enabled: true,
                },
            )]),
        };

        let err = cfg
            .validate()
            .expect_err("token env should reject raw secret-like values");
        assert!(err.to_string().contains("valid environment variable name"));
    }
}

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use codex_helper_core::config::ServiceKind;
use codex_helper_core::host_local::HostLocalSessionHistoryMode;
use codex_helper_core::proxy::{ADMIN_TOKEN_ENV_VAR, admin_port_for_proxy_port};
use codex_helper_core::runtime_host::ProxyRuntimeOptions;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ServerService {
    Codex,
    Claude,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ServerConfigSection {
    pub service: Option<ServerService>,
    pub host: Option<IpAddr>,
    pub port: Option<u16>,
    pub admin_host: Option<IpAddr>,
    pub admin_port: Option<u16>,
    pub advertised_admin_base_url: Option<String>,
    pub host_local_session_history: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct ServerConfigOverrides {
    pub service: Option<ServerService>,
    pub host: Option<IpAddr>,
    pub port: Option<u16>,
    pub admin_host: Option<IpAddr>,
    pub admin_port: Option<u16>,
    pub advertised_admin_base_url: Option<String>,
    pub host_local_session_history: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct EffectiveServerConfig {
    pub service: ServerService,
    pub host: IpAddr,
    pub port: u16,
    pub admin_host: IpAddr,
    pub admin_port: u16,
    pub advertised_admin_base_url: Option<String>,
    pub host_local_session_history: bool,
}

impl EffectiveServerConfig {
    pub fn from_sources(
        file: ServerConfigSection,
        overrides: ServerConfigOverrides,
    ) -> Result<Self> {
        let service = overrides
            .service
            .or(file.service)
            .unwrap_or(ServerService::Codex);
        let service_kind = ServiceKind::from(service);
        let host = overrides
            .host
            .or(file.host)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let port = overrides
            .port
            .or(file.port)
            .unwrap_or_else(|| default_proxy_port_for_service(service_kind));
        let admin_host = overrides
            .admin_host
            .or(file.admin_host)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let admin_port = overrides
            .admin_port
            .or(file.admin_port)
            .unwrap_or_else(|| admin_port_for_proxy_port(port));
        let advertised_admin_base_url = normalize_optional_url(
            overrides
                .advertised_admin_base_url
                .or(file.advertised_admin_base_url),
        );
        let host_local_session_history = overrides
            .host_local_session_history
            .or(file.host_local_session_history)
            .unwrap_or(false);

        let effective = Self {
            service,
            host,
            port,
            admin_host,
            admin_port,
            advertised_admin_base_url,
            host_local_session_history,
        };
        effective.validate()?;
        Ok(effective)
    }

    pub fn service_kind(&self) -> ServiceKind {
        ServiceKind::from(self.service)
    }

    pub fn admin_addr(&self) -> SocketAddr {
        SocketAddr::from((self.admin_host, self.admin_port))
    }

    pub fn runtime_options(&self) -> ProxyRuntimeOptions {
        ProxyRuntimeOptions::for_proxy_port(self.port)
            .with_admin_addr(self.admin_addr())
            .with_advertised_admin_base_url(self.advertised_admin_base_url.clone())
            .with_host_local_session_history_mode(if self.host_local_session_history {
                HostLocalSessionHistoryMode::Enabled
            } else {
                HostLocalSessionHistoryMode::Disabled
            })
    }

    fn validate(&self) -> Result<()> {
        if !self.admin_host.is_loopback() && !admin_token_configured() {
            bail!(
                "admin host {} is not loopback; set {} before exposing the admin API",
                self.admin_host,
                ADMIN_TOKEN_ENV_VAR
            );
        }
        if let Some(url) = self.advertised_admin_base_url.as_deref()
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            bail!("advertised-admin-base-url must start with http:// or https://");
        }
        Ok(())
    }
}

impl From<ServerService> for ServiceKind {
    fn from(value: ServerService) -> Self {
        match value {
            ServerService::Codex => Self::Codex,
            ServerService::Claude => Self::Claude,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct ServerConfigFile {
    server: ServerConfigSection,
}

pub fn load_server_config(path: Option<&Path>) -> Result<ServerConfigSection> {
    match path {
        Some(path) => {
            let contents = std::fs::read_to_string(path)
                .with_context(|| format!("read server config {}", path.display()))?;
            parse_server_config(&contents)
                .with_context(|| format!("parse server config {}", path.display()))
        }
        None => Ok(ServerConfigSection::default()),
    }
}

fn parse_server_config(contents: &str) -> Result<ServerConfigSection> {
    let file: ServerConfigFile = toml::from_str(contents)?;
    Ok(file.server)
}

fn default_proxy_port_for_service(service_kind: ServiceKind) -> u16 {
    match service_kind {
        ServiceKind::Codex => 3211,
        ServiceKind::Claude => 3210,
    }
}

fn normalize_optional_url(url: Option<String>) -> Option<String> {
    url.map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

fn admin_token_configured() -> bool {
    std::env::var(ADMIN_TOKEN_ENV_VAR)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_section() {
        let config = parse_server_config(
            r#"
            [server]
            service = "codex"
            host = "0.0.0.0"
            port = 3211
            admin-host = "127.0.0.1"
            admin-port = 4211
            advertised-admin-base-url = "http://nas.local:4211/"
            host-local-session-history = false
            "#,
        )
        .expect("parse server config");

        assert_eq!(config.service, Some(ServerService::Codex));
        assert_eq!(config.host, Some("0.0.0.0".parse().unwrap()));
        assert_eq!(config.port, Some(3211));
        assert_eq!(config.admin_host, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(config.admin_port, Some(4211));
        assert_eq!(
            config.advertised_admin_base_url.as_deref(),
            Some("http://nas.local:4211/")
        );
        assert_eq!(config.host_local_session_history, Some(false));
    }

    #[test]
    fn effective_config_merges_cli_overrides_and_normalizes_admin_url() {
        let config = EffectiveServerConfig::from_sources(
            ServerConfigSection {
                service: Some(ServerService::Claude),
                advertised_admin_base_url: Some("http://nas.local:4211/".to_string()),
                ..ServerConfigSection::default()
            },
            ServerConfigOverrides {
                service: Some(ServerService::Codex),
                port: Some(3211),
                ..ServerConfigOverrides::default()
            },
        )
        .expect("effective config");

        assert_eq!(config.service, ServerService::Codex);
        assert_eq!(config.port, 3211);
        assert_eq!(
            config.advertised_admin_base_url.as_deref(),
            Some("http://nas.local:4211")
        );
    }

    #[test]
    fn effective_config_rejects_remote_admin_without_token() {
        let old = std::env::var(ADMIN_TOKEN_ENV_VAR).ok();
        unsafe {
            std::env::remove_var(ADMIN_TOKEN_ENV_VAR);
        }

        let result = EffectiveServerConfig::from_sources(
            ServerConfigSection::default(),
            ServerConfigOverrides {
                admin_host: Some("0.0.0.0".parse().unwrap()),
                ..ServerConfigOverrides::default()
            },
        );

        if let Some(old) = old {
            unsafe {
                std::env::set_var(ADMIN_TOKEN_ENV_VAR, old);
            }
        }
        assert!(result.is_err());
    }
}

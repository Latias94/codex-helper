use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use codex_helper_core::config::ServiceKind;
use codex_helper_core::control_plane_client::validate_admin_token_header_value;
use codex_helper_core::proxy::{ADMIN_TOKEN_ENV_VAR, admin_port_for_proxy_port};
use codex_helper_core::runtime_host::ProxyRuntimeOptions;
use serde::Deserialize;
use toml::Value as TomlValue;

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
}

#[derive(Debug, Clone, Default)]
pub struct ServerConfigOverrides {
    pub service: Option<ServerService>,
    pub host: Option<IpAddr>,
    pub port: Option<u16>,
    pub admin_host: Option<IpAddr>,
    pub admin_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct EffectiveServerConfig {
    pub service: ServerService,
    pub host: IpAddr,
    pub port: u16,
    pub admin_host: IpAddr,
    pub admin_port: u16,
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
        let effective = Self {
            service,
            host,
            port,
            admin_host,
            admin_port,
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
        ProxyRuntimeOptions::for_proxy_port(self.port).with_admin_addr(self.admin_addr())
    }

    fn validate(&self) -> Result<()> {
        if !self.admin_host.is_loopback() {
            validate_configured_admin_token().with_context(|| {
                format!(
                    "admin host {} is not loopback; configure {} before exposing the admin API",
                    self.admin_host, ADMIN_TOKEN_ENV_VAR
                )
            })?;
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
    let raw: TomlValue = toml::from_str(contents)?;
    reject_retired_server_settings(&raw)?;
    let file: ServerConfigFile = toml::from_str(contents)?;
    Ok(file.server)
}

fn reject_retired_server_settings(value: &TomlValue) -> Result<()> {
    let Some(server) = value.get("server").and_then(TomlValue::as_table) else {
        return Ok(());
    };
    let mut retired = ["advertised-admin-base-url", "host-local-session-history"]
        .into_iter()
        .filter(|field| server.contains_key(*field))
        .map(|field| format!("server.{field}"))
        .collect::<Vec<_>>();
    if retired.is_empty() {
        return Ok(());
    }
    retired.sort();
    let labels = retired
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "server config contains retired setting(s): {labels}. Each listed setting has been removed. Remove every listed setting before retrying; configure trusted relay admin_url values on each client, and keep client-local session history on the client"
    )
}

fn default_proxy_port_for_service(service_kind: ServiceKind) -> u16 {
    match service_kind {
        ServiceKind::Codex => 3211,
        ServiceKind::Claude => 3210,
    }
}

fn validate_configured_admin_token() -> Result<()> {
    let value = std::env::var(ADMIN_TOKEN_ENV_VAR)
        .with_context(|| format!("{ADMIN_TOKEN_ENV_VAR} is missing or not valid Unicode"))?;
    let value = value.trim();
    if value.is_empty() {
        bail!("{ADMIN_TOKEN_ENV_VAR} is empty");
    }
    validate_admin_token_header_value(value)
        .with_context(|| format!("{ADMIN_TOKEN_ENV_VAR} is not a valid HTTP header value"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ScopedEnv {
        _lock: MutexGuard<'static, ()>,
        previous: Option<OsString>,
    }

    impl ScopedEnv {
        fn set(value: &str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os(ADMIN_TOKEN_ENV_VAR);
            // SAFETY: this guard serializes this module's test-only environment mutations.
            unsafe { std::env::set_var(ADMIN_TOKEN_ENV_VAR, value) };
            Self {
                _lock: lock,
                previous,
            }
        }

        fn remove() -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os(ADMIN_TOKEN_ENV_VAR);
            // SAFETY: this guard serializes this module's test-only environment mutations.
            unsafe { std::env::remove_var(ADMIN_TOKEN_ENV_VAR) };
            Self {
                _lock: lock,
                previous,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: the environment lock remains held while restoring the prior value.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(ADMIN_TOKEN_ENV_VAR, value),
                    None => std::env::remove_var(ADMIN_TOKEN_ENV_VAR),
                }
            }
        }
    }

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
            "#,
        )
        .expect("parse server config");

        assert_eq!(config.service, Some(ServerService::Codex));
        assert_eq!(config.host, Some("0.0.0.0".parse().unwrap()));
        assert_eq!(config.port, Some(3211));
        assert_eq!(config.admin_host, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(config.admin_port, Some(4211));
    }

    #[test]
    fn retired_server_fields_are_rejected_with_migration_guidance() {
        for field in [
            "advertised-admin-base-url = \"http://nas.local:4211\"",
            "host-local-session-history = true",
        ] {
            let error = parse_server_config(format!("[server]\n{field}\n").as_str())
                .expect_err("retired server field must not be ignored");
            let message = error.to_string();
            let field_name = field.split_once(" = ").expect("field assignment").0;
            assert!(
                message.contains(field_name),
                "error must identify {field_name}: {message}"
            );
            assert!(
                message.contains("has been removed") && message.contains("Remove"),
                "error must include migration guidance: {message}"
            );
        }

        let error = parse_server_config(
            r#"
            [server]
            host-local-session-history = false
            advertised-admin-base-url = "https://relay.example:4211"
            "#,
        )
        .expect_err("all retired server fields must be reported");
        let message = error.to_string();
        let advertised = message
            .find("server.advertised-admin-base-url")
            .expect("advertised field path");
        let history = message
            .find("server.host-local-session-history")
            .expect("history field path");
        assert!(
            advertised < history,
            "paths must have stable order: {message}"
        );

        parse_server_config(
            r#"
            [server]
            advertised-admin-base-url-hint = "extension"
            host-local-session-history-note = true
            "#,
        )
        .expect("similarly named extension keys must not trigger retired-field checks");
    }

    #[test]
    fn load_server_config_rejects_retired_fields_from_file() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "codex-helper-server-retired-config-{}-{nonce}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "[server]\nhost-local-session-history = true\n")
            .expect("write temporary server config");

        let result = load_server_config(Some(&path));
        std::fs::remove_file(&path).expect("remove temporary server config");

        let error = result.expect_err("file loader must reject retired server fields");
        let message = error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ");
        assert!(message.contains(path.to_string_lossy().as_ref()));
        assert!(message.contains("server.host-local-session-history"));
        assert!(message.contains("has been removed"));
    }

    #[test]
    fn effective_config_merges_cli_overrides() {
        let config = EffectiveServerConfig::from_sources(
            ServerConfigSection {
                service: Some(ServerService::Claude),
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
    }

    #[test]
    fn effective_config_rejects_remote_admin_without_token() {
        let _token = ScopedEnv::remove();

        let result = EffectiveServerConfig::from_sources(
            ServerConfigSection::default(),
            ServerConfigOverrides {
                admin_host: Some("0.0.0.0".parse().unwrap()),
                ..ServerConfigOverrides::default()
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn effective_config_rejects_remote_admin_with_invalid_header_token() {
        let _token = ScopedEnv::set("invalid\nheader");

        let result = EffectiveServerConfig::from_sources(
            ServerConfigSection::default(),
            ServerConfigOverrides {
                admin_host: Some("0.0.0.0".parse().unwrap()),
                ..ServerConfigOverrides::default()
            },
        );

        let error = result.expect_err("invalid HTTP header token must fail before bind");
        assert!(error.to_string().contains(ADMIN_TOKEN_ENV_VAR));
        assert!(!error.to_string().contains("invalid\nheader"));
    }
}

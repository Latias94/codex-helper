use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use clap::ValueEnum;
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
    pub host_local_session_history: Option<bool>,
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
            host-local-session-history = false
            "#,
        )
        .expect("parse server config");

        assert_eq!(config.service, Some(ServerService::Codex));
        assert_eq!(config.host, Some("0.0.0.0".parse().unwrap()));
        assert_eq!(config.port, Some(3211));
        assert_eq!(config.admin_host, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(config.admin_port, Some(4211));
        assert_eq!(config.host_local_session_history, Some(false));
    }
}

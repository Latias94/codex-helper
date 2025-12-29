use crate::config::{
    ServiceConfig, ServiceKind, UpstreamAuth, UpstreamConfig, config_file_path,
    import_codex_config_from_codex_cli, init_config_toml, load_config, save_config,
};
use crate::{CliError, CliResult, ConfigCommand};

async fn resolve_service(codex: bool, claude: bool) -> anyhow::Result<&'static str> {
    if codex && claude {
        anyhow::bail!("Please specify at most one of --codex / --claude");
    }
    if codex {
        return Ok("codex");
    }
    if claude {
        return Ok("claude");
    }

    // 未显式指定时，根据配置中的 default_service 决定默认服务（缺省为 Codex）。
    match load_config().await {
        Ok(cfg) => match cfg.default_service {
            Some(ServiceKind::Claude) => Ok("claude"),
            _ => Ok("codex"),
        },
        Err(_) => Ok("codex"),
    }
}

pub async fn handle_config_cmd(cmd: ConfigCommand) -> CliResult<()> {
    match cmd {
        ConfigCommand::Init { force } => {
            let path = init_config_toml(force)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Wrote TOML config template to {:?}", path);
        }
        ConfigCommand::List { codex, claude } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mgr, label) = if service == "claude" {
                (&cfg.claude, "Claude")
            } else {
                (&cfg.codex, "Codex")
            };
            let cfg_path = config_file_path();

            if mgr.configs.is_empty() {
                println!("No {} configs in {:?}", label, cfg_path);
            } else {
                let active = mgr.active.clone();
                println!("{} configs (from {:?}):", label, cfg_path);
                let mut items = mgr
                    .configs
                    .iter()
                    .map(|(name, svc)| (name.as_str(), svc))
                    .collect::<Vec<_>>();
                items.sort_by(|(a_name, a), (b_name, b)| {
                    let a_level = a.level.clamp(1, 10);
                    let b_level = b.level.clamp(1, 10);
                    a_level.cmp(&b_level).then_with(|| a_name.cmp(b_name))
                });

                for (name, service_cfg) in items {
                    let marker = if active.as_deref() == Some(name) {
                        "*"
                    } else {
                        " "
                    };
                    let enabled = if service_cfg.enabled { "on" } else { "off" };
                    let level = service_cfg.level.clamp(1, 10);
                    if let Some(alias) = &service_cfg.alias {
                        println!(
                            "  {} L{} {} {} [{}] ({} upstreams)",
                            marker,
                            level,
                            enabled,
                            name,
                            alias,
                            service_cfg.upstreams.len()
                        );
                    } else {
                        println!(
                            "  {} L{} {} {} ({} upstreams)",
                            marker,
                            level,
                            enabled,
                            name,
                            service_cfg.upstreams.len()
                        );
                    }
                }
            }
        }
        ConfigCommand::Add {
            name,
            base_url,
            auth_token,
            auth_token_env,
            api_key,
            api_key_env,
            alias,
            level,
            disabled,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let upstream = UpstreamConfig {
                base_url,
                auth: UpstreamAuth {
                    auth_token,
                    auth_token_env,
                    api_key,
                    api_key_env,
                },
                tags: Default::default(),
                supported_models: Default::default(),
                model_mapping: Default::default(),
            };
            let service_cfg = ServiceConfig {
                name: name.clone(),
                alias,
                enabled: !disabled,
                level: level.clamp(1, 10),
                upstreams: vec![upstream],
            };

            if service == "claude" {
                cfg.claude.configs.insert(name.clone(), service_cfg);
                if cfg.claude.active.is_none() {
                    cfg.claude.active = Some(name.clone());
                }
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Added Claude config '{}'", name);
            } else {
                cfg.codex.configs.insert(name.clone(), service_cfg);
                if cfg.codex.active.is_none() {
                    cfg.codex.active = Some(name.clone());
                }
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Added Codex config '{}'", name);
            }
        }
        ConfigCommand::SetActive {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if service == "claude" {
                if !cfg.claude.configs.contains_key(&name) {
                    println!("Claude config '{}' not found", name);
                } else {
                    cfg.claude.active = Some(name.clone());
                    save_config(&cfg)
                        .await
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    println!("Active Claude config set to '{}'", name);
                }
            } else if !cfg.codex.configs.contains_key(&name) {
                println!("Codex config '{}' not found", name);
            } else {
                cfg.codex.active = Some(name.clone());
                save_config(&cfg)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Active Codex config set to '{}'", name);
            }
        }
        ConfigCommand::SetLevel {
            name,
            level,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            if !(1..=10).contains(&level) {
                return Err(CliError::ProxyConfig(
                    "level must be in range 1..=10".to_string(),
                ));
            }

            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mgr = if service == "claude" {
                &mut cfg.claude
            } else {
                &mut cfg.codex
            };

            let Some(svc) = mgr.configs.get_mut(&name) else {
                println!(
                    "{} config '{}' not found",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
                return Ok(());
            };
            svc.level = level;
            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!(
                "Set {} config '{}' level to {}",
                if service == "claude" {
                    "Claude"
                } else {
                    "Codex"
                },
                name,
                level
            );
        }
        ConfigCommand::Enable {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mgr = if service == "claude" {
                &mut cfg.claude
            } else {
                &mut cfg.codex
            };

            let Some(svc) = mgr.configs.get_mut(&name) else {
                println!(
                    "{} config '{}' not found",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
                return Ok(());
            };
            svc.enabled = true;
            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!(
                "Enabled {} config '{}'",
                if service == "claude" {
                    "Claude"
                } else {
                    "Codex"
                },
                name
            );
        }
        ConfigCommand::Disable {
            name,
            codex,
            claude,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let is_active = {
                let mgr = if service == "claude" {
                    &mut cfg.claude
                } else {
                    &mut cfg.codex
                };

                let Some(svc) = mgr.configs.get_mut(&name) else {
                    println!(
                        "{} config '{}' not found",
                        if service == "claude" {
                            "Claude"
                        } else {
                            "Codex"
                        },
                        name
                    );
                    return Ok(());
                };
                svc.enabled = false;
                mgr.active.as_deref() == Some(name.as_str())
            };

            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if is_active {
                println!(
                    "Disabled {} config '{}' (note: active config is still eligible for routing)",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
            } else {
                println!(
                    "Disabled {} config '{}'",
                    if service == "claude" {
                        "Claude"
                    } else {
                        "Codex"
                    },
                    name
                );
            }
        }
        ConfigCommand::ImportFromCodex { force } => {
            let cfg = import_codex_config_from_codex_cli(force)
                .await
                .map_err(|e| CliError::CodexConfig(e.to_string()))?;
            if cfg.codex.configs.is_empty() {
                println!(
                    "No Codex configs were imported from ~/.codex; please ensure ~/.codex/config.toml and ~/.codex/auth.json are valid."
                );
            } else {
                let names: Vec<_> = cfg.codex.configs.keys().cloned().collect();
                println!(
                    "Imported Codex configs from ~/.codex (force = {}): {:?}",
                    force, names
                );
            }
        }
    }

    Ok(())
}

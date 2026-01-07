use crate::config::{
    RetryConfig, RetryProfileName, ServiceConfig, ServiceKind, UpstreamAuth, UpstreamConfig,
    config_file_path, import_codex_config_from_codex_cli, init_config_toml, load_config,
    overwrite_codex_config_from_codex_cli_in_place, save_config,
};
use crate::{CliError, CliResult, ConfigCommand, RetryProfile};

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
        ConfigCommand::Init { force, no_import } => {
            let path = init_config_toml(force, !no_import)
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
        ConfigCommand::SetRetryProfile { profile } => {
            let mut cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let profile_name = match profile {
                RetryProfile::Balanced => RetryProfileName::Balanced,
                RetryProfile::SameUpstream => RetryProfileName::SameUpstream,
                RetryProfile::AggressiveFailover => RetryProfileName::AggressiveFailover,
                RetryProfile::CostPrimary => RetryProfileName::CostPrimary,
            };

            // Apply profile and clear explicit per-field overrides to keep config minimal.
            cfg.retry = RetryConfig {
                profile: Some(profile_name),
                ..RetryConfig::default()
            };

            save_config(&cfg)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            println!("Set retry profile to '{:?}'", profile);
            let resolved = cfg.retry.resolve();
            println!(
                "retry: upstream(strategy={:?} max_attempts={} backoff={}..{} jitter={}) provider(strategy={:?} max_attempts={}) guardrails(never_on_status='{}' never_on_class={:?}) cooldown(cf_chal={}s cf_to={}s transport={}s) cooldown_backoff(factor={} max={}s)",
                resolved.upstream.strategy,
                resolved.upstream.max_attempts,
                resolved.upstream.backoff_ms,
                resolved.upstream.backoff_max_ms,
                resolved.upstream.jitter_ms,
                resolved.provider.strategy,
                resolved.provider.max_attempts,
                resolved.never_on_status,
                resolved.never_on_class,
                resolved.cloudflare_challenge_cooldown_secs,
                resolved.cloudflare_timeout_cooldown_secs,
                resolved.transport_cooldown_secs,
                resolved.cooldown_backoff_factor,
                resolved.cooldown_backoff_max_secs,
            );
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
        ConfigCommand::OverwriteFromCodex { dry_run, yes } => {
            if !dry_run && !yes {
                return Err(CliError::ProxyConfig(
                    "该操作会覆盖并重建 Codex 配置（active/enabled/level 会重置），请使用 --yes 确认，或先用 --dry-run 预览".to_string(),
                ));
            }
            let cfg = load_config()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            let mut working = if dry_run { cfg.clone() } else { cfg };
            overwrite_codex_config_from_codex_cli_in_place(&mut working)
                .map_err(|e| CliError::CodexConfig(e.to_string()))?;

            if dry_run {
                println!("Dry-run: no files written.");
            } else {
                save_config(&working)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            }

            let names: Vec<_> = working.codex.configs.keys().cloned().collect();
            println!(
                "Overwrote Codex configs from ~/.codex (dry_run = {}): {:?}",
                dry_run, names
            );
        }
    }

    Ok(())
}

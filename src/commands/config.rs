use crate::cli_types::ConfigSchemaTarget;
use crate::config::{
    ProxyConfig, ProxyConfigV2, RetryConfig, RetryProfileName, ServiceConfig, ServiceConfigManager,
    ServiceKind, ServiceRoutingExplanation, UpstreamAuth, UpstreamConfig,
    bootstrap::{
        import_codex_config_from_codex_cli, overwrite_codex_config_from_codex_cli_in_place,
    },
    compact_v2_config, compile_v2_to_runtime, explain_service_routing, migrate_legacy_to_v2,
    storage::{config_file_path, init_config_toml, load_config, save_config, save_config_v2},
};
use crate::{CliError, CliResult, ConfigCommand, RetryProfile};
use serde::Serialize;
use tokio::fs;

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

#[derive(Debug, Clone)]
enum ConfigDocument {
    Legacy(ProxyConfig),
    V2(ProxyConfigV2),
}

impl ConfigDocument {
    fn schema_version(&self) -> u32 {
        match self {
            Self::Legacy(cfg) => cfg.version.unwrap_or(1),
            Self::V2(cfg) => cfg.version,
        }
    }

    fn runtime(&self) -> anyhow::Result<ProxyConfig> {
        match self {
            Self::Legacy(cfg) => Ok(cfg.clone()),
            Self::V2(cfg) => compile_v2_to_runtime(cfg),
        }
    }

    fn v2_view(&self) -> ProxyConfigV2 {
        match self {
            Self::Legacy(cfg) => migrate_legacy_to_v2(cfg),
            Self::V2(cfg) => cfg.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ConfigExplainGroup {
    name: String,
    alias: Option<String>,
    enabled: bool,
    level: u8,
    upstreams: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ConfigExplainPayload {
    schema_version: u32,
    service: String,
    active_group: Option<String>,
    routing: ServiceRoutingExplanation,
    group: Option<ConfigExplainGroup>,
}

async fn load_config_document() -> anyhow::Result<ConfigDocument> {
    let path = config_file_path();
    if !path.exists() {
        return Ok(ConfigDocument::Legacy(load_config().await?));
    }

    let is_toml = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
    if !is_toml {
        return Ok(ConfigDocument::Legacy(load_config().await?));
    }

    let text = fs::read_to_string(&path).await?;
    let version = toml::from_str::<toml::Value>(&text)
        .ok()
        .and_then(|value| value.get("version").and_then(|v| v.as_integer()))
        .map(|value| value as u32);

    if version == Some(2) {
        let cfg = toml::from_str::<ProxyConfigV2>(&text)?;
        compile_v2_to_runtime(&cfg)?;
        Ok(ConfigDocument::V2(cfg))
    } else {
        Ok(ConfigDocument::Legacy(load_config().await?))
    }
}

fn select_service_manager<'a>(
    cfg: &'a ProxyConfig,
    service: &str,
) -> (&'a ServiceConfigManager, &'static str) {
    if service == "claude" {
        (&cfg.claude, "Claude")
    } else {
        (&cfg.codex, "Codex")
    }
}

fn build_group_explain(
    mgr: &ServiceConfigManager,
    group_name: Option<&str>,
) -> anyhow::Result<Option<ConfigExplainGroup>> {
    let Some(group_name) = group_name else {
        return Ok(None);
    };

    let svc = mgr
        .configs
        .get(group_name)
        .ok_or_else(|| anyhow::anyhow!("group/config '{}' not found", group_name))?;
    Ok(Some(ConfigExplainGroup {
        name: group_name.to_string(),
        alias: svc.alias.clone(),
        enabled: svc.enabled,
        level: svc.level.clamp(1, 10),
        upstreams: svc.upstreams.iter().map(|up| up.base_url.clone()).collect(),
    }))
}

fn print_explain_text(
    label: &str,
    schema_version: u32,
    routing: &ServiceRoutingExplanation,
    group: Option<&ConfigExplainGroup>,
) {
    println!("Schema version: v{}", schema_version);
    println!("Service: {}", label);
    println!(
        "Active group: {}",
        routing.active_config.as_deref().unwrap_or("<none>")
    );
    println!("Routing mode: {}", routing.mode);

    if routing.eligible_configs.is_empty() {
        println!("Candidate order: <empty>");
    } else {
        println!("Candidate order:");
        for (idx, candidate) in routing.eligible_configs.iter().enumerate() {
            let active = if candidate.active { " active" } else { "" };
            if let Some(alias) = candidate.alias.as_deref() {
                println!(
                    "  {}. {}{} (alias={}, level={}, enabled={}, upstreams={})",
                    idx + 1,
                    candidate.name,
                    active,
                    alias,
                    candidate.level,
                    candidate.enabled,
                    candidate.upstreams
                );
            } else {
                println!(
                    "  {}. {}{} (level={}, enabled={}, upstreams={})",
                    idx + 1,
                    candidate.name,
                    active,
                    candidate.level,
                    candidate.enabled,
                    candidate.upstreams
                );
            }
        }
    }

    if let Some(fallback) = &routing.fallback_config {
        println!(
            "Fallback: {} (level={}, enabled={}, upstreams={})",
            fallback.name, fallback.level, fallback.enabled, fallback.upstreams
        );
    }

    if let Some(group) = group {
        println!(
            "Group '{}': level={} enabled={} upstreams={}",
            group.name,
            group.level,
            group.enabled,
            group.upstreams.len()
        );
        if group.upstreams.is_empty() {
            println!("  <no upstreams>");
        } else {
            for (idx, upstream) in group.upstreams.iter().enumerate() {
                println!("  [{}] {}", idx, upstream);
            }
        }
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

        ConfigCommand::Explain {
            codex,
            claude,
            json,
            group,
        } => {
            let service = resolve_service(codex, claude)
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let runtime = document
                .runtime()
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let (mgr, label) = select_service_manager(&runtime, service);
            let routing = explain_service_routing(mgr);
            let group_detail = build_group_explain(mgr, group.as_deref())
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

            if json {
                let payload = ConfigExplainPayload {
                    schema_version: document.schema_version(),
                    service: service.to_string(),
                    active_group: mgr.active.clone(),
                    routing,
                    group: group_detail,
                };
                let text = serde_json::to_string_pretty(&payload)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                print_explain_text(
                    label,
                    document.schema_version(),
                    &routing,
                    group_detail.as_ref(),
                );
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
        ConfigCommand::Migrate {
            to,
            dry_run,
            write,
            compact,
            yes,
        } => {
            if write && !yes {
                return Err(CliError::ProxyConfig(
                    "This will overwrite ~/.codex-helper/config.toml; use --yes to confirm."
                        .to_string(),
                ));
            }

            let preview = dry_run || !write;
            match to {
                ConfigSchemaTarget::V2 => {
                    let document = load_config_document()
                        .await
                        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                    let migrated = if compact {
                        compact_v2_config(&document.v2_view())
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?
                    } else {
                        document.v2_view()
                    };

                    if preview {
                        let text = toml::to_string_pretty(&migrated)
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("{text}");
                    } else {
                        let path = save_config_v2(&migrated)
                            .await
                            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                        println!("Migrated config written to {:?}", path);
                    }
                }
            }
        }
    }

    Ok(())
}

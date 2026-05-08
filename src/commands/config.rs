use super::config_doc::load_config_document;
use crate::config::{
    RetryConfig, RetryProfileName,
    bootstrap::{
        import_codex_config_from_codex_cli, overwrite_codex_config_from_codex_cli_in_place,
    },
    storage::{init_config_toml, load_config, save_config, save_config_v3},
};
use crate::{CliError, CliResult, ConfigCommand, RetryProfile};

fn print_migration_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
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
                "retry: upstream(strategy={:?} max_attempts={} backoff={}..{} jitter={}) route(strategy={:?} max_attempts={}) guardrails(never_on_status='{}' never_on_class={:?}) cooldown(cf_chal={}s cf_to={}s transport={}s) cooldown_backoff(factor={} max={}s)",
                resolved.upstream.strategy,
                resolved.upstream.max_attempts,
                resolved.upstream.backoff_ms,
                resolved.upstream.backoff_max_ms,
                resolved.upstream.jitter_ms,
                resolved.route.strategy,
                resolved.route.max_attempts,
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
                    "No Codex providers were imported from ~/.codex; please ensure ~/.codex/config.toml and ~/.codex/auth.json are valid."
                );
            } else {
                let names: Vec<_> = cfg.codex.configs.keys().cloned().collect();
                println!(
                    "Imported Codex providers from ~/.codex (force = {}): {:?}",
                    force, names
                );
            }
        }
        ConfigCommand::OverwriteFromCodex { dry_run, yes } => {
            if !dry_run && !yes {
                return Err(CliError::ProxyConfig(
                    "This will overwrite and rebuild Codex provider config; use --yes to confirm, or preview with --dry-run.".to_string(),
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
                "Overwrote Codex providers from ~/.codex (dry_run = {}): {:?}",
                dry_run, names
            );
        }
        ConfigCommand::Migrate {
            dry_run,
            write,
            yes,
        } => {
            if write && !yes {
                return Err(CliError::ProxyConfig(
                    "This will overwrite ~/.codex-helper/config.toml; use --yes to confirm."
                        .to_string(),
                ));
            }

            let preview = dry_run || !write;
            let document = load_config_document()
                .await
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            let report = document
                .v3_migration_report()
                .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
            print_migration_warnings(&report.warnings);

            if preview {
                let text = toml::to_string_pretty(&report.config)
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("{text}");
            } else {
                let path = save_config_v3(&report.config)
                    .await
                    .map_err(|e| CliError::ProxyConfig(e.to_string()))?;
                println!("Migrated config written to {:?}", path);
            }
        }
    }

    Ok(())
}

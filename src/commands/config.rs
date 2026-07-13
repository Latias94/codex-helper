use crate::config::{
    RetryConfig, RetryProfileName,
    storage::{init_config_toml, load_config_with_source, save_helper_config},
};
use crate::{CliError, CliResult, ConfigCommand, RetryProfile};

pub async fn handle_config_cmd(cmd: ConfigCommand) -> CliResult<()> {
    match cmd {
        ConfigCommand::Init { force } => {
            let path = init_config_toml(force)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Wrote TOML config template to {:?}", path);
        }
        ConfigCommand::SetRetryProfile { profile } => {
            let loaded = load_config_with_source()
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            let mut cfg = loaded.source;

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

            save_helper_config(&cfg)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
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
    }

    Ok(())
}

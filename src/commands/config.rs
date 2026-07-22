use crate::config::{
    RetryConfig, RetryProfileName,
    storage::{init_config_toml_with_outcome, load_config, mutate_helper_config},
};
use crate::{CliError, CliResult, ConfigCommand, RetryProfile};

pub async fn handle_config_cmd(cmd: ConfigCommand) -> CliResult<()> {
    match cmd {
        ConfigCommand::Init { force } => {
            let outcome = init_config_toml_with_outcome(force)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            if let Some(report) = outcome.migration_report {
                print!("{report}");
            } else {
                println!("Wrote TOML config template to {:?}", outcome.path);
            }
        }
        ConfigCommand::SetRetryProfile { profile } => {
            // Trigger any legacy migration before entering the current-config mutation path.
            load_config()
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;

            let profile_name = match profile {
                RetryProfile::Balanced => RetryProfileName::Balanced,
                RetryProfile::SameUpstream => RetryProfileName::SameUpstream,
                RetryProfile::AggressiveFailover => RetryProfileName::AggressiveFailover,
                RetryProfile::CostPrimary => RetryProfileName::CostPrimary,
            };

            let retry = RetryConfig {
                profile: Some(profile_name),
                ..RetryConfig::default()
            };
            let resolved = retry.resolve();

            mutate_helper_config(move |config| {
                config.retry = retry;
                Ok(())
            })
            .await
            .map_err(|e| CliError::Configuration(e.to_string()))?;
            println!("Set retry profile to '{:?}'", profile);
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
        ConfigCommand::Migrate {
            dry_run: _,
            write,
            yes: _,
        } => {
            // Clap enforces --write/--yes pairing; omitting --write is the safe preview mode.
            let report = crate::config::LoadedConfig::migrate_config_file(write)
                .await
                .map_err(|e| CliError::Configuration(e.to_string()))?;
            print!("{report}");
        }
    }

    Ok(())
}

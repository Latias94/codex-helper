use crate::cli_types::PricingCommand;
use crate::{CliError, CliResult};
use codex_helper_core::basellm_catalog::{
    self, BasellmCatalogAttemptState, BasellmCatalogLkg, BasellmCatalogLoad,
    BasellmCatalogSyncOptions, BasellmSyncErrorCategory, BasellmSyncOutcome,
};
use codex_helper_core::pricing::{
    self, CostConfidence, LocalModelPriceOverride, LocalModelPriceOverridesDocument,
    LocalModelPriceTier, ManualPricingLayerStatus, ModelPriceCatalogSnapshot, ModelPriceView,
};
use owo_colors::OwoColorize;
use serde::Serialize;
use std::io::Write;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const PRICING_SYNC_TIMEOUT_SECS: u64 = 20;
const BASELLM_STATUS_STALE_SECS: i64 = 6 * 60 * 60;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PricingRemoteState {
    NeverSynced,
    Fresh,
    Stale,
    LastError,
    Quarantined,
    ReadOnly,
    Corrupt,
}

#[derive(Debug, Serialize)]
struct PricingStatusOutput {
    remote_state: PricingRemoteState,
    lkg_available: bool,
    source_url: Option<String>,
    fetched_at_unix: Option<i64>,
    validated_at_unix: Option<i64>,
    last_checked_at_unix: Option<i64>,
    last_check_outcome: Option<BasellmSyncOutcome>,
    last_error_category: Option<BasellmSyncErrorCategory>,
    quarantined: bool,
    read_only_schema_version: Option<u32>,
    retry_after_unix: Option<i64>,
    body_generation: Option<u64>,
    content_generation: Option<u64>,
    check_generation: Option<u64>,
    provider_count: usize,
    model_count: usize,
    priced_model_count: usize,
    tier_count: usize,
    effective_revision: String,
    effective_source: String,
    effective_model_count: usize,
    bundled_model_count: usize,
    remote_model_count: usize,
    remote_projection_warning_count: usize,
    manual_status: ManualPricingLayerStatus,
    manual_model_count: usize,
    manual_shadowed_remote_models: usize,
    manual_error_present: bool,
    effective_reloaded: bool,
    effective_changed: bool,
}

#[derive(Debug, Serialize)]
struct PricingRefreshOutput {
    outcome: BasellmSyncOutcome,
    error_category: Option<BasellmSyncErrorCategory>,
    approved_quarantined_candidate: bool,
    status: PricingStatusOutput,
}

pub async fn handle_pricing_cmd(cmd: PricingCommand) -> CliResult<()> {
    let mut stdout = std::io::stdout();
    handle_pricing_cmd_with_writer(cmd, &mut stdout).await
}

async fn handle_pricing_cmd_with_writer(
    cmd: PricingCommand,
    writer: &mut dyn Write,
) -> CliResult<()> {
    match cmd {
        PricingCommand::Path => {
            println!("{}", pricing::model_price_overrides_path().display());
        }
        PricingCommand::Status { json } => {
            let status = load_pricing_status();
            if json {
                write_json(writer, &status)?;
            } else {
                print_pricing_status(&status);
            }
        }
        PricingCommand::ForceRefresh {
            url,
            approve_economic_changes,
            json,
        } => {
            let mut options = BasellmCatalogSyncOptions::default()
                .with_source_url(url)
                .with_force(true);
            let approved_hash = if approve_economic_changes {
                Some(quarantined_candidate_hash_for_approval(
                    basellm_catalog::load_basellm_catalog_attempt_state().as_ref(),
                )?)
            } else {
                None
            };
            if let Some(hash) = approved_hash.as_deref() {
                options = options.with_approved_quarantine_hash(hash);
            }

            let report = basellm_catalog::sync_basellm_catalog(options).await;
            let output = PricingRefreshOutput {
                outcome: report.outcome,
                error_category: report.attempt.last_error_category,
                approved_quarantined_candidate: approved_hash.is_some(),
                status: load_pricing_status(),
            };
            if json {
                write_json(writer, &output)?;
            } else {
                println!(
                    "BaseLLM refresh: {:?}{}",
                    output.outcome,
                    output
                        .error_category
                        .map(|category| format!(" ({category:?})"))
                        .unwrap_or_default()
                );
                print_pricing_status(&output.status);
            }
        }
        PricingCommand::List {
            json,
            local,
            model,
            provider,
        } => {
            let snapshot = load_snapshot_for_list(local)?;
            let snapshot = filter_snapshot(snapshot, model.as_deref(), provider.as_deref())?;

            if json {
                let text = serde_json::to_string_pretty(&snapshot)
                    .map_err(|e| CliError::Pricing(e.to_string()))?;
                writeln!(writer, "{text}").map_err(|error| CliError::Pricing(error.to_string()))?;
            } else {
                print_snapshot_text(&snapshot, local, model.as_deref());
            }
        }
        PricingCommand::Set {
            model_id,
            provider,
            display_name,
            aliases,
            input_per_1m_usd,
            output_per_1m_usd,
            cache_read_input_per_1m_usd,
            cache_creation_input_per_1m_usd,
            confidence,
        } => {
            let model_id = normalize_model_id(&model_id)?;
            let mut document = pricing::load_model_price_overrides_document()
                .and_then(|document| document.normalized())
                .map_err(CliError::Pricing)?;

            let provider = normalize_provider(&provider)?;
            document
                .insert_model(
                    &provider,
                    &model_id,
                    LocalModelPriceOverride {
                        display_name,
                        aliases,
                        input_per_1m_usd,
                        output_per_1m_usd,
                        cache_read_input_per_1m_usd,
                        cache_creation_input_per_1m_usd,
                        tiers: Vec::new(),
                        confidence: Some(confidence.into()),
                    },
                )
                .map_err(CliError::Pricing)?;

            let path = pricing::save_model_price_overrides_document(&document)
                .map_err(CliError::Pricing)?;
            println!(
                "Updated local price override for '{}/{}' at {:?}",
                provider, model_id, path
            );
        }
        PricingCommand::Remove { model_id, provider } => {
            let model_id = normalize_model_id(&model_id)?;
            let provider = normalize_provider(&provider)?;
            let mut document = pricing::load_model_price_overrides_document()
                .and_then(|document| document.normalized())
                .map_err(CliError::Pricing)?;
            if document
                .remove_model(&provider, &model_id)
                .map_err(CliError::Pricing)?
                .is_none()
            {
                println!(
                    "No local price override found for '{}/{}'; nothing changed.",
                    provider, model_id
                );
                return Ok(());
            }

            let path = pricing::save_model_price_overrides_document(&document)
                .map_err(CliError::Pricing)?;
            println!(
                "Removed local price override for '{}/{}' and rewrote {:?}",
                provider, model_id, path
            );
        }
        PricingCommand::Sync {
            url,
            models,
            provider,
            replace,
            dry_run,
        } => {
            let snapshot = fetch_remote_pricing_catalog(&url).await?;
            import_snapshot(snapshot, models, provider.as_deref(), replace, dry_run)?;
        }
        PricingCommand::ImportBasellm {
            url,
            models,
            provider,
            replace,
            dry_run,
        } => {
            let text = fetch_remote_pricing_text(&url).await?;
            let snapshot = pricing::basellm_model_price_catalog_snapshot_from_json(&url, &text)
                .map_err(CliError::Pricing)?;
            import_snapshot(snapshot, models, Some(&provider), replace, dry_run)?;
        }
    }

    Ok(())
}

fn load_pricing_status() -> PricingStatusOutput {
    let lkg_load = basellm_catalog::load_basellm_catalog_lkg_from_path_blocking(
        &basellm_catalog::basellm_catalog_lkg_path(),
    );
    let attempt = basellm_catalog::load_basellm_catalog_attempt_state();
    let previous_effective = pricing::effective_pricing_catalog_snapshot();
    let effective = pricing::refresh_effective_pricing_catalog();
    let effective_changed = previous_effective.revision != effective.revision
        || previous_effective.manual_content_hash != effective.manual_content_hash
        || previous_effective.manual_status != effective.manual_status;
    let lkg = match &lkg_load {
        BasellmCatalogLoad::Valid(snapshot) => Some(snapshot.clone()),
        _ => None,
    };
    let counts = lkg
        .as_ref()
        .map(|snapshot| snapshot.counts)
        .unwrap_or_default();
    let read_only_schema_version = match &lkg_load {
        BasellmCatalogLoad::UnsupportedSchema(version) => Some(*version),
        _ => attempt
            .as_ref()
            .and_then(|attempt| attempt.read_only_schema_version),
    };

    PricingStatusOutput {
        remote_state: classify_pricing_remote_state(&lkg_load, attempt.as_ref(), unix_now()),
        lkg_available: lkg.is_some(),
        source_url: lkg
            .as_ref()
            .map(|snapshot| snapshot.source_url.clone())
            .or_else(|| attempt.as_ref().map(|attempt| attempt.source_url.clone())),
        fetched_at_unix: lkg.as_ref().map(|snapshot| snapshot.fetched_at_unix),
        validated_at_unix: lkg.as_ref().map(|snapshot| snapshot.validated_at_unix),
        last_checked_at_unix: attempt.as_ref().map(|attempt| attempt.last_checked_at_unix),
        last_check_outcome: attempt.as_ref().map(|attempt| attempt.outcome),
        last_error_category: attempt
            .as_ref()
            .and_then(|attempt| attempt.last_error_category),
        quarantined: attempt
            .as_ref()
            .is_some_and(|attempt| attempt.outcome == BasellmSyncOutcome::Quarantined),
        read_only_schema_version,
        retry_after_unix: attempt
            .as_ref()
            .and_then(|attempt| attempt.retry_after_unix),
        body_generation: lkg.as_ref().map(|snapshot| snapshot.body_generation),
        content_generation: lkg.as_ref().map(|snapshot| snapshot.content_generation),
        check_generation: attempt.as_ref().map(|attempt| attempt.check_generation),
        provider_count: counts.provider_count,
        model_count: counts.model_count,
        priced_model_count: counts.priced_model_count,
        tier_count: counts.tier_count,
        effective_revision: effective.revision.clone(),
        effective_source: effective.source.clone(),
        effective_model_count: effective.model_count,
        bundled_model_count: effective.bundled_model_count,
        remote_model_count: effective.remote_model_count,
        remote_projection_warning_count: effective.remote_projection_warnings.len(),
        manual_status: effective.manual_status,
        manual_model_count: effective.manual_model_count,
        manual_shadowed_remote_models: count_manual_shadows(lkg.as_deref()),
        manual_error_present: effective.manual_error.is_some(),
        effective_reloaded: true,
        effective_changed,
    }
}

fn classify_pricing_remote_state(
    load: &BasellmCatalogLoad,
    attempt: Option<&BasellmCatalogAttemptState>,
    now_unix: i64,
) -> PricingRemoteState {
    if matches!(load, BasellmCatalogLoad::UnsupportedSchema(_))
        || attempt.is_some_and(|attempt| attempt.outcome == BasellmSyncOutcome::ReadOnly)
    {
        return PricingRemoteState::ReadOnly;
    }
    if matches!(load, BasellmCatalogLoad::Corrupt) {
        return PricingRemoteState::Corrupt;
    }
    if attempt.is_some_and(|attempt| attempt.outcome == BasellmSyncOutcome::Quarantined) {
        return PricingRemoteState::Quarantined;
    }
    if attempt.is_some_and(|attempt| attempt.last_error_category.is_some()) {
        return PricingRemoteState::LastError;
    }

    let BasellmCatalogLoad::Valid(snapshot) = load else {
        return PricingRemoteState::NeverSynced;
    };
    let freshness_anchor = attempt
        .map(|attempt| attempt.last_checked_at_unix)
        .unwrap_or(snapshot.validated_at_unix);
    if now_unix.saturating_sub(freshness_anchor) > BASELLM_STATUS_STALE_SECS {
        PricingRemoteState::Stale
    } else {
        PricingRemoteState::Fresh
    }
}

fn count_manual_shadows(lkg: Option<&BasellmCatalogLkg>) -> usize {
    let Some(lkg) = lkg else {
        return 0;
    };
    let Ok(document) =
        pricing::load_model_price_overrides_document().and_then(|document| document.normalized())
    else {
        return 0;
    };
    document
        .providers
        .iter()
        .flat_map(|(provider, rows)| {
            rows.models
                .keys()
                .map(move |model| (provider.as_str(), model.as_str()))
        })
        .filter(|(provider, model)| {
            lkg.model(provider, model)
                .is_some_and(|model| model.price.is_some())
        })
        .count()
}

fn quarantined_candidate_hash_for_approval(
    attempt: Option<&BasellmCatalogAttemptState>,
) -> Result<String, CliError> {
    let Some(attempt) = attempt else {
        return Err(CliError::Pricing(
            "no quarantined BaseLLM economic-change candidate is available".to_string(),
        ));
    };
    if attempt.outcome != BasellmSyncOutcome::Quarantined
        || attempt.last_error_category != Some(BasellmSyncErrorCategory::EconomicAnomaly)
    {
        return Err(CliError::Pricing(
            "the latest BaseLLM attempt is not an economic-change quarantine".to_string(),
        ));
    }
    attempt.quarantined_candidate_hash.clone().ok_or_else(|| {
        CliError::Pricing("the quarantined BaseLLM attempt has no valid candidate hash".to_string())
    })
}

fn write_json(output: &mut dyn Write, value: &impl Serialize) -> CliResult<()> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::Pricing(error.to_string()))?;
    writeln!(output, "{text}").map_err(|error| CliError::Pricing(error.to_string()))?;
    Ok(())
}

fn print_pricing_status(status: &PricingStatusOutput) {
    println!(
        "{}",
        format!(
            "BaseLLM pricing: {}",
            remote_state_label(status.remote_state)
        )
        .bold()
    );
    println!(
        "source={} lkg={} last_check={} outcome={} error={}",
        status.source_url.as_deref().unwrap_or("-"),
        if status.lkg_available {
            "available"
        } else {
            "unavailable"
        },
        status
            .last_checked_at_unix
            .map(age_text)
            .unwrap_or_else(|| "never".to_string()),
        status
            .last_check_outcome
            .map(|outcome| format!("{outcome:?}"))
            .unwrap_or_else(|| "-".to_string()),
        status
            .last_error_category
            .map(|category| format!("{category:?}"))
            .unwrap_or_else(|| "-".to_string()),
    );
    println!(
        "generations: body={} content={} check={} effective={}",
        optional_u64_text(status.body_generation),
        optional_u64_text(status.content_generation),
        optional_u64_text(status.check_generation),
        short_revision(&status.effective_revision),
    );
    println!(
        "remote: providers={} models={} priced={} tiers={} projected={} warnings={}",
        status.provider_count,
        status.model_count,
        status.priced_model_count,
        status.tier_count,
        status.remote_model_count,
        status.remote_projection_warning_count,
    );
    println!(
        "manual: {:?} models={} shadows={} invalid={} reloaded={} changed={}",
        status.manual_status,
        status.manual_model_count,
        status.manual_shadowed_remote_models,
        status.manual_error_present,
        status.effective_reloaded,
        status.effective_changed,
    );
    println!(
        "effective: source={} models={} bundled={}",
        status.effective_source, status.effective_model_count, status.bundled_model_count,
    );
}

fn remote_state_label(state: PricingRemoteState) -> &'static str {
    match state {
        PricingRemoteState::NeverSynced => "never synced",
        PricingRemoteState::Fresh => "fresh",
        PricingRemoteState::Stale => "stale",
        PricingRemoteState::LastError => "last refresh failed; LKG retained",
        PricingRemoteState::Quarantined => "economic change quarantined",
        PricingRemoteState::ReadOnly => "read-only (newer schema)",
        PricingRemoteState::Corrupt => "corrupt LKG",
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0)
}

fn age_text(timestamp_unix: i64) -> String {
    let seconds = unix_now().saturating_sub(timestamp_unix).max(0);
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    if seconds < 60 * 60 {
        return format!("{}m ago", seconds / 60);
    }
    if seconds < 24 * 60 * 60 {
        return format!("{}h ago", seconds / (60 * 60));
    }
    format!("{}d ago", seconds / (24 * 60 * 60))
}

fn optional_u64_text(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn short_revision(revision: &str) -> &str {
    revision.get(..revision.len().min(20)).unwrap_or(revision)
}

async fn fetch_remote_pricing_catalog(url: &str) -> Result<ModelPriceCatalogSnapshot, CliError> {
    let text = fetch_remote_pricing_text(url).await?;
    serde_json::from_str(&text)
        .map_err(|err| CliError::Pricing(format!("invalid pricing catalog JSON: {err}")))
}

async fn fetch_remote_pricing_text(url: &str) -> Result<String, CliError> {
    let url = reqwest::Url::parse(url)
        .map_err(|err| CliError::Pricing(format!("invalid pricing sync URL: {err}")))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(CliError::Pricing(format!(
                "pricing sync URL must use http or https, got '{scheme}'"
            )));
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(PRICING_SYNC_TIMEOUT_SECS))
        .build()
        .map_err(|err| CliError::Pricing(format!("failed to build HTTP client: {err}")))?;
    let text = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| CliError::Pricing(format!("pricing sync request failed: {err}")))?
        .error_for_status()
        .map_err(|err| CliError::Pricing(format!("pricing sync HTTP error: {err}")))?
        .text()
        .await
        .map_err(|err| CliError::Pricing(format!("failed to read pricing sync response: {err}")))?;
    Ok(text)
}

fn import_snapshot(
    snapshot: ModelPriceCatalogSnapshot,
    models: Vec<String>,
    provider: Option<&str>,
    replace: bool,
    dry_run: bool,
) -> CliResult<()> {
    let base_document = if replace {
        LocalModelPriceOverridesDocument::default()
    } else {
        pricing::load_model_price_overrides_document()
            .and_then(|document| document.normalized())
            .map_err(CliError::Pricing)?
    };
    let (document, imported) =
        merge_snapshot_into_overrides(base_document, &snapshot, &models, provider)?;

    if imported == 0 {
        println!(
            "No pricing rows matched the requested filters from {}.",
            snapshot.source
        );
        return Ok(());
    }

    if dry_run {
        println!(
            "Would import {} pricing row(s) from {} into {:?}.",
            imported,
            snapshot.source,
            pricing::model_price_overrides_path()
        );
    } else {
        let path =
            pricing::save_model_price_overrides_document(&document).map_err(CliError::Pricing)?;
        println!(
            "Imported {} pricing row(s) from {} into {:?}.",
            imported, snapshot.source, path
        );
    }
    Ok(())
}

fn merge_snapshot_into_overrides(
    mut document: LocalModelPriceOverridesDocument,
    snapshot: &ModelPriceCatalogSnapshot,
    model_filters: &[String],
    provider_filter: Option<&str>,
) -> Result<(LocalModelPriceOverridesDocument, usize), CliError> {
    let filters = model_filters
        .iter()
        .map(|model| normalize_model_id(model))
        .collect::<Result<Vec<_>, _>>()?;
    let provider_filter = provider_filter.map(normalize_provider).transpose()?;
    let mut imported = 0usize;

    for row in &snapshot.models {
        if provider_filter
            .as_deref()
            .is_some_and(|provider| !row.matches_provider(provider))
        {
            continue;
        }
        if !filters.is_empty()
            && !filters
                .iter()
                .any(|filter| row.matches_model(filter.as_str()))
        {
            continue;
        }

        let model_id = normalize_model_id(&row.model_id)?;
        document
            .insert_model(
                &row.provider,
                &model_id,
                LocalModelPriceOverride {
                    display_name: row.display_name.clone(),
                    aliases: row.aliases.clone(),
                    input_per_1m_usd: row.input_per_1m_usd.clone(),
                    output_per_1m_usd: row.output_per_1m_usd.clone(),
                    cache_read_input_per_1m_usd: row.cache_read_input_per_1m_usd.clone(),
                    cache_creation_input_per_1m_usd: row.cache_creation_input_per_1m_usd.clone(),
                    tiers: row.tiers.iter().map(LocalModelPriceTier::from).collect(),
                    confidence: Some(row.confidence),
                },
            )
            .map_err(CliError::Pricing)?;
        imported += 1;
    }

    let document = document.normalized().map_err(CliError::Pricing)?;
    Ok((document, imported))
}

fn load_snapshot_for_list(local: bool) -> Result<ModelPriceCatalogSnapshot, CliError> {
    if local {
        return pricing::local_model_price_catalog_snapshot().map_err(CliError::Pricing);
    }

    pricing::load_model_price_overrides_document().map_err(CliError::Pricing)?;
    Ok(pricing::operator_model_price_catalog_snapshot())
}

fn filter_snapshot(
    mut snapshot: ModelPriceCatalogSnapshot,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<ModelPriceCatalogSnapshot, CliError> {
    let provider = provider.map(normalize_provider).transpose()?;
    snapshot.models.retain(|row| {
        model
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none_or(|model| row.matches_model(model))
            && provider
                .as_deref()
                .is_none_or(|provider| row.matches_provider(provider))
    });
    snapshot.model_count = snapshot.models.len();
    Ok(snapshot)
}

fn print_snapshot_text(snapshot: &ModelPriceCatalogSnapshot, local: bool, model: Option<&str>) {
    let title = if local {
        "Local pricing overrides"
    } else {
        "Operator pricing catalog"
    };
    println!(
        "{}",
        format!("{title}: {} ({})", snapshot.source, snapshot.model_count).bold()
    );

    if snapshot.models.is_empty() {
        if let Some(model) = model {
            println!("No pricing rows matched '{}'.", model);
        } else if local {
            println!(
                "No local pricing overrides at {:?}.",
                pricing::model_price_overrides_path()
            );
        } else {
            println!("No pricing rows available.");
        }
        return;
    }

    println!(
        "{}",
        "provider/model | input/1m | output/1m | cache read/1m | cache create/1m | tiers | confidence | source | aliases"
            .bold()
    );
    for row in &snapshot.models {
        print_snapshot_row(row);
    }
}

fn print_snapshot_row(row: &ModelPriceView) {
    let label = match row.display_name.as_deref() {
        Some(display_name) if !display_name.trim().is_empty() => {
            format!("{}/{} [{}]", row.provider, row.model_id, display_name)
        }
        _ => format!("{}/{}", row.provider, row.model_id),
    };
    let aliases = if row.aliases.is_empty() {
        "-".to_string()
    } else {
        row.aliases.join(", ")
    };

    println!(
        "{} | {} | {} | {} | {} | {} | {} | {} | {}",
        label,
        format_usd(row.input_per_1m_usd.as_str()),
        format_usd(row.output_per_1m_usd.as_str()),
        format_optional_usd(row.cache_read_input_per_1m_usd.as_deref()),
        format_optional_usd(row.cache_creation_input_per_1m_usd.as_deref()),
        row.tiers.len(),
        confidence_label(row.confidence),
        row.source,
        aliases,
    );
}

fn format_usd(value: &str) -> String {
    format!("${value}")
}

fn format_optional_usd(value: Option<&str>) -> String {
    value.map(format_usd).unwrap_or_else(|| "-".to_string())
}

fn confidence_label(confidence: CostConfidence) -> &'static str {
    match confidence {
        CostConfidence::Unknown => "unknown",
        CostConfidence::Partial => "partial",
        CostConfidence::Estimated => "estimated",
        CostConfidence::Exact => "exact",
    }
}

fn normalize_model_id(value: &str) -> Result<String, CliError> {
    let model_id = value.trim();
    if model_id.is_empty() {
        return Err(CliError::Pricing(
            "model id cannot be empty or whitespace".to_string(),
        ));
    }
    Ok(model_id.to_string())
}

fn normalize_provider(value: &str) -> Result<String, CliError> {
    pricing::canonical_provider(value)
        .ok_or_else(|| CliError::Pricing("provider cannot be empty or whitespace".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_types::{Cli, Command};
    use crate::commands::test_support::{ScopedEnv, TempTestDir, env_lock};
    use clap::Parser;
    use codex_helper_core::basellm_catalog::{BasellmCatalogContent, BasellmCatalogCounts};
    use codex_helper_core::pricing::ModelPriceTierView;
    use std::sync::Arc;

    fn price_row(provider: &str, model_id: &str, aliases: Vec<&str>) -> ModelPriceView {
        ModelPriceView {
            provider: provider.to_string(),
            model_id: model_id.to_string(),
            display_name: Some(format!("{model_id} display")),
            aliases: aliases.into_iter().map(str::to_string).collect(),
            input_per_1m_usd: "1.25".to_string(),
            output_per_1m_usd: "10.00".to_string(),
            cache_read_input_per_1m_usd: Some("0.125".to_string()),
            cache_creation_input_per_1m_usd: None,
            tiers: Vec::new(),
            source: "remote-test".to_string(),
            source_generation: Some("generation-7".to_string()),
            confidence: CostConfidence::Exact,
        }
    }

    fn catalog_snapshot(rows: Vec<ModelPriceView>) -> ModelPriceCatalogSnapshot {
        ModelPriceCatalogSnapshot {
            source: "remote-test".to_string(),
            model_count: rows.len(),
            models: rows,
        }
    }

    fn local_override(input: &str, output: &str) -> LocalModelPriceOverride {
        LocalModelPriceOverride {
            display_name: None,
            aliases: Vec::new(),
            input_per_1m_usd: input.to_string(),
            output_per_1m_usd: output.to_string(),
            cache_read_input_per_1m_usd: None,
            cache_creation_input_per_1m_usd: None,
            tiers: Vec::new(),
            confidence: Some(CostConfidence::Estimated),
        }
    }

    fn valid_lkg(validated_at_unix: i64) -> BasellmCatalogLoad {
        BasellmCatalogLoad::Valid(Arc::new(BasellmCatalogLkg {
            schema_version: 1,
            manifest_generation: 1,
            body_generation: 2,
            content_generation: 3,
            source_url: "https://basellm.github.io/llm-metadata/api/all.json".to_string(),
            fetched_at_unix: validated_at_unix,
            validated_at_unix,
            etag: Some("validator-must-not-leak".to_string()),
            last_modified: Some("validator-must-not-leak".to_string()),
            content_hash: format!("sha256:{}", "a".repeat(64)),
            counts: BasellmCatalogCounts::default(),
            warnings: vec!["payload-must-not-leak".to_string()],
            catalog: BasellmCatalogContent::default(),
        }))
    }

    fn attempt(
        outcome: BasellmSyncOutcome,
        category: Option<BasellmSyncErrorCategory>,
    ) -> BasellmCatalogAttemptState {
        BasellmCatalogAttemptState {
            schema_version: 1,
            check_generation: 4,
            source_url: "https://basellm.github.io/llm-metadata/api/all.json".to_string(),
            last_checked_at_unix: 10_000,
            outcome,
            last_error_category: category,
            content_hash: None,
            content_generation: Some(3),
            quarantined_candidate_hash: None,
            read_only_schema_version: None,
            retry_after_unix: None,
        }
    }

    #[test]
    fn sync_merge_imports_remote_rows_without_dropping_existing_overrides() {
        let mut document = LocalModelPriceOverridesDocument::default();
        document
            .insert_model("openai", "existing-model", local_override("0.10", "0.20"))
            .expect("insert existing");
        let snapshot = catalog_snapshot(vec![price_row(
            "openai",
            "remote-model",
            vec!["relay-fast"],
        )]);

        let (document, imported) =
            merge_snapshot_into_overrides(document, &snapshot, &[], Some("openai")).expect("merge");

        assert_eq!(imported, 1);
        assert!(document.model("openai", "existing-model").is_some());
        let imported = document
            .model("openai", "remote-model")
            .expect("remote model");
        assert_eq!(
            imported.display_name.as_deref(),
            Some("remote-model display")
        );
        assert_eq!(imported.aliases, vec!["relay-fast"]);
        assert_eq!(
            imported.cache_read_input_per_1m_usd.as_deref(),
            Some("0.125")
        );
        assert_eq!(imported.confidence, Some(CostConfidence::Exact));
    }

    #[test]
    fn sync_merge_filters_by_model_alias() {
        let snapshot = catalog_snapshot(vec![
            price_row("openai", "gpt-relay", vec!["relay-fast"]),
            price_row("openai", "other-model", vec![]),
        ]);

        let (document, imported) = merge_snapshot_into_overrides(
            LocalModelPriceOverridesDocument::default(),
            &snapshot,
            &[String::from("relay-fast")],
            Some("openai"),
        )
        .expect("merge");

        assert_eq!(imported, 1);
        assert!(document.model("openai", "gpt-relay").is_some());
        assert!(document.model("openai", "other-model").is_none());
    }

    #[test]
    fn basellm_import_filters_provider_and_preserves_tiers() {
        let mut openai = price_row("openai", "shared-model", vec![]);
        openai.tiers.push(ModelPriceTierView {
            threshold_tokens: 272_000,
            input_per_1m_usd: Some("10".to_string()),
            output_per_1m_usd: Some("45".to_string()),
            cache_read_input_per_1m_usd: Some("1".to_string()),
            cache_creation_input_per_1m_usd: Some("12.5".to_string()),
        });
        let routing = price_row("routing-run", "shared-model", vec![]);
        let snapshot = catalog_snapshot(vec![openai, routing]);

        let (document, imported) = merge_snapshot_into_overrides(
            LocalModelPriceOverridesDocument::default(),
            &snapshot,
            &[],
            Some("codex"),
        )
        .expect("merge");

        assert_eq!(imported, 1);
        let imported = document
            .model("openai", "shared-model")
            .expect("openai model");
        assert_eq!(imported.tiers.len(), 1);
        assert_eq!(imported.tiers[0].threshold_tokens, 272_000);
        assert!(document.model("routing-run", "shared-model").is_none());
    }

    #[test]
    fn pricing_remote_state_distinguishes_operator_states() {
        let now = 100_000;
        assert_eq!(
            classify_pricing_remote_state(&BasellmCatalogLoad::Missing, None, now),
            PricingRemoteState::NeverSynced
        );
        assert_eq!(
            classify_pricing_remote_state(&valid_lkg(now), None, now),
            PricingRemoteState::Fresh
        );
        assert_eq!(
            classify_pricing_remote_state(
                &valid_lkg(now - BASELLM_STATUS_STALE_SECS - 1),
                None,
                now,
            ),
            PricingRemoteState::Stale
        );
        assert_eq!(
            classify_pricing_remote_state(
                &valid_lkg(now),
                Some(&attempt(
                    BasellmSyncOutcome::Unavailable,
                    Some(BasellmSyncErrorCategory::Transport),
                )),
                now,
            ),
            PricingRemoteState::LastError
        );
        assert_eq!(
            classify_pricing_remote_state(
                &valid_lkg(now),
                Some(&attempt(
                    BasellmSyncOutcome::Quarantined,
                    Some(BasellmSyncErrorCategory::EconomicAnomaly),
                )),
                now,
            ),
            PricingRemoteState::Quarantined
        );
        assert_eq!(
            classify_pricing_remote_state(&BasellmCatalogLoad::UnsupportedSchema(9), None, now,),
            PricingRemoteState::ReadOnly
        );
        assert_eq!(
            classify_pricing_remote_state(&BasellmCatalogLoad::Corrupt, None, now),
            PricingRemoteState::Corrupt
        );
    }

    #[test]
    fn economic_change_approval_is_bound_to_latest_quarantine() {
        let mut quarantined = attempt(
            BasellmSyncOutcome::Quarantined,
            Some(BasellmSyncErrorCategory::EconomicAnomaly),
        );
        let candidate_hash = format!("sha256:{}", "c".repeat(64));
        quarantined.quarantined_candidate_hash = Some(candidate_hash.clone());
        assert_eq!(
            quarantined_candidate_hash_for_approval(Some(&quarantined)).expect("approval"),
            candidate_hash
        );

        let unavailable = attempt(
            BasellmSyncOutcome::Unavailable,
            Some(BasellmSyncErrorCategory::Transport),
        );
        assert!(quarantined_candidate_hash_for_approval(Some(&unavailable)).is_err());
        assert!(quarantined_candidate_hash_for_approval(None).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pricing_status_json_runs_the_command_path_without_leaking_corrupt_state() {
        let _env_lock = env_lock().await;
        let helper_home = TempTestDir::new("codex-helper-cli-test-pricing-status");
        let mut scoped_env = ScopedEnv::default();
        unsafe {
            scoped_env.set_path("CODEX_HELPER_HOME", helper_home.path());
        }

        let lkg_path = basellm_catalog::basellm_catalog_lkg_path();
        std::fs::create_dir_all(lkg_path.parent().expect("LKG parent")).expect("create LKG parent");
        std::fs::write(
            &lkg_path,
            br#"{
                "schema_version": 1,
                "etag": "validator-must-not-leak",
                "last_modified": "validator-must-not-leak",
                "catalog": "payload-must-not-leak",
                "source_url": "https://user:credential@example.invalid/private?token=secret"
            }"#,
        )
        .expect("write corrupt LKG fixture");

        let cli = Cli::try_parse_from(["codex-helper", "pricing", "status", "--json"])
            .expect("parse pricing status command");
        let Some(Command::Pricing { cmd }) = cli.command else {
            panic!("expected pricing command");
        };
        let mut stdout = Vec::new();
        handle_pricing_cmd_with_writer(cmd, &mut stdout)
            .await
            .expect("run pricing status command");
        let text = String::from_utf8(stdout).expect("UTF-8 stdout");
        let value: serde_json::Value = serde_json::from_str(&text).expect("pricing status JSON");

        assert_eq!(value["remote_state"], "corrupt");
        assert_eq!(value["lkg_available"], false);
        assert!(value["source_url"].is_null());
        assert!(value["fetched_at_unix"].is_null());
        assert!(value["last_error_category"].is_null());
        for forbidden in [
            "catalog",
            "etag",
            "last_modified",
            "quarantined_candidate_hash",
            "payload-must-not-leak",
            "validator-must-not-leak",
            "credential",
            "token=secret",
        ] {
            assert!(!text.contains(forbidden), "leaked {forbidden}: {text}");
        }
        assert!(value.get("body_generation").is_some());
        assert!(value.get("content_generation").is_some());
        assert!(value.get("check_generation").is_some());
        assert!(value.get("effective_revision").is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pricing_force_refresh_json_reports_offline_category_without_leaking_source_query() {
        let _env_lock = env_lock().await;
        let helper_home = TempTestDir::new("codex-helper-cli-test-pricing-refresh");
        let mut scoped_env = ScopedEnv::default();
        unsafe {
            scoped_env.set_path("CODEX_HELPER_HOME", helper_home.path());
            scoped_env.set("NO_PROXY", "127.0.0.1,localhost");
            scoped_env.set("no_proxy", "127.0.0.1,localhost");
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind offline fixture");
        let address = listener.local_addr().expect("offline fixture address");
        let rejector = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let secret = "credential-query-must-not-leak";
        let source_url =
            format!("https://{address}/private/catalog.json?api_key={secret}#operator-fragment");
        let cli = Cli::try_parse_from([
            "codex-helper",
            "pricing",
            "force-refresh",
            "--url",
            source_url.as_str(),
            "--json",
        ])
        .expect("parse pricing force-refresh command");
        let Some(Command::Pricing { cmd }) = cli.command else {
            panic!("expected pricing command");
        };
        let mut stdout = Vec::new();
        let result = handle_pricing_cmd_with_writer(cmd, &mut stdout).await;
        rejector.abort();
        let _ = rejector.await;
        result.expect("run pricing force-refresh command");

        let text = String::from_utf8(stdout).expect("UTF-8 stdout");
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("pricing force-refresh JSON");
        assert_eq!(value["outcome"], "unavailable");
        assert_eq!(value["error_category"], "transport");
        assert_eq!(value["status"]["remote_state"], "last_error");
        assert_eq!(value["status"]["last_error_category"], "transport");
        assert!(value["status"]["source_url"].is_string());
        for forbidden in [
            secret,
            "api_key",
            "operator-fragment",
            "user:credential",
            "etag",
            "last_modified",
            "raw_payload",
        ] {
            assert!(!text.contains(forbidden), "leaked {forbidden}: {text}");
        }
    }
}

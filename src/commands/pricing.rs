use crate::cli_types::PricingCommand;
use crate::{CliError, CliResult};
use codex_helper_core::pricing::{
    self, CostConfidence, LocalModelPriceOverride, LocalModelPriceOverridesDocument,
    ModelPriceCatalogSnapshot, ModelPriceView,
};
use owo_colors::OwoColorize;
use std::time::Duration;

const PRICING_SYNC_TIMEOUT_SECS: u64 = 20;

pub async fn handle_pricing_cmd(cmd: PricingCommand) -> CliResult<()> {
    match cmd {
        PricingCommand::Path => {
            println!("{}", pricing::model_price_overrides_path().display());
        }
        PricingCommand::List { json, local, model } => {
            let snapshot = load_snapshot_for_list(local)?;
            let snapshot = filter_snapshot(snapshot, model.as_deref());

            if json {
                let text = serde_json::to_string_pretty(&snapshot)
                    .map_err(|e| CliError::Pricing(e.to_string()))?;
                println!("{text}");
            } else {
                print_snapshot_text(&snapshot, local, model.as_deref());
            }
        }
        PricingCommand::Set {
            model_id,
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

            document.models.insert(
                model_id.clone(),
                LocalModelPriceOverride {
                    display_name,
                    aliases,
                    input_per_1m_usd,
                    output_per_1m_usd,
                    cache_read_input_per_1m_usd,
                    cache_creation_input_per_1m_usd,
                    confidence: Some(confidence.into()),
                },
            );

            let path = pricing::save_model_price_overrides_document(&document)
                .map_err(CliError::Pricing)?;
            println!(
                "Updated local price override for '{}' at {:?}",
                model_id, path
            );
        }
        PricingCommand::Remove { model_id } => {
            let model_id = normalize_model_id(&model_id)?;
            let mut document = pricing::load_model_price_overrides_document()
                .and_then(|document| document.normalized())
                .map_err(CliError::Pricing)?;
            if document.models.remove(&model_id).is_none() {
                println!(
                    "No local price override found for '{}'; nothing changed.",
                    model_id
                );
                return Ok(());
            }

            let path = pricing::save_model_price_overrides_document(&document)
                .map_err(CliError::Pricing)?;
            println!(
                "Removed local price override for '{}' and rewrote {:?}",
                model_id, path
            );
        }
        PricingCommand::Sync {
            url,
            models,
            replace,
            dry_run,
        } => {
            let snapshot = fetch_remote_pricing_catalog(&url).await?;
            import_snapshot(snapshot, models, replace, dry_run)?;
        }
        PricingCommand::SyncBasellm {
            url,
            models,
            replace,
            dry_run,
        } => {
            let text = fetch_remote_pricing_text(&url).await?;
            let snapshot = pricing::basellm_model_price_catalog_snapshot_from_json(&url, &text)
                .map_err(CliError::Pricing)?;
            import_snapshot(snapshot, models, replace, dry_run)?;
        }
    }

    Ok(())
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
    let (document, imported) = merge_snapshot_into_overrides(base_document, &snapshot, &models)?;

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
) -> Result<(LocalModelPriceOverridesDocument, usize), CliError> {
    let filters = model_filters
        .iter()
        .map(|model| normalize_model_id(model))
        .collect::<Result<Vec<_>, _>>()?;
    let mut imported = 0usize;

    for row in &snapshot.models {
        if !filters.is_empty()
            && !filters
                .iter()
                .any(|filter| row.matches_model(filter.as_str()))
        {
            continue;
        }

        let model_id = normalize_model_id(&row.model_id)?;
        document.models.insert(
            model_id,
            LocalModelPriceOverride {
                display_name: row.display_name.clone(),
                aliases: row.aliases.clone(),
                input_per_1m_usd: row.input_per_1m_usd.clone(),
                output_per_1m_usd: row.output_per_1m_usd.clone(),
                cache_read_input_per_1m_usd: row.cache_read_input_per_1m_usd.clone(),
                cache_creation_input_per_1m_usd: row.cache_creation_input_per_1m_usd.clone(),
                confidence: Some(row.confidence),
            },
        );
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
) -> ModelPriceCatalogSnapshot {
    let Some(model) = model.map(str::trim).filter(|value| !value.is_empty()) else {
        return snapshot;
    };

    snapshot.models.retain(|row| row.matches_model(model));
    snapshot.model_count = snapshot.models.len();
    snapshot
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
        "model | input/1m | output/1m | cache read/1m | cache create/1m | confidence | source | aliases"
            .bold()
    );
    for row in &snapshot.models {
        print_snapshot_row(row);
    }
}

fn print_snapshot_row(row: &ModelPriceView) {
    let label = match row.display_name.as_deref() {
        Some(display_name) if !display_name.trim().is_empty() => {
            format!("{} [{}]", row.model_id, display_name)
        }
        _ => row.model_id.clone(),
    };
    let aliases = if row.aliases.is_empty() {
        "-".to_string()
    } else {
        row.aliases.join(", ")
    };

    println!(
        "{} | {} | {} | {} | {} | {} | {} | {}",
        label,
        format_usd(row.input_per_1m_usd.as_str()),
        format_usd(row.output_per_1m_usd.as_str()),
        format_optional_usd(row.cache_read_input_per_1m_usd.as_deref()),
        format_optional_usd(row.cache_creation_input_per_1m_usd.as_deref()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn price_row(model_id: &str, aliases: Vec<&str>) -> ModelPriceView {
        ModelPriceView {
            model_id: model_id.to_string(),
            display_name: Some(format!("{model_id} display")),
            aliases: aliases.into_iter().map(str::to_string).collect(),
            input_per_1m_usd: "1.25".to_string(),
            output_per_1m_usd: "10.00".to_string(),
            cache_read_input_per_1m_usd: Some("0.125".to_string()),
            cache_creation_input_per_1m_usd: None,
            source: "remote-test".to_string(),
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
            confidence: Some(CostConfidence::Estimated),
        }
    }

    #[test]
    fn sync_merge_imports_remote_rows_without_dropping_existing_overrides() {
        let mut document = LocalModelPriceOverridesDocument::default();
        document
            .models
            .insert("existing-model".to_string(), local_override("0.10", "0.20"));
        let snapshot = catalog_snapshot(vec![price_row("remote-model", vec!["relay-fast"])]);

        let (document, imported) =
            merge_snapshot_into_overrides(document, &snapshot, &[]).expect("merge");

        assert_eq!(imported, 1);
        assert!(document.models.contains_key("existing-model"));
        let imported = document.models.get("remote-model").expect("remote model");
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
            price_row("gpt-relay", vec!["relay-fast"]),
            price_row("other-model", vec![]),
        ]);

        let (document, imported) = merge_snapshot_into_overrides(
            LocalModelPriceOverridesDocument::default(),
            &snapshot,
            &[String::from("relay-fast")],
        )
        .expect("merge");

        assert_eq!(imported, 1);
        assert!(document.models.contains_key("gpt-relay"));
        assert!(!document.models.contains_key("other-model"));
    }
}

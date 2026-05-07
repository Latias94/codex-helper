use crate::cli_types::PricingCommand;
use crate::{CliError, CliResult};
use codex_helper_core::pricing::{
    self, CostConfidence, LocalModelPriceOverride, ModelPriceCatalogSnapshot, ModelPriceView,
};
use owo_colors::OwoColorize;

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
    }

    Ok(())
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

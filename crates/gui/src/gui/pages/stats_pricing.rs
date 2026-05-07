use std::collections::HashMap;

use crate::pricing::{CostConfidence, ModelPriceView};

use super::*;

pub(super) fn render_pricing_catalog(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };
    let catalog = &snapshot.pricing_catalog;

    ui.separator();
    ui.label(pick(ctx.lang, "价格目录", "Pricing catalog"));
    ui.label(format!(
        "source={}  models={}  api={}",
        catalog.source,
        catalog.model_count,
        if snapshot.supports_pricing_catalog_api {
            pick(ctx.lang, "supported", "supported")
        } else {
            pick(ctx.lang, "bundled fallback", "bundled fallback")
        }
    ));

    if catalog.models.is_empty() {
        ui.label(pick(
            ctx.lang,
            "当前价格目录为空，成本会继续显示为未知。",
            "The current price catalog is empty, so costs remain unknown.",
        ));
        return;
    }

    let rows = catalog.prioritized_models(recent_model_order(&snapshot), 30);
    egui::ScrollArea::vertical()
        .id_salt("stats_pricing_catalog_scroll")
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_pricing_catalog_grid")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "模型", "Model"));
                    ui.label("input / 1m");
                    ui.label("output / 1m");
                    ui.label("cache read / 1m");
                    ui.label("cache create / 1m");
                    ui.label(pick(ctx.lang, "来源", "Source"));
                    ui.end_row();

                    for row in rows {
                        ui.label(shorten(&price_model_label(row), 28));
                        ui.label(format_price(&row.input_per_1m_usd));
                        ui.label(format_price(&row.output_per_1m_usd));
                        ui.label(format_optional_price(
                            row.cache_read_input_per_1m_usd.as_deref(),
                        ));
                        ui.label(format_optional_price(
                            row.cache_creation_input_per_1m_usd.as_deref(),
                        ));
                        ui.label(shorten(
                            format!("{} / {}", confidence_label(row.confidence), row.source)
                                .as_str(),
                            30,
                        ));
                        ui.end_row();
                    }
                });
        });
}

fn recent_model_order(snapshot: &GuiRuntimeSnapshot) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for request in &snapshot.recent {
        if let Some(model) = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }
    for card in &snapshot.session_cards {
        if let Some(model) = card
            .effective_model
            .as_ref()
            .map(|value| value.value.as_str())
            .or(card.last_model.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }

    let mut models = counts.into_iter().collect::<Vec<_>>();
    models.sort_by(|(left_model, left_count), (right_model, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_model.cmp(right_model))
    });
    models.into_iter().map(|(model, _)| model).collect()
}

fn price_model_label(row: &ModelPriceView) -> String {
    match row.display_name.as_deref() {
        Some(display) if display != row.model_id => format!("{display} ({})", row.model_id),
        Some(display) => display.to_string(),
        None => row.model_id.clone(),
    }
}

fn format_price(value: &str) -> String {
    format!("${value}")
}

fn format_optional_price(value: Option<&str>) -> String {
    value.map(format_price).unwrap_or_else(|| "-".to_string())
}

fn confidence_label(confidence: CostConfidence) -> &'static str {
    match confidence {
        CostConfidence::Unknown => "unknown",
        CostConfidence::Partial => "partial",
        CostConfidence::Estimated => "estimated",
        CostConfidence::Exact => "exact",
    }
}

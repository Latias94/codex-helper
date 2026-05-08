use std::collections::BTreeSet;
use std::time::Duration;

use crate::pricing::{
    CostConfidence, LocalModelPriceOverride, LocalModelPriceOverridesDocument,
    ModelPriceCatalogSnapshot, ModelPriceView,
};

use super::view_state::StatsPricingEditorState;
use super::*;

pub(super) fn render_local_pricing_overrides(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) {
    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "本地价格覆盖", "Local pricing overrides"));
    let path = crate::pricing::model_price_overrides_path();
    ui.small(format!("path={}", path.display()));

    if !matches!(snapshot.kind, ProxyModeKind::Running) {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "附着模式只展示远端价格目录；本地价格覆盖编辑只在本机运行代理时启用，避免误写当前机器但不影响远端代理。",
                "Attached mode only shows the remote pricing catalog; local override editing is enabled only while this GUI is running the proxy locally.",
            ),
        );
        return;
    }

    let document = match load_local_pricing_overrides_document() {
        Ok(document) => document,
        Err(err) => {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                format!("failed to load local pricing overrides: {err}"),
            );
            if ui
                .button(pick(ctx.lang, "打开覆盖文件", "Open overrides file"))
                .clicked()
            {
                open_pricing_overrides_file(ctx);
            }
            return;
        }
    };

    render_observed_unpriced_models(ui, ctx, snapshot);
    render_local_pricing_override_rows(ui, ctx, &document);
    render_local_pricing_override_form(ui, ctx, &document);
}

fn load_local_pricing_overrides_document() -> Result<LocalModelPriceOverridesDocument, String> {
    crate::pricing::load_model_price_overrides_document().and_then(|document| document.normalized())
}

fn render_observed_unpriced_models(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) {
    let models = observed_unpriced_models(snapshot, 12);
    if models.is_empty() {
        return;
    }

    ui.horizontal_wrapped(|ui| {
        ui.small(pick(ctx.lang, "最近观测到但未定价：", "Observed unpriced:"));
        for model in models {
            if ui.button(shorten(&model, 28)).clicked() {
                start_pricing_editor_for_model(&mut ctx.view.stats.pricing_editor, &model);
            }
        }
    });
}

fn observed_unpriced_models(snapshot: &GuiRuntimeSnapshot, limit: usize) -> Vec<String> {
    observed_unpriced_models_from_candidates(
        snapshot
            .recent
            .iter()
            .filter_map(|request| request.model.as_deref())
            .chain(snapshot.session_cards.iter().filter_map(|card| {
                card.effective_model
                    .as_ref()
                    .map(|value| value.value.as_str())
                    .or(card.last_model.as_deref())
            })),
        &snapshot.pricing_catalog,
        limit,
    )
}

fn observed_unpriced_models_from_candidates<I, S>(
    observed_models: I,
    catalog: &ModelPriceCatalogSnapshot,
    limit: usize,
) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for model in observed_models {
        let model = model.as_ref().trim();
        if model.is_empty() {
            continue;
        }
        let key = model.to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }
        if catalog.models.iter().any(|row| row.matches_model(model)) {
            continue;
        }
        out.push(model.to_string());
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn render_local_pricing_override_rows(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    document: &LocalModelPriceOverridesDocument,
) {
    if document.models.is_empty() {
        ui.label(pick(
            ctx.lang,
            "还没有本地覆盖。保存下面的表单后会创建 pricing_overrides.toml。",
            "No local overrides yet. Saving the form below will create pricing_overrides.toml.",
        ));
        return;
    }

    let current = ctx.view.stats.pricing_editor.selected_model_id.clone();
    let mut selected_model_id = None;

    egui::ScrollArea::vertical()
        .id_salt("stats_pricing_local_overrides_scroll")
        .max_height(180.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_pricing_local_overrides_grid")
                .striped(true)
                .num_columns(6)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "模型", "Model"));
                    ui.label(pick(ctx.lang, "显示名", "Display"));
                    ui.label("input / 1m");
                    ui.label("output / 1m");
                    ui.label("cache");
                    ui.label(pick(ctx.lang, "置信度", "Confidence"));
                    ui.end_row();

                    for (model_id, row) in &document.models {
                        if ui
                            .selectable_label(
                                current.as_deref() == Some(model_id.as_str()),
                                shorten(model_id, 28),
                            )
                            .clicked()
                        {
                            selected_model_id = Some(model_id.clone());
                        }
                        ui.label(
                            row.display_name
                                .as_deref()
                                .filter(|value| !value.trim().is_empty())
                                .map(|value| shorten(value, 28))
                                .unwrap_or_else(|| "-".to_string()),
                        );
                        ui.label(format_price(&row.input_per_1m_usd));
                        ui.label(format_price(&row.output_per_1m_usd));
                        ui.label(shorten(
                            format!(
                                "read={} create={}",
                                format_optional_price(row.cache_read_input_per_1m_usd.as_deref()),
                                format_optional_price(
                                    row.cache_creation_input_per_1m_usd.as_deref()
                                )
                            )
                            .as_str(),
                            36,
                        ));
                        ui.label(confidence_label(
                            row.confidence.unwrap_or(CostConfidence::Estimated),
                        ));
                        ui.end_row();
                    }
                });
        });

    if let Some(model_id) = selected_model_id
        && let Some(row) = document.models.get(&model_id)
    {
        load_pricing_editor_from_override(&mut ctx.view.stats.pricing_editor, &model_id, row);
    }
}

fn render_local_pricing_override_form(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    document: &LocalModelPriceOverridesDocument,
) {
    let mut new_clicked = false;
    let mut reload_clicked = false;
    let mut save_clicked = false;
    let mut delete_clicked = false;
    let mut open_clicked = false;

    ui.group(|ui| {
        let editor = &mut ctx.view.stats.pricing_editor;
        let selected = editor
            .selected_model_id
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "新覆盖", "New override"));
        ui.horizontal(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "当前编辑", "Editing"),
                selected
            ));
            if ui.button(pick(ctx.lang, "新建", "New")).clicked() {
                new_clicked = true;
            }
            if ui
                .add_enabled(
                    editor.selected_model_id.is_some(),
                    egui::Button::new(pick(ctx.lang, "从磁盘重载所选", "Reload selected")),
                )
                .clicked()
            {
                reload_clicked = true;
            }
            if ui
                .button(pick(ctx.lang, "打开覆盖文件", "Open overrides file"))
                .clicked()
            {
                open_clicked = true;
            }
        });

        ui.add_space(6.0);
        egui::Grid::new("stats_pricing_override_editor_grid")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("model id");
                ui.add_sized(
                    [260.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.draft_model_id),
                );
                ui.end_row();

                ui.label(pick(ctx.lang, "显示名", "Display name"));
                ui.add_sized(
                    [260.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.display_name),
                );
                ui.end_row();

                ui.label("aliases");
                ui.add_sized(
                    [360.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.aliases),
                );
                ui.end_row();

                ui.label("input / 1m USD");
                ui.add_sized(
                    [120.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.input_per_1m_usd),
                );
                ui.end_row();

                ui.label("output / 1m USD");
                ui.add_sized(
                    [120.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.output_per_1m_usd),
                );
                ui.end_row();

                ui.label("cache read / 1m USD");
                ui.add_sized(
                    [120.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.cache_read_input_per_1m_usd),
                );
                ui.end_row();

                ui.label("cache create / 1m USD");
                ui.add_sized(
                    [120.0, 22.0],
                    egui::TextEdit::singleline(&mut editor.cache_creation_input_per_1m_usd),
                );
                ui.end_row();

                ui.label(pick(ctx.lang, "置信度", "Confidence"));
                egui::ComboBox::from_id_salt("stats_pricing_override_confidence")
                    .selected_text(confidence_label(editor.confidence))
                    .show_ui(ui, |ui| {
                        for confidence in [
                            CostConfidence::Estimated,
                            CostConfidence::Exact,
                            CostConfidence::Partial,
                            CostConfidence::Unknown,
                        ] {
                            ui.selectable_value(
                                &mut editor.confidence,
                                confidence,
                                confidence_label(confidence),
                            );
                        }
                    });
                ui.end_row();
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "保存本地覆盖", "Save local override"))
                .clicked()
            {
                save_clicked = true;
            }
            if ui
                .add_enabled(
                    editor.selected_model_id.is_some(),
                    egui::Button::new(pick(ctx.lang, "删除所选", "Delete selected")),
                )
                .clicked()
            {
                delete_clicked = true;
            }
        });
    });

    if new_clicked {
        clear_pricing_editor(&mut ctx.view.stats.pricing_editor);
    }
    if reload_clicked {
        reload_selected_pricing_editor_from_document(ctx, document);
    }
    if open_clicked {
        open_pricing_overrides_file(ctx);
    }
    if save_clicked {
        save_pricing_override_from_editor(ctx);
    }
    if delete_clicked {
        delete_selected_pricing_override(ctx);
    }
}

fn reload_selected_pricing_editor_from_document(
    ctx: &mut PageCtx<'_>,
    document: &LocalModelPriceOverridesDocument,
) {
    let Some(model_id) = ctx.view.stats.pricing_editor.selected_model_id.clone() else {
        return;
    };
    match document.models.get(&model_id) {
        Some(row) => {
            load_pricing_editor_from_override(&mut ctx.view.stats.pricing_editor, &model_id, row);
            *ctx.last_info = Some(
                pick(
                    ctx.lang,
                    "已从磁盘重载所选价格覆盖",
                    "Reloaded selected pricing override from disk",
                )
                .to_string(),
            );
            *ctx.last_error = None;
        }
        None => {
            clear_pricing_editor(&mut ctx.view.stats.pricing_editor);
            *ctx.last_error = Some(format!("pricing override '{model_id}' no longer exists"));
        }
    }
}

fn save_pricing_override_from_editor(ctx: &mut PageCtx<'_>) {
    let old_model_id = ctx.view.stats.pricing_editor.selected_model_id.clone();
    let (model_id, row) = match build_pricing_override_from_editor(&ctx.view.stats.pricing_editor) {
        Ok(row) => row,
        Err(err) => {
            *ctx.last_error = Some(format!("invalid pricing override: {err}"));
            return;
        }
    };

    let mut document = match load_local_pricing_overrides_document() {
        Ok(document) => document,
        Err(err) => {
            *ctx.last_error = Some(format!("failed to load local pricing overrides: {err}"));
            return;
        }
    };

    if let Some(old) = old_model_id.as_deref()
        && old != model_id
        && document.models.contains_key(&model_id)
    {
        *ctx.last_error = Some(format!(
            "pricing override '{model_id}' already exists; select it before editing"
        ));
        return;
    }

    if let Some(old) = old_model_id.as_deref()
        && old != model_id
    {
        document.models.remove(old);
    }
    document.models.insert(model_id.clone(), row);

    match crate::pricing::save_model_price_overrides_document(&document) {
        Ok(path) => {
            if let Ok(reloaded) = load_local_pricing_overrides_document()
                && let Some(row) = reloaded.models.get(&model_id)
            {
                load_pricing_editor_from_override(
                    &mut ctx.view.stats.pricing_editor,
                    &model_id,
                    row,
                );
            }
            ctx.proxy
                .refresh_current_if_due(ctx.rt, Duration::from_secs(0));
            *ctx.last_info = Some(format!(
                "{}: {}",
                pick(
                    ctx.lang,
                    "已保存本地价格覆盖",
                    "Saved local pricing override"
                ),
                path.display()
            ));
            *ctx.last_error = None;
        }
        Err(err) => {
            *ctx.last_error = Some(format!("failed to save local pricing overrides: {err}"));
        }
    }
}

pub(super) fn import_catalog_price_to_local_override(ctx: &mut PageCtx<'_>, row: &ModelPriceView) {
    let model_id = row.model_id.trim();
    if model_id.is_empty() {
        *ctx.last_error = Some("pricing catalog row has an empty model id".to_string());
        return;
    }

    let mut document = match load_local_pricing_overrides_document() {
        Ok(document) => document,
        Err(err) => {
            *ctx.last_error = Some(format!("failed to load local pricing overrides: {err}"));
            return;
        }
    };
    let override_row = local_override_from_catalog_row(row);
    document
        .models
        .insert(model_id.to_string(), override_row.clone());

    match crate::pricing::save_model_price_overrides_document(&document) {
        Ok(path) => {
            load_pricing_editor_from_override(
                &mut ctx.view.stats.pricing_editor,
                model_id,
                &override_row,
            );
            ctx.proxy
                .refresh_current_if_due(ctx.rt, Duration::from_secs(0));
            *ctx.last_info = Some(format!(
                "{} '{}' ({})",
                pick(
                    ctx.lang,
                    "已从价格目录保存本地覆盖",
                    "Saved local override from catalog"
                ),
                model_id,
                path.display()
            ));
            *ctx.last_error = None;
        }
        Err(err) => {
            *ctx.last_error = Some(format!("failed to save local pricing overrides: {err}"));
        }
    }
}

fn local_override_from_catalog_row(row: &ModelPriceView) -> LocalModelPriceOverride {
    LocalModelPriceOverride {
        display_name: row.display_name.clone(),
        aliases: row.aliases.clone(),
        input_per_1m_usd: row.input_per_1m_usd.clone(),
        output_per_1m_usd: row.output_per_1m_usd.clone(),
        cache_read_input_per_1m_usd: row.cache_read_input_per_1m_usd.clone(),
        cache_creation_input_per_1m_usd: row.cache_creation_input_per_1m_usd.clone(),
        confidence: Some(row.confidence),
    }
}

fn delete_selected_pricing_override(ctx: &mut PageCtx<'_>) {
    let Some(model_id) = ctx.view.stats.pricing_editor.selected_model_id.clone() else {
        return;
    };
    let mut document = match load_local_pricing_overrides_document() {
        Ok(document) => document,
        Err(err) => {
            *ctx.last_error = Some(format!("failed to load local pricing overrides: {err}"));
            return;
        }
    };
    if document.models.remove(&model_id).is_none() {
        clear_pricing_editor(&mut ctx.view.stats.pricing_editor);
        *ctx.last_error = Some(format!("pricing override '{model_id}' no longer exists"));
        return;
    }

    match crate::pricing::save_model_price_overrides_document(&document) {
        Ok(path) => {
            clear_pricing_editor(&mut ctx.view.stats.pricing_editor);
            ctx.proxy
                .refresh_current_if_due(ctx.rt, Duration::from_secs(0));
            *ctx.last_info = Some(format!(
                "{} '{}' ({})",
                pick(
                    ctx.lang,
                    "已删除本地价格覆盖",
                    "Deleted local pricing override"
                ),
                model_id,
                path.display()
            ));
            *ctx.last_error = None;
        }
        Err(err) => {
            *ctx.last_error = Some(format!("failed to save local pricing overrides: {err}"));
        }
    }
}

fn open_pricing_overrides_file(ctx: &mut PageCtx<'_>) {
    let path = crate::pricing::model_price_overrides_path();
    let (target, select_file) = if path.exists() {
        (path.as_path(), true)
    } else {
        (path.parent().unwrap_or(path.as_path()), false)
    };

    if let Err(err) = open_in_file_manager(target, select_file) {
        *ctx.last_error = Some(format!("open pricing overrides failed: {err}"));
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

fn load_pricing_editor_from_override(
    editor: &mut StatsPricingEditorState,
    model_id: &str,
    row: &LocalModelPriceOverride,
) {
    editor.selected_model_id = Some(model_id.to_string());
    editor.draft_model_id = model_id.to_string();
    editor.display_name = row.display_name.clone().unwrap_or_default();
    editor.aliases = join_aliases(&row.aliases);
    editor.input_per_1m_usd = row.input_per_1m_usd.clone();
    editor.output_per_1m_usd = row.output_per_1m_usd.clone();
    editor.cache_read_input_per_1m_usd =
        row.cache_read_input_per_1m_usd.clone().unwrap_or_default();
    editor.cache_creation_input_per_1m_usd = row
        .cache_creation_input_per_1m_usd
        .clone()
        .unwrap_or_default();
    editor.confidence = row.confidence.unwrap_or(CostConfidence::Estimated);
}

fn clear_pricing_editor(editor: &mut StatsPricingEditorState) {
    *editor = StatsPricingEditorState::default();
}

fn start_pricing_editor_for_model(editor: &mut StatsPricingEditorState, model_id: &str) {
    clear_pricing_editor(editor);
    editor.draft_model_id = model_id.trim().to_string();
}

fn build_pricing_override_from_editor(
    editor: &StatsPricingEditorState,
) -> Result<(String, LocalModelPriceOverride), String> {
    let model_id = editor.draft_model_id.trim();
    if model_id.is_empty() {
        return Err("model id cannot be empty".to_string());
    }

    let row = LocalModelPriceOverride {
        display_name: optional_trimmed_string(&editor.display_name),
        aliases: parse_aliases(&editor.aliases),
        input_per_1m_usd: editor.input_per_1m_usd.trim().to_string(),
        output_per_1m_usd: editor.output_per_1m_usd.trim().to_string(),
        cache_read_input_per_1m_usd: optional_trimmed_string(&editor.cache_read_input_per_1m_usd),
        cache_creation_input_per_1m_usd: optional_trimmed_string(
            &editor.cache_creation_input_per_1m_usd,
        ),
        confidence: Some(editor.confidence),
    };
    let sanitized = row.sanitized(model_id)?;
    Ok((model_id.to_string(), sanitized))
}

fn optional_trimmed_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_aliases(value: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut aliases = Vec::new();
    for alias in value.split([',', '\n', ';']) {
        let alias = alias.trim();
        if alias.is_empty() {
            continue;
        }
        let key = alias.to_ascii_lowercase();
        if seen.insert(key) {
            aliases.push(alias.to_string());
        }
    }
    aliases
}

fn join_aliases(aliases: &[String]) -> String {
    aliases.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_aliases_trims_splits_and_deduplicates_case_insensitively() {
        assert_eq!(
            parse_aliases(" relay-gpt5,relay-fast\nRelay-GPT5; custom "),
            vec!["relay-gpt5", "relay-fast", "custom"]
        );
    }

    #[test]
    fn build_pricing_override_from_editor_trims_optional_fields() {
        let editor = StatsPricingEditorState {
            draft_model_id: " custom-codex ".to_string(),
            display_name: "  Custom Codex  ".to_string(),
            aliases: " relay-custom, custom-codex ".to_string(),
            input_per_1m_usd: " 0.50 ".to_string(),
            output_per_1m_usd: " 1.50 ".to_string(),
            cache_read_input_per_1m_usd: " ".to_string(),
            cache_creation_input_per_1m_usd: " 0 ".to_string(),
            confidence: CostConfidence::Exact,
            ..StatsPricingEditorState::default()
        };

        let (model_id, row) = build_pricing_override_from_editor(&editor).expect("valid override");

        assert_eq!(model_id, "custom-codex");
        assert_eq!(row.display_name.as_deref(), Some("Custom Codex"));
        assert_eq!(row.aliases, vec!["relay-custom"]);
        assert_eq!(row.input_per_1m_usd, "0.50");
        assert_eq!(row.output_per_1m_usd, "1.50");
        assert_eq!(row.cache_read_input_per_1m_usd, None);
        assert_eq!(row.cache_creation_input_per_1m_usd.as_deref(), Some("0"));
        assert_eq!(row.confidence, Some(CostConfidence::Exact));
    }

    #[test]
    fn load_pricing_editor_from_override_round_trips_fields() {
        let row = LocalModelPriceOverride {
            display_name: Some("Relay GPT".to_string()),
            aliases: vec!["relay-gpt".to_string(), "relay-fast".to_string()],
            input_per_1m_usd: "1".to_string(),
            output_per_1m_usd: "2".to_string(),
            cache_read_input_per_1m_usd: Some("0.1".to_string()),
            cache_creation_input_per_1m_usd: None,
            confidence: Some(CostConfidence::Estimated),
        };
        let mut editor = StatsPricingEditorState::default();

        load_pricing_editor_from_override(&mut editor, "gpt-relay", &row);

        assert_eq!(editor.selected_model_id.as_deref(), Some("gpt-relay"));
        assert_eq!(editor.draft_model_id, "gpt-relay");
        assert_eq!(editor.display_name, "Relay GPT");
        assert_eq!(editor.aliases, "relay-gpt, relay-fast");
        assert_eq!(editor.input_per_1m_usd, "1");
        assert_eq!(editor.cache_read_input_per_1m_usd, "0.1");
        assert_eq!(editor.cache_creation_input_per_1m_usd, "");
        assert_eq!(editor.confidence, CostConfidence::Estimated);
    }

    #[test]
    fn observed_unpriced_models_filters_catalog_matches_and_duplicates() {
        let catalog = crate::pricing::bundled_model_price_catalog_snapshot();

        let models = observed_unpriced_models_from_candidates(
            [
                "gpt-5.4-high",
                "relay-gpt-5",
                "RELAY-GPT-5",
                "",
                "relay-codex",
            ],
            &catalog,
            8,
        );

        assert_eq!(models, vec!["relay-gpt-5", "relay-codex"]);
    }

    #[test]
    fn local_override_from_catalog_row_preserves_price_fields() {
        let row = ModelPriceView {
            model_id: "relay-model".to_string(),
            display_name: Some("Relay Model".to_string()),
            aliases: vec!["alias-a".to_string(), "alias-b".to_string()],
            input_per_1m_usd: "1.25".to_string(),
            output_per_1m_usd: "9.50".to_string(),
            cache_read_input_per_1m_usd: Some("0.125".to_string()),
            cache_creation_input_per_1m_usd: Some("1.00".to_string()),
            source: "remote-catalog".to_string(),
            confidence: CostConfidence::Exact,
        };

        let override_row = local_override_from_catalog_row(&row);

        assert_eq!(override_row.display_name.as_deref(), Some("Relay Model"));
        assert_eq!(override_row.aliases, vec!["alias-a", "alias-b"]);
        assert_eq!(override_row.input_per_1m_usd, "1.25");
        assert_eq!(override_row.output_per_1m_usd, "9.50");
        assert_eq!(
            override_row.cache_read_input_per_1m_usd.as_deref(),
            Some("0.125")
        );
        assert_eq!(
            override_row.cache_creation_input_per_1m_usd.as_deref(),
            Some("1.00")
        );
        assert_eq!(override_row.confidence, Some(CostConfidence::Exact));
    }
}

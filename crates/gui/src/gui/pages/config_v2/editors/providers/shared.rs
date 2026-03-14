use super::*;

pub(super) struct ProviderCardItem {
    pub(super) name: String,
    pub(super) alias: String,
    pub(super) station_summary: String,
    pub(super) auth_summary: String,
    pub(super) route_summary: String,
    pub(super) endpoint_summary: String,
    pub(super) station_names: Vec<String>,
    pub(super) is_selected: bool,
    pub(super) enabled: bool,
    pub(super) failover_ready: bool,
    pub(super) endpoint_count: usize,
    pub(super) enabled_endpoint_count: usize,
    pub(super) station_count: usize,
    pub(super) has_auth_token_env: bool,
    pub(super) has_api_key_env: bool,
}

pub(super) fn build_provider_card_item(
    lang: Language,
    provider: &PersistedProviderSpec,
    station_names: &[String],
    is_selected: bool,
) -> ProviderCardItem {
    let alias = provider.alias.clone().unwrap_or_default();
    let endpoint_count = provider.endpoints.len();
    let enabled_endpoint_count = provider
        .endpoints
        .iter()
        .filter(|item| item.enabled)
        .count();
    let failover_ready = enabled_endpoint_count > 1;
    let has_auth_token_env = provider.auth_token_env.is_some();
    let has_api_key_env = provider.api_key_env.is_some();

    ProviderCardItem {
        name: provider.name.clone(),
        alias,
        station_summary: format_station_summary(lang, station_names),
        auth_summary: format_auth_summary(lang, provider),
        route_summary: format_route_summary(lang, endpoint_count, enabled_endpoint_count),
        endpoint_summary: format_endpoint_summary(lang, endpoint_count, enabled_endpoint_count),
        station_names: station_names.to_vec(),
        is_selected,
        enabled: provider.enabled,
        failover_ready,
        endpoint_count,
        enabled_endpoint_count,
        station_count: station_names.len(),
        has_auth_token_env,
        has_api_key_env,
    }
}

pub(super) fn render_provider_card_list<F>(
    ui: &mut egui::Ui,
    lang: Language,
    id_salt: &str,
    empty_label: &str,
    items: &[ProviderCardItem],
    mut on_select: F,
) where
    F: FnMut(&str),
{
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(340.0)
        .show(ui, |ui| {
            if items.is_empty() {
                ui.label(empty_label);
                return;
            }

            for item in items {
                let stroke_color = if item.is_selected {
                    egui::Color32::from_rgb(76, 114, 176)
                } else {
                    egui::Color32::from_rgb(190, 190, 190)
                };
                let fill_color = if item.is_selected {
                    egui::Color32::from_rgb(243, 247, 255)
                } else {
                    egui::Color32::from_rgb(250, 250, 250)
                };
                let response = egui::Frame::group(ui.style())
                    .fill(fill_color)
                    .stroke(egui::Stroke::new(
                        if item.is_selected { 1.5 } else { 1.0 },
                        stroke_color,
                    ))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new(item.name.as_str()).strong());
                            if !item.alias.is_empty() {
                                render_provider_badge(
                                    ui,
                                    format!("alias {}", item.alias),
                                    egui::Color32::from_rgb(76, 114, 176),
                                );
                            }
                            if item.is_selected {
                                render_provider_badge(
                                    ui,
                                    pick(lang, "当前", "selected"),
                                    egui::Color32::from_rgb(76, 114, 176),
                                );
                            }
                            if !item.enabled {
                                render_provider_badge(
                                    ui,
                                    pick(lang, "off", "off"),
                                    egui::Color32::from_rgb(150, 150, 150),
                                );
                            }
                        });
                        ui.small(item.station_summary.as_str());
                        ui.small(format!("{} · {}", item.auth_summary, item.endpoint_summary));
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            render_provider_badge(
                                ui,
                                format!("stations {}", item.station_count),
                                egui::Color32::from_rgb(86, 122, 62),
                            );
                            render_provider_badge(
                                ui,
                                format!(
                                    "endpoints {}/{}",
                                    item.enabled_endpoint_count, item.endpoint_count
                                ),
                                egui::Color32::from_rgb(76, 114, 176),
                            );
                            render_provider_badge(
                                ui,
                                item.auth_summary.as_str(),
                                if item.has_auth_token_env || item.has_api_key_env {
                                    egui::Color32::from_rgb(176, 122, 76)
                                } else {
                                    egui::Color32::from_rgb(150, 150, 150)
                                },
                            );
                            render_provider_badge(
                                ui,
                                item.route_summary.as_str(),
                                if item.failover_ready {
                                    egui::Color32::from_rgb(86, 122, 62)
                                } else {
                                    egui::Color32::from_rgb(122, 90, 166)
                                },
                            );
                        });
                    })
                    .response
                    .interact(egui::Sense::click());

                if response.clicked() {
                    on_select(item.name.as_str());
                }
                ui.add_space(6.0);
            }
        });
}

fn format_station_summary(lang: Language, station_names: &[String]) -> String {
    if station_names.is_empty() {
        return pick(lang, "尚未被 station 引用", "Not used by any station").to_string();
    }

    let preview = match station_names {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first}, {second}"),
        [first, second, rest @ ..] => format!("{first}, {second} +{}", rest.len()),
    };
    format!("{} {}", pick(lang, "引用于", "Used by"), preview)
}

fn format_auth_summary(lang: Language, provider: &PersistedProviderSpec) -> String {
    match (
        provider.auth_token_env.as_deref(),
        provider.api_key_env.as_deref(),
    ) {
        (Some(_), Some(_)) => "auth_token_env + api_key_env".to_string(),
        (Some(_), None) => "auth_token_env".to_string(),
        (None, Some(_)) => "api_key_env".to_string(),
        (None, None) => pick(lang, "无 env 引用", "No auth env ref").to_string(),
    }
}

fn format_route_summary(
    lang: Language,
    endpoint_count: usize,
    enabled_endpoint_count: usize,
) -> String {
    match (endpoint_count, enabled_endpoint_count) {
        (0, _) => pick(lang, "empty", "empty").to_string(),
        (_, 0) => pick(lang, "all-off", "all-off").to_string(),
        (1, 1) => pick(lang, "single-path", "single-path").to_string(),
        (_, 1) => pick(lang, "single-active", "single-active").to_string(),
        _ => pick(lang, "fallback-ready", "fallback-ready").to_string(),
    }
}

fn format_endpoint_summary(
    lang: Language,
    endpoint_count: usize,
    enabled_endpoint_count: usize,
) -> String {
    if endpoint_count == 0 {
        return pick(lang, "尚未配置 endpoint", "No endpoints configured").to_string();
    }

    format!(
        "{enabled_endpoint_count}/{endpoint_count} {}",
        pick(lang, "endpoint 已启用", "endpoints enabled")
    )
}

fn render_provider_badge(ui: &mut egui::Ui, text: impl Into<String>, color: egui::Color32) {
    ui.label(
        egui::RichText::new(text.into())
            .small()
            .color(color)
            .background_color(color.gamma_multiply(0.10)),
    );
}

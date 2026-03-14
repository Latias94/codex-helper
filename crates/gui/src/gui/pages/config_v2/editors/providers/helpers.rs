use super::shared::ProviderCardItem;
use super::*;

pub(super) fn provider_spec_catalog<'a>(
    provider_structure_control_plane_enabled: bool,
    attached_provider_specs: Option<&'a BTreeMap<String, PersistedProviderSpec>>,
    local_provider_spec_catalog: &'a BTreeMap<String, PersistedProviderSpec>,
) -> Option<&'a BTreeMap<String, PersistedProviderSpec>> {
    if provider_structure_control_plane_enabled {
        attached_provider_specs
    } else {
        Some(local_provider_spec_catalog)
    }
}

pub(super) fn provider_spec_snapshot<'a>(
    name: &str,
    provider_structure_control_plane_enabled: bool,
    attached_provider_specs: Option<&'a BTreeMap<String, PersistedProviderSpec>>,
    local_provider_spec_catalog: &'a BTreeMap<String, PersistedProviderSpec>,
) -> Option<&'a PersistedProviderSpec> {
    provider_spec_catalog(
        provider_structure_control_plane_enabled,
        attached_provider_specs,
        local_provider_spec_catalog,
    )
    .and_then(|providers| providers.get(name))
}

pub(super) fn provider_station_refs(
    view: &crate::config::ServiceViewV2,
    attached_station_specs: Option<&(
        BTreeMap<String, PersistedStationSpec>,
        BTreeMap<String, PersistedStationProviderRef>,
    )>,
    provider_structure_control_plane_enabled: bool,
) -> BTreeMap<String, Vec<String>> {
    let mut refs = BTreeMap::<String, Vec<String>>::new();

    let mut push_station_refs =
        |station_name: &str, members: &[crate::config::GroupMemberRefV2]| {
            let mut seen = std::collections::BTreeSet::new();
            for member in members {
                if seen.insert(member.provider.clone()) {
                    refs.entry(member.provider.clone())
                        .or_default()
                        .push(station_name.to_string());
                }
            }
        };

    if provider_structure_control_plane_enabled {
        if let Some((stations, _)) = attached_station_specs {
            for (station_name, station) in stations {
                push_station_refs(station_name.as_str(), &station.members);
            }
        }
    } else {
        for (station_name, station) in &view.groups {
            push_station_refs(station_name.as_str(), &station.members);
        }
    }

    for names in refs.values_mut() {
        names.sort();
    }

    refs
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_provider_editor_from_selected(
    selected_provider_name: &Option<String>,
    provider_editor_name: &mut Option<String>,
    provider_editor_alias: &mut String,
    provider_editor_enabled: &mut bool,
    provider_editor_auth_token_env: &mut String,
    provider_editor_api_key_env: &mut String,
    provider_editor_endpoints: &mut Vec<ConfigProviderEndpointEditorState>,
    provider_specs: &BTreeMap<String, PersistedProviderSpec>,
) {
    if provider_editor_name.as_deref() == selected_provider_name.as_deref() {
        return;
    }

    let selected_provider = selected_provider_name
        .as_deref()
        .and_then(|name| provider_specs.get(name));
    *provider_editor_name = selected_provider_name.clone();
    *provider_editor_alias = selected_provider
        .and_then(|provider| provider.alias.clone())
        .unwrap_or_default();
    *provider_editor_enabled = selected_provider
        .map(|provider| provider.enabled)
        .unwrap_or(true);
    *provider_editor_auth_token_env = selected_provider
        .and_then(|provider| provider.auth_token_env.clone())
        .unwrap_or_default();
    *provider_editor_api_key_env = selected_provider
        .and_then(|provider| provider.api_key_env.clone())
        .unwrap_or_default();
    *provider_editor_endpoints = selected_provider
        .map(|provider| {
            provider
                .endpoints
                .iter()
                .map(super::endpoints::config_provider_endpoint_editor_from_spec)
                .collect()
        })
        .unwrap_or_default();
}

pub(super) fn render_provider_summary_card(
    ui: &mut egui::Ui,
    title: &str,
    value: impl Into<String>,
    hint: impl Into<String>,
) {
    ui.group(|ui| {
        ui.small(title);
        ui.heading(value.into());
        ui.small(hint.into());
    });
}

pub(super) fn render_provider_detail_badge(
    ui: &mut egui::Ui,
    text: impl Into<String>,
    color: egui::Color32,
) {
    ui.label(
        egui::RichText::new(text.into())
            .small()
            .color(color)
            .background_color(color.gamma_multiply(0.10)),
    );
}

pub(super) fn render_provider_overview_cards(
    ui: &mut egui::Ui,
    lang: Language,
    item: &ProviderCardItem,
) {
    ui.columns(3, |cols| {
        render_provider_summary_card(
            &mut cols[0],
            pick(lang, "Stations", "Stations"),
            item.station_count.to_string(),
            if item.station_names.is_empty() {
                pick(lang, "尚未接入路由", "Not attached to any station").to_string()
            } else {
                item.station_names.join(", ")
            },
        );
        render_provider_summary_card(
            &mut cols[1],
            pick(lang, "Auth", "Auth"),
            if item.has_auth_token_env || item.has_api_key_env {
                pick(lang, "已声明", "Configured").to_string()
            } else {
                pick(lang, "缺失", "Missing").to_string()
            },
            item.auth_summary.clone(),
        );
        render_provider_summary_card(
            &mut cols[2],
            pick(lang, "Routing", "Routing"),
            item.route_summary.clone(),
            item.endpoint_summary.clone(),
        );
    });
}

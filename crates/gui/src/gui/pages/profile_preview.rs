use std::collections::BTreeMap;

use eframe::egui;

use super::config_document::parse_proxy_config_document;
use super::runtime_station::capability_support_label;
use super::session_presentation::non_empty_trimmed;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfilePreviewStationSource {
    Profile,
    ConfiguredActive,
    Auto,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfilePreviewMemberRoute {
    pub(super) provider_name: String,
    pub(super) provider_alias: Option<String>,
    pub(super) provider_enabled: Option<bool>,
    pub(super) provider_missing: bool,
    pub(super) endpoint_names: Vec<String>,
    pub(super) uses_all_endpoints: bool,
    pub(super) preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileRoutePreview {
    pub(super) station_source: ProfilePreviewStationSource,
    pub(super) resolved_station_name: Option<String>,
    pub(super) station_exists: bool,
    pub(super) structure_available: bool,
    pub(super) station_alias: Option<String>,
    pub(super) station_enabled: Option<bool>,
    pub(super) station_level: Option<u8>,
    pub(super) members: Vec<ProfilePreviewMemberRoute>,
    pub(super) capabilities: Option<StationCapabilitySummary>,
    pub(super) model_supported: Option<bool>,
    pub(super) service_tier_supported: Option<bool>,
    pub(super) reasoning_supported: Option<bool>,
}

pub(super) fn build_profile_route_preview(
    profile: &crate::config::ServiceControlProfile,
    configured_active_station: Option<&str>,
    auto_station: Option<&str>,
    station_specs: Option<&BTreeMap<String, PersistedStationSpec>>,
    provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
    runtime_station_catalog: Option<&BTreeMap<String, StationOption>>,
) -> ProfileRoutePreview {
    let explicit_station = non_empty_trimmed(profile.station.as_deref());
    let configured_active_station = non_empty_trimmed(configured_active_station);
    let auto_station = non_empty_trimmed(auto_station);

    let (station_source, resolved_station_name) = if let Some(name) = explicit_station {
        (ProfilePreviewStationSource::Profile, Some(name))
    } else if let Some(name) = configured_active_station {
        (ProfilePreviewStationSource::ConfiguredActive, Some(name))
    } else if let Some(name) = auto_station {
        (ProfilePreviewStationSource::Auto, Some(name))
    } else {
        (ProfilePreviewStationSource::Unresolved, None)
    };

    let station_spec = resolved_station_name
        .as_deref()
        .and_then(|name| station_specs.and_then(|specs| specs.get(name)));
    let runtime_station = resolved_station_name
        .as_deref()
        .and_then(|name| runtime_station_catalog.and_then(|catalog| catalog.get(name)));
    let capabilities = runtime_station.map(|station| station.capabilities.clone());

    let members = station_spec
        .map(|station| {
            station
                .members
                .iter()
                .map(|member| {
                    let provider =
                        provider_catalog.and_then(|providers| providers.get(&member.provider));
                    let endpoint_names = if member.endpoint_names.is_empty() {
                        provider
                            .map(|provider| {
                                provider
                                    .endpoints
                                    .iter()
                                    .map(|endpoint| endpoint.name.clone())
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    } else {
                        member.endpoint_names.clone()
                    };
                    ProfilePreviewMemberRoute {
                        provider_name: member.provider.clone(),
                        provider_alias: provider.and_then(|provider| provider.alias.clone()),
                        provider_enabled: provider.map(|provider| provider.enabled),
                        provider_missing: provider.is_none(),
                        endpoint_names,
                        uses_all_endpoints: member.endpoint_names.is_empty(),
                        preferred: member.preferred,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let model_supported = profile
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .and_then(|model| {
            capabilities.as_ref().and_then(|capabilities| {
                if capabilities.supported_models.is_empty() {
                    None
                } else {
                    Some(
                        capabilities
                            .supported_models
                            .iter()
                            .any(|item| item == model),
                    )
                }
            })
        });
    let service_tier_supported = profile
        .service_tier
        .as_deref()
        .filter(|tier| !tier.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_service_tier)
            })
        });
    let reasoning_supported = profile
        .reasoning_effort
        .as_deref()
        .filter(|effort| !effort.trim().is_empty())
        .and_then(|_| {
            capabilities.as_ref().and_then(|capabilities| {
                capability_support_truthy(capabilities.supports_reasoning_effort)
            })
        });

    ProfileRoutePreview {
        station_source,
        station_exists: station_spec.is_some() || runtime_station.is_some(),
        structure_available: station_spec.is_some(),
        resolved_station_name,
        station_alias: station_spec
            .and_then(|station| station.alias.clone())
            .or_else(|| runtime_station.and_then(|station| station.alias.clone())),
        station_enabled: station_spec
            .map(|station| station.enabled)
            .or_else(|| runtime_station.map(|station| station.enabled)),
        station_level: station_spec
            .map(|station| station.level)
            .or_else(|| runtime_station.map(|station| station.level)),
        members,
        capabilities,
        model_supported,
        service_tier_supported,
        reasoning_supported,
    }
}

pub(super) fn local_profile_preview_catalogs_from_text(
    text: &str,
    service_name: &str,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    let ConfigWorkingDocument::V2(cfg) = parse_proxy_config_document(text).ok()? else {
        return None;
    };
    let view = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    let catalog = crate::config::build_persisted_station_catalog(view);
    Some((
        catalog
            .stations
            .into_iter()
            .map(|station| (station.name.clone(), station))
            .collect(),
        catalog
            .providers
            .into_iter()
            .map(|provider| (provider.name.clone(), provider))
            .collect(),
    ))
}

fn capability_support_truthy(support: CapabilitySupport) -> Option<bool> {
    match support {
        CapabilitySupport::Supported => Some(true),
        CapabilitySupport::Unsupported => Some(false),
        CapabilitySupport::Unknown => None,
    }
}

pub(super) fn render_profile_route_preview(
    ui: &mut egui::Ui,
    lang: Language,
    profile: &crate::config::ServiceControlProfile,
    preview: &ProfileRoutePreview,
) {
    ui.add_space(6.0);
    ui.group(|ui| {
        ui.label(pick(lang, "联动预览", "Linked preview"));

        let station_source = match preview.station_source {
            ProfilePreviewStationSource::Profile => pick(lang, "profile.station", "profile.station"),
            ProfilePreviewStationSource::ConfiguredActive => {
                pick(lang, "active_station", "active_station")
            }
            ProfilePreviewStationSource::Auto => pick(lang, "自动候选", "auto candidate"),
            ProfilePreviewStationSource::Unresolved => pick(lang, "未解析", "unresolved"),
        };
        ui.small(format!(
            "{}: {} ({})",
            pick(lang, "目标站点", "Target station"),
            preview
                .resolved_station_name
                .as_deref()
                .unwrap_or_else(|| pick(lang, "<未确定>", "<unresolved>")),
            station_source
        ));

        if let Some(enabled) = preview.station_enabled {
            ui.small(format!(
                "{}: {}  {}: {}",
                pick(lang, "启用", "Enabled"),
                enabled,
                pick(lang, "等级", "Level"),
                preview.station_level.unwrap_or(1)
            ));
        }
        if let Some(alias) = preview.station_alias.as_deref()
            && !alias.trim().is_empty()
        {
            ui.small(format!("alias: {alias}"));
        }

        if preview.resolved_station_name.is_some() && !preview.station_exists {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前预览目标站点不存在，profile 落地后会失效或被校验拒绝。",
                    "The previewed target station does not exist; this profile would be invalid or rejected.",
                ),
            );
        }

        if let Some(capabilities) = preview.capabilities.as_ref() {
            ui.small(format!(
                "{}: {}  {}: {}",
                pick(lang, "支持 service tier", "Supports service tier"),
                capability_support_label(lang, capabilities.supports_service_tier),
                pick(lang, "支持 reasoning", "Supports reasoning"),
                capability_support_label(lang, capabilities.supports_reasoning_effort)
            ));
            if !capabilities.supported_models.is_empty() {
                ui.small(format!(
                    "{}: {}",
                    pick(lang, "支持模型", "Supported models"),
                    capabilities.supported_models.join(", ")
                ));
            }
        }

        if profile.service_tier.as_deref() == Some("priority") {
            ui.small(pick(
                lang,
                "fast mode 提示：当前 profile 使用 service_tier=priority。",
                "Fast mode hint: this profile uses service_tier=priority.",
            ));
        }
        if let Some(false) = preview.model_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 model 不在该站点已知支持模型列表内。",
                    "The current model is not in the station's known supported model list.",
                ),
            );
        }
        if let Some(false) = preview.service_tier_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 service_tier 与该站点能力摘要不匹配。",
                    "The current service_tier does not match the station capability summary.",
                ),
            );
        }
        if let Some(false) = preview.reasoning_supported {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "当前 reasoning_effort 与该站点能力摘要不匹配。",
                    "The current reasoning_effort does not match the station capability summary.",
                ),
            );
        }

        if !preview.structure_available {
            ui.small(pick(
                lang,
                "当前没有可见的 station/provider 结构，因此这里只能预览到站点层。",
                "No visible station/provider structure is available, so this preview is limited to the station layer.",
            ));
        } else if preview.members.is_empty() {
            ui.small(pick(
                lang,
                "当前站点还没有 member/provider 引用。",
                "The current station does not have any member/provider refs yet.",
            ));
        } else {
            ui.small(format!(
                "{}: {}",
                pick(lang, "成员路由", "Member routes"),
                preview.members.len()
            ));
            for (index, member) in preview.members.iter().enumerate() {
                let endpoint_scope = if member.uses_all_endpoints {
                    if member.endpoint_names.is_empty() {
                        pick(lang, "<全部 endpoint>", "<all endpoints>").to_string()
                    } else {
                        format!(
                            "{} ({})",
                            pick(lang, "全部 endpoint", "all endpoints"),
                            member.endpoint_names.join(", ")
                        )
                    }
                } else if member.endpoint_names.is_empty() {
                    pick(lang, "<未指定 endpoint>", "<no endpoints>").to_string()
                } else {
                    member.endpoint_names.join(", ")
                };
                let alias = member.provider_alias.as_deref().unwrap_or("-");
                let preferred = if member.preferred {
                    pick(lang, " preferred", " preferred")
                } else {
                    ""
                };
                let enabled_suffix = match member.provider_enabled {
                    Some(false) => " [off]",
                    _ => "",
                };
                let missing_suffix = if member.provider_missing {
                    pick(lang, " [missing]", " [missing]")
                } else {
                    ""
                };
                ui.small(format!(
                    "#{} {} ({}){}{}{} -> {}",
                    index + 1,
                    member.provider_name,
                    alias,
                    preferred,
                    enabled_suffix,
                    missing_suffix,
                    endpoint_scope
                ));
            }
        }
    });
}

pub(super) fn session_route_preview_value(
    resolved: Option<&ResolvedRouteValue>,
    fallback: Option<&str>,
    lang: Language,
) -> String {
    resolved
        .map(|value| value.value.clone())
        .or_else(|| {
            fallback
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| pick(lang, "<未解析>", "<unresolved>").to_string())
}

pub(super) fn session_profile_target_value(raw: Option<&str>, lang: Language) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| pick(lang, "<自动>", "<auto>").to_string())
}

pub(super) fn session_profile_target_station_value(
    preview: &ProfileRoutePreview,
    lang: Language,
) -> String {
    match preview.resolved_station_name.as_deref() {
        Some(name) => {
            let source = match preview.station_source {
                ProfilePreviewStationSource::Profile => "profile.station",
                ProfilePreviewStationSource::ConfiguredActive => "active_station",
                ProfilePreviewStationSource::Auto => "auto",
                ProfilePreviewStationSource::Unresolved => "unresolved",
            };
            format!("{name} ({source})")
        }
        None => match preview.station_source {
            ProfilePreviewStationSource::Unresolved => {
                pick(lang, "<未解析>", "<unresolved>").to_string()
            }
            _ => pick(lang, "<自动>", "<auto>").to_string(),
        },
    }
}

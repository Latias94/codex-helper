use eframe::egui;

use super::profile_preview_state::{ProfilePreviewStationSource, ProfileRoutePreview};
use super::runtime_station::capability_support_label;
use super::*;

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

use super::profile_preview_state::ProfilePreviewMemberRoute;
use super::view_state::ConfigV2Section;
use super::*;

fn runtime_service_name(snapshot: &GuiRuntimeSnapshot, ctx: &PageCtx<'_>) -> &'static str {
    match snapshot.service_name.as_deref() {
        Some("claude") => "claude",
        Some("codex") => "codex",
        _ => match ctx.view.config.service {
            crate::config::ServiceKind::Claude => "claude",
            crate::config::ServiceKind::Codex => "codex",
        },
    }
}

fn runtime_service_kind(service_name: &str) -> crate::config::ServiceKind {
    match service_name {
        "claude" => crate::config::ServiceKind::Claude,
        _ => crate::config::ServiceKind::Codex,
    }
}

fn station_provider_catalogs(
    ctx: &PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    let service_name = runtime_service_name(snapshot, ctx);
    if let Some(attached) = ctx.proxy.attached()
        && attached.service_name.as_deref() == Some(service_name)
        && attached.supports_persisted_station_settings
    {
        return Some((
            attached.persisted_stations.clone(),
            attached.persisted_station_providers.clone(),
        ));
    }

    if matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
        return None;
    }

    local_profile_preview_catalogs_from_text(ctx.proxy_config_text, service_name)
}

fn station_provider_preview(
    ctx: &PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    cfg: &StationOption,
) -> (
    ProfileRoutePreview,
    Option<BTreeMap<String, PersistedStationProviderRef>>,
) {
    let structure = station_provider_catalogs(ctx, snapshot);
    let runtime_station_catalog = snapshot
        .stations
        .iter()
        .cloned()
        .map(|station| (station.name.clone(), station))
        .collect::<BTreeMap<_, _>>();
    let preview = build_profile_route_preview(
        &crate::config::ServiceControlProfile {
            station: Some(cfg.name.clone()),
            ..Default::default()
        },
        None,
        None,
        structure.as_ref().map(|(stations, _)| stations),
        structure.as_ref().map(|(_, providers)| providers),
        Some(&runtime_station_catalog),
    );
    let provider_catalog = structure.map(|(_, providers)| providers);
    (preview, provider_catalog)
}

fn provider_member_endpoint_scope_label(
    lang: Language,
    member: &ProfilePreviewMemberRoute,
) -> String {
    if member.uses_all_endpoints {
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
    }
}

fn provider_member_target_labels(
    member: &ProfilePreviewMemberRoute,
    provider_catalog: Option<&BTreeMap<String, PersistedStationProviderRef>>,
) -> Vec<String> {
    let Some(provider) = provider_catalog.and_then(|catalog| catalog.get(&member.provider_name))
    else {
        return Vec::new();
    };

    let endpoints = if member.uses_all_endpoints {
        provider.endpoints.iter().collect::<Vec<_>>()
    } else {
        provider
            .endpoints
            .iter()
            .filter(|endpoint| {
                member
                    .endpoint_names
                    .iter()
                    .any(|name| name == &endpoint.name)
            })
            .collect::<Vec<_>>()
    };

    endpoints
        .into_iter()
        .map(|endpoint| {
            format!(
                "{}={}",
                endpoint.name,
                summarize_upstream_target(&endpoint.base_url, 56)
            )
        })
        .collect()
}

pub(super) fn render_station_provider_routes_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
) {
    let (preview, provider_catalog) = station_provider_preview(ctx, snapshot, cfg);

    ui.label(pick(
        ctx.lang,
        "Provider 成员路由",
        "Provider member routes",
    ));
    ui.small(pick(
        ctx.lang,
        "运行时切换仍发生在 station 层；这里展示的是该站点背后的 provider / endpoint 池和 preferred/fallback 次序。",
        "Runtime switching still happens at the station layer; this section shows the provider/endpoint pool behind the station plus its preferred/fallback order.",
    ));

    if !preview.structure_available {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "当前无法读取 station/provider 结构；这通常表示远端附着未开放持久化站点设置 API，或本地当前不是 v2 station/provider 配置。",
                "The station/provider structure is unavailable right now. This usually means the remote attach does not expose the persisted station-settings API, or the local config is not using the v2 station/provider schema.",
            ),
        );
        ui.horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "前往配置页", "Open Config page"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::Config);
            }
        });
        return;
    }

    if preview.members.is_empty() {
        ui.small(pick(
            ctx.lang,
            "该站点当前还没有 provider/member 引用。",
            "This station currently has no provider/member references.",
        ));
        if ui
            .button(pick(ctx.lang, "前往 Providers", "Open Providers"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Config);
            ctx.view.config.service = runtime_service_kind(runtime_service_name(snapshot, ctx));
            ctx.view.config.v2_section = ConfigV2Section::Providers;
        }
        return;
    }

    let endpoint_total = preview
        .members
        .iter()
        .map(|member| member.endpoint_names.len())
        .sum::<usize>();
    ui.small(format!(
        "{}: {}  endpoints: {}",
        pick(ctx.lang, "provider 数", "Providers"),
        preview.members.len(),
        endpoint_total
    ));

    for (index, member) in preview.members.iter().enumerate() {
        let alias = member.provider_alias.as_deref().unwrap_or("-");
        let preferred = if member.preferred {
            pick(ctx.lang, "preferred", "preferred")
        } else {
            pick(ctx.lang, "fallback", "fallback")
        };
        let status = match (member.provider_missing, member.provider_enabled) {
            (true, _) => pick(ctx.lang, "missing", "missing"),
            (false, Some(false)) => pick(ctx.lang, "disabled", "disabled"),
            _ => pick(ctx.lang, "enabled", "enabled"),
        };
        let endpoint_scope = provider_member_endpoint_scope_label(ctx.lang, member);
        let targets = provider_member_target_labels(member, provider_catalog.as_ref());

        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "#{} {} ({alias})  [{} | {}]",
                    index + 1,
                    member.provider_name,
                    preferred,
                    status
                ));
                if ui
                    .button(pick(ctx.lang, "打开 Provider", "Open Provider"))
                    .clicked()
                {
                    ctx.view.requested_page = Some(Page::Config);
                    ctx.view.config.service =
                        runtime_service_kind(runtime_service_name(snapshot, ctx));
                    ctx.view.config.v2_section = ConfigV2Section::Providers;
                    ctx.view.config.selected_provider_name = Some(member.provider_name.clone());
                }
            });
            ui.small(format!(
                "{}: {endpoint_scope}",
                pick(ctx.lang, "endpoint 范围", "Endpoint scope")
            ));
            ui.small(format!(
                "{}: {}",
                pick(ctx.lang, "目标", "Targets"),
                if targets.is_empty() {
                    "-".to_string()
                } else {
                    targets.join(", ")
                }
            ));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_member_endpoint_scope_label_formats_all_endpoints() {
        let member = ProfilePreviewMemberRoute {
            provider_name: "right".to_string(),
            provider_alias: Some("Right".to_string()),
            provider_enabled: Some(true),
            provider_missing: false,
            endpoint_names: vec!["hk".to_string(), "us".to_string()],
            uses_all_endpoints: true,
            preferred: true,
        };

        assert_eq!(
            provider_member_endpoint_scope_label(Language::En, &member),
            "all endpoints (hk, us)"
        );
    }

    #[test]
    fn provider_member_target_labels_resolve_hosts_for_selected_endpoints() {
        let member = ProfilePreviewMemberRoute {
            provider_name: "right".to_string(),
            provider_alias: Some("Right".to_string()),
            provider_enabled: Some(true),
            provider_missing: false,
            endpoint_names: vec!["hk".to_string()],
            uses_all_endpoints: false,
            preferred: true,
        };
        let provider_catalog = BTreeMap::from([(
            "right".to_string(),
            PersistedStationProviderRef {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: true,
                endpoints: vec![
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "hk".to_string(),
                        base_url: "https://hk.example.com/v1".to_string(),
                        enabled: true,
                    },
                    crate::config::PersistedStationProviderEndpointRef {
                        name: "us".to_string(),
                        base_url: "https://us.example.com/v1".to_string(),
                        enabled: true,
                    },
                ],
            },
        )]);

        let targets = provider_member_target_labels(&member, Some(&provider_catalog));

        assert_eq!(targets, vec!["hk=hk.example.com".to_string()]);
    }
}

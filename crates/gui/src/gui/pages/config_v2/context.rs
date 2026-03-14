use super::*;

pub(in crate::gui::pages) struct ConfigV2RenderContext {
    pub(in crate::gui::pages) schema_version: u32,
    pub(in crate::gui::pages) selected_service: &'static str,
    pub(in crate::gui::pages) station_names: Vec<String>,
    pub(in crate::gui::pages) profile_names: Vec<String>,
    pub(in crate::gui::pages) default_profile: Option<String>,
    pub(in crate::gui::pages) station_display_names: Vec<String>,
    pub(in crate::gui::pages) selected_name: Option<String>,
    pub(in crate::gui::pages) station_control_plane_catalog: BTreeMap<String, StationOption>,
    pub(in crate::gui::pages) station_control_plane_enabled: bool,
    pub(in crate::gui::pages) station_control_plane_configured_active: Option<String>,
    pub(in crate::gui::pages) station_control_plane_effective_active: Option<String>,
    pub(in crate::gui::pages) station_default_profile: Option<String>,
    pub(in crate::gui::pages) attached_station_specs: Option<(
        BTreeMap<String, PersistedStationSpec>,
        BTreeMap<String, PersistedStationProviderRef>,
    )>,
    pub(in crate::gui::pages) station_structure_control_plane_enabled: bool,
    pub(in crate::gui::pages) station_structure_edit_enabled: bool,
    pub(in crate::gui::pages) attached_provider_specs:
        Option<BTreeMap<String, PersistedProviderSpec>>,
    pub(in crate::gui::pages) provider_structure_control_plane_enabled: bool,
    pub(in crate::gui::pages) provider_structure_edit_enabled: bool,
    pub(in crate::gui::pages) runtime_service: Option<String>,
    pub(in crate::gui::pages) supports_v1: bool,
    pub(in crate::gui::pages) cfg_health: Option<StationHealth>,
    pub(in crate::gui::pages) hc_status: Option<HealthCheckStatus>,
    pub(in crate::gui::pages) profile_control_plane_catalog:
        BTreeMap<String, crate::config::ServiceControlProfile>,
    pub(in crate::gui::pages) profile_control_plane_default: Option<String>,
    pub(in crate::gui::pages) profile_control_plane_station_names: Vec<String>,
    pub(in crate::gui::pages) profile_control_plane_enabled: bool,
    pub(in crate::gui::pages) provider_catalog: BTreeMap<String, crate::config::ProviderConfigV2>,
    pub(in crate::gui::pages) local_provider_spec_catalog: BTreeMap<String, PersistedProviderSpec>,
    pub(in crate::gui::pages) local_station_spec_catalog: BTreeMap<String, PersistedStationSpec>,
    pub(in crate::gui::pages) local_provider_ref_catalog:
        BTreeMap<String, PersistedStationProviderRef>,
    pub(in crate::gui::pages) provider_display_names: Vec<String>,
    pub(in crate::gui::pages) profile_catalog:
        BTreeMap<String, crate::config::ServiceControlProfile>,
    pub(in crate::gui::pages) configured_active_name: Option<String>,
    pub(in crate::gui::pages) effective_active_name: Option<String>,
    pub(in crate::gui::pages) attached_mode: bool,
}

impl ConfigV2RenderContext {
    pub(in crate::gui::pages) fn build(ctx: &mut PageCtx<'_>) -> Option<Self> {
        let Some(ConfigWorkingDocument::V2(cfg)) = ctx.view.config.working.as_ref() else {
            return None;
        };

        let runtime = crate::config::compile_v2_to_runtime(cfg).ok();
        let view = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => &cfg.claude,
            crate::config::ServiceKind::Codex => &cfg.codex,
        };

        let mut station_names = view.groups.keys().cloned().collect::<Vec<_>>();
        station_names.sort_by(|a, b| {
            let la = view.groups.get(a).map(|c| c.level).unwrap_or(1);
            let lb = view.groups.get(b).map(|c| c.level).unwrap_or(1);
            la.cmp(&lb).then_with(|| a.cmp(b))
        });
        let profile_names = view.profiles.keys().cloned().collect::<Vec<_>>();
        let active_name = view.active_group.clone();
        let active_fallback = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => runtime
                .as_ref()
                .and_then(|compiled| compiled.claude.active_station().map(|cfg| cfg.name.clone())),
            crate::config::ServiceKind::Codex => runtime
                .as_ref()
                .and_then(|compiled| compiled.codex.active_station().map(|cfg| cfg.name.clone())),
        };
        let default_profile = view.default_profile.clone();

        let selected_service = match ctx.view.config.service {
            crate::config::ServiceKind::Claude => "claude",
            crate::config::ServiceKind::Codex => "codex",
        };
        let control_plane_snapshot = ctx.proxy.snapshot().filter(|snapshot| {
            snapshot.supports_v1 && snapshot.service_name.as_deref() == Some(selected_service)
        });
        let station_control_plane_snapshot = control_plane_snapshot
            .clone()
            .filter(|snapshot| snapshot.supports_persisted_station_config);
        let station_control_plane_catalog = station_control_plane_snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .stations
                    .iter()
                    .cloned()
                    .map(|config| (config.name.clone(), config))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let station_control_plane_enabled = station_control_plane_snapshot.is_some();
        let station_control_plane_configured_active = station_control_plane_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.configured_active_station.clone());
        let station_control_plane_effective_active = station_control_plane_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.effective_active_station.clone());
        let station_default_profile = if station_control_plane_enabled {
            station_control_plane_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.configured_default_profile.clone())
                .or_else(|| {
                    station_control_plane_snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.default_profile.clone())
                })
        } else {
            default_profile.clone()
        };

        let attached_station_specs = ctx
            .proxy
            .attached()
            .filter(|att| {
                att.service_name.as_deref() == Some(selected_service)
                    && att.supports_station_spec_api
            })
            .map(|att| {
                (
                    att.persisted_stations.clone(),
                    att.persisted_station_providers.clone(),
                )
            });
        let station_structure_control_plane_enabled = attached_station_specs.is_some();
        let station_structure_edit_enabled = station_structure_control_plane_enabled
            || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);

        let attached_provider_specs = ctx
            .proxy
            .attached()
            .filter(|att| {
                att.service_name.as_deref() == Some(selected_service)
                    && att.supports_provider_spec_api
            })
            .map(|att| att.persisted_providers.clone());
        let provider_structure_control_plane_enabled = attached_provider_specs.is_some();
        let provider_structure_edit_enabled = provider_structure_control_plane_enabled
            || !matches!(ctx.proxy.kind(), ProxyModeKind::Attached);

        let station_display_names = if let Some((stations, _)) = attached_station_specs.as_ref() {
            let mut names = stations.values().cloned().collect::<Vec<_>>();
            names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
            names.into_iter().map(|station| station.name).collect()
        } else if let Some(snapshot) = station_control_plane_snapshot.as_ref() {
            let mut names = snapshot.stations.clone();
            names.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
            names.into_iter().map(|config| config.name).collect()
        } else {
            station_names.clone()
        };

        if ctx
            .view
            .config
            .selected_name
            .as_ref()
            .is_none_or(|name| !station_display_names.iter().any(|item| item == name))
        {
            ctx.view.config.selected_name = station_display_names.first().cloned();
        }
        let selected_name = ctx.view.config.selected_name.clone();
        let selected_station_name = selected_name.clone().unwrap_or_default();

        let (runtime_service, supports_v1, cfg_health, hc_status) = match ctx.proxy.kind() {
            ProxyModeKind::Running => {
                if let Some(running) = ctx.proxy.running() {
                    let state = running.state.clone();
                    let (health, checks) = ctx.rt.block_on(async {
                        tokio::join!(
                            state.get_station_health(running.service_name),
                            state.list_health_checks(running.service_name)
                        )
                    });
                    (
                        Some(running.service_name.to_string()),
                        true,
                        health.get(&selected_station_name).cloned(),
                        checks.get(&selected_station_name).cloned(),
                    )
                } else {
                    (None, false, None, None)
                }
            }
            ProxyModeKind::Attached => {
                if let Some(attached) = ctx.proxy.attached() {
                    (
                        attached.service_name.clone(),
                        attached.api_version == Some(1),
                        attached.station_health.get(&selected_station_name).cloned(),
                        attached.health_checks.get(&selected_station_name).cloned(),
                    )
                } else {
                    (None, false, None, None)
                }
            }
            _ => (None, false, None, None),
        };

        let profile_control_plane_catalog = control_plane_snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .profiles
                    .iter()
                    .map(|profile| (profile.name.clone(), service_profile_from_option(profile)))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let profile_control_plane_default = control_plane_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.configured_default_profile.clone())
            .or_else(|| {
                control_plane_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.default_profile.clone())
            });
        let profile_control_plane_station_names = control_plane_snapshot
            .as_ref()
            .map(|snapshot| {
                let mut names = snapshot
                    .stations
                    .iter()
                    .map(|config| config.name.clone())
                    .collect::<Vec<_>>();
                names.sort();
                names.dedup();
                names
            })
            .unwrap_or_else(|| station_names.clone());
        let profile_control_plane_enabled = control_plane_snapshot.is_some();

        let provider_catalog = view.providers.clone();
        let local_provider_catalog = crate::config::build_persisted_provider_catalog(view);
        let local_provider_spec_catalog = local_provider_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();
        let local_station_catalog = crate::config::build_persisted_station_catalog(view);
        let local_station_spec_catalog = local_station_catalog
            .stations
            .iter()
            .cloned()
            .map(|station| (station.name.clone(), station))
            .collect::<BTreeMap<_, _>>();
        let local_provider_ref_catalog = local_station_catalog
            .providers
            .iter()
            .cloned()
            .map(|provider| (provider.name.clone(), provider))
            .collect::<BTreeMap<_, _>>();

        let mut provider_display_names =
            if let Some(provider_specs) = attached_provider_specs.as_ref() {
                provider_specs.keys().cloned().collect::<Vec<_>>()
            } else {
                local_provider_spec_catalog
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            };
        provider_display_names.sort();

        let configured_active_name = if station_control_plane_enabled {
            station_control_plane_configured_active.clone()
        } else {
            active_name.clone()
        };
        let effective_active_name = if station_control_plane_enabled {
            station_control_plane_effective_active.clone()
        } else if active_name.is_some() {
            active_name.clone()
        } else {
            active_fallback
        };

        Some(Self {
            schema_version: cfg.version,
            selected_service,
            station_names,
            profile_names,
            default_profile,
            station_display_names,
            selected_name,
            station_control_plane_catalog,
            station_control_plane_enabled,
            station_control_plane_configured_active,
            station_control_plane_effective_active,
            station_default_profile,
            attached_station_specs,
            station_structure_control_plane_enabled,
            station_structure_edit_enabled,
            attached_provider_specs,
            provider_structure_control_plane_enabled,
            provider_structure_edit_enabled,
            runtime_service,
            supports_v1,
            cfg_health,
            hc_status,
            profile_control_plane_catalog,
            profile_control_plane_default,
            profile_control_plane_station_names,
            profile_control_plane_enabled,
            provider_catalog,
            local_provider_spec_catalog,
            local_station_spec_catalog,
            local_provider_ref_catalog,
            provider_display_names,
            profile_catalog: view.profiles.clone(),
            configured_active_name,
            effective_active_name,
            attached_mode: matches!(ctx.proxy.kind(), ProxyModeKind::Attached),
        })
    }

    pub(super) fn sync_draft(&self, draft: &mut ConfigV2EditorDraft) {
        if self.station_structure_control_plane_enabled {
            if let Some((station_specs, _)) = self.attached_station_specs.as_ref() {
                draft.sync_station_editor_from_specs(self.selected_name.as_deref(), station_specs);
            }
        } else if self.station_control_plane_enabled {
            draft.sync_station_editor_from_runtime(
                self.selected_name.as_deref(),
                &self.station_control_plane_catalog,
            );
        } else if !self.attached_mode {
            draft.sync_station_editor_from_specs(
                self.selected_name.as_deref(),
                &self.local_station_spec_catalog,
            );
        }

        draft.sync_selected_provider_name(&self.provider_display_names);
        if self.provider_structure_control_plane_enabled {
            if let Some(provider_specs) = self.attached_provider_specs.as_ref() {
                draft.sync_provider_editor_from_specs(provider_specs);
            }
        } else if !self.attached_mode {
            draft.sync_provider_editor_from_specs(&self.local_provider_spec_catalog);
        }

        if self.profile_control_plane_enabled {
            draft.sync_selected_profile_name_remote(
                &self.profile_control_plane_catalog,
                self.profile_control_plane_default.as_deref(),
            );
            draft.sync_profile_editor_from_remote(&self.profile_control_plane_catalog);
        } else {
            draft.sync_selected_profile_name_local(
                &self.profile_names,
                self.default_profile.as_deref(),
            );
        }
    }

    pub(in crate::gui::pages) fn preview_station_specs(
        &self,
    ) -> Option<&BTreeMap<String, PersistedStationSpec>> {
        if self.station_structure_control_plane_enabled {
            self.attached_station_specs.as_ref().map(|specs| &specs.0)
        } else if self.attached_mode {
            None
        } else {
            Some(&self.local_station_spec_catalog)
        }
    }

    pub(in crate::gui::pages) fn preview_provider_catalog(
        &self,
    ) -> Option<&BTreeMap<String, PersistedStationProviderRef>> {
        if self.station_structure_control_plane_enabled {
            self.attached_station_specs.as_ref().map(|specs| &specs.1)
        } else if self.attached_mode {
            None
        } else {
            Some(&self.local_provider_ref_catalog)
        }
    }

    pub(in crate::gui::pages) fn preview_runtime_station_catalog(
        &self,
    ) -> Option<&BTreeMap<String, StationOption>> {
        self.station_control_plane_enabled
            .then_some(&self.station_control_plane_catalog)
    }
}

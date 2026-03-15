use super::*;

pub(super) fn build_runtime_station_catalog(
    snapshot: &GuiRuntimeSnapshot,
) -> BTreeMap<String, StationOption> {
    snapshot
        .stations
        .iter()
        .cloned()
        .map(|config| (config.name.clone(), config))
        .collect()
}

pub(super) fn resolve_session_preview_catalogs(
    ctx: &PageCtx<'_>,
    session_preview_service_name: &str,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    ctx.proxy
        .attached()
        .and_then(|att| {
            att.supports_station_spec_api.then(|| {
                (
                    att.persisted_stations.clone(),
                    att.persisted_station_providers.clone(),
                )
            })
        })
        .or_else(|| {
            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
                None
            } else {
                local_profile_preview_catalogs_from_text(
                    ctx.proxy_config_text,
                    session_preview_service_name,
                )
            }
        })
}

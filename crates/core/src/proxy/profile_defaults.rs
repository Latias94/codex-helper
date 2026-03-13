use crate::config::ServiceConfigManager;
use crate::state::ProxyState;

pub(super) async fn effective_default_profile_name(
    state: &ProxyState,
    service_name: &str,
    mgr: &ServiceConfigManager,
) -> Option<String> {
    if let Some(name) = state
        .get_runtime_default_profile_override(service_name)
        .await
        && mgr.profile(name.as_str()).is_some()
    {
        return Some(name);
    }
    mgr.default_profile_ref().map(|(name, _)| name.to_string())
}

pub(super) fn configured_active_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.active
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn effective_active_station_name(mgr: &ServiceConfigManager) -> Option<String> {
    mgr.active_station().map(|cfg| cfg.name.clone())
}

#[cfg(test)]
mod tests {
    use super::{
        ServiceConfigManager, configured_active_station_name, effective_active_station_name,
        effective_default_profile_name,
    };
    use crate::config::{ServiceConfig, ServiceControlProfile};
    use crate::state::ProxyState;

    fn make_manager() -> ServiceConfigManager {
        let mut mgr = ServiceConfigManager {
            active: Some("  right  ".to_string()),
            default_profile: Some("fallback".to_string()),
            ..Default::default()
        };
        mgr.profiles.insert(
            "fallback".to_string(),
            ServiceControlProfile {
                station: Some("right".to_string()),
                ..Default::default()
            },
        );
        mgr.profiles.insert(
            "fast".to_string(),
            ServiceControlProfile {
                station: Some("backup".to_string()),
                model: Some("gpt-5.4".to_string()),
                ..Default::default()
            },
        );
        mgr.configs.insert(
            "right".to_string(),
            ServiceConfig {
                name: "right".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: Vec::new(),
            },
        );
        mgr.configs.insert(
            "backup".to_string(),
            ServiceConfig {
                name: "backup".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: Vec::new(),
            },
        );
        mgr
    }

    #[test]
    fn configured_active_station_name_trims_but_effective_active_uses_manager_resolution() {
        let mgr = make_manager();

        assert_eq!(
            configured_active_station_name(&mgr).as_deref(),
            Some("right")
        );
        assert_eq!(
            effective_active_station_name(&mgr).as_deref(),
            Some("backup")
        );
    }

    #[test]
    fn effective_default_profile_prefers_runtime_override_when_profile_exists() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .set_runtime_default_profile_override("codex".to_string(), "fast".to_string(), 1)
                .await;

            let name = effective_default_profile_name(state.as_ref(), "codex", &make_manager())
                .await
                .expect("default profile");

            assert_eq!(name, "fast");
        });
    }

    #[test]
    fn effective_default_profile_falls_back_when_runtime_override_missing_or_invalid() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state = ProxyState::new();
            state
                .set_runtime_default_profile_override("codex".to_string(), "missing".to_string(), 1)
                .await;

            let name = effective_default_profile_name(state.as_ref(), "codex", &make_manager())
                .await
                .expect("fallback profile");

            assert_eq!(name, "fallback");
        });
    }
}

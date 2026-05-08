use serde::{Deserialize, Serialize};

use crate::state::RuntimeConfigState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SharedControlPlaneCapabilities {
    #[serde(default)]
    pub session_observability: bool,
    #[serde(default)]
    pub request_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostLocalControlPlaneCapabilities {
    #[serde(default)]
    pub session_history: bool,
    #[serde(default)]
    pub transcript_read: bool,
    #[serde(default)]
    pub cwd_enrichment: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RemoteAdminAccessCapabilities {
    #[serde(default)]
    pub loopback_without_token: bool,
    #[serde(default)]
    pub remote_requires_token: bool,
    #[serde(default)]
    pub remote_enabled: bool,
    #[serde(default)]
    pub token_header: String,
    #[serde(default)]
    pub token_env_var: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ControlPlaneSurfaceCapabilities {
    #[serde(default)]
    pub snapshot: bool,
    #[serde(default)]
    pub operator_summary: bool,
    #[serde(default)]
    pub status_active: bool,
    #[serde(default)]
    pub status_recent: bool,
    #[serde(default)]
    pub status_session_stats: bool,
    #[serde(default)]
    pub status_health_checks: bool,
    #[serde(default)]
    pub status_station_health: bool,
    #[serde(default)]
    pub runtime_status: bool,
    #[serde(default)]
    pub runtime_reload: bool,
    #[serde(default)]
    pub request_ledger_recent: bool,
    #[serde(default)]
    pub request_ledger_summary: bool,
    #[serde(default)]
    pub control_trace: bool,
    #[serde(default)]
    pub retry_config: bool,
    #[serde(default)]
    pub pricing_catalog: bool,
    #[serde(default)]
    pub stations: bool,
    #[serde(default)]
    pub station_runtime: bool,
    #[serde(
        default,
        rename = "station_persisted_settings",
        alias = "station_persisted_config"
    )]
    pub station_persisted_settings: bool,
    #[serde(default)]
    pub station_specs: bool,
    #[serde(default)]
    pub station_probe: bool,
    #[serde(default)]
    pub providers: bool,
    #[serde(default)]
    pub provider_runtime: bool,
    #[serde(default)]
    pub provider_balance_refresh: bool,
    #[serde(default)]
    pub provider_specs: bool,
    #[serde(default)]
    pub profiles: bool,
    #[serde(default)]
    pub default_profile_override: bool,
    #[serde(default)]
    pub persisted_default_profile: bool,
    #[serde(default)]
    pub profile_mutation: bool,
    #[serde(default)]
    pub session_overrides: bool,
    #[serde(default)]
    pub session_profile_override: bool,
    #[serde(default)]
    pub session_model_override: bool,
    #[serde(default)]
    pub session_reasoning_effort_override: bool,
    #[serde(default)]
    pub session_station_override: bool,
    #[serde(default)]
    pub session_service_tier_override: bool,
    #[serde(default)]
    pub session_override_reset: bool,
    #[serde(default)]
    pub global_station_override: bool,
    #[serde(default)]
    pub healthcheck_start: bool,
    #[serde(default)]
    pub healthcheck_cancel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ApiV1Capabilities {
    #[serde(default)]
    pub api_version: u32,
    #[serde(default)]
    pub service_name: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub surface_capabilities: ControlPlaneSurfaceCapabilities,
    #[serde(default)]
    pub shared_capabilities: SharedControlPlaneCapabilities,
    #[serde(default)]
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    #[serde(default)]
    pub remote_admin_access: RemoteAdminAccessCapabilities,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelCatalogKind {
    #[default]
    ImplicitAny,
    Declared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StationCapabilitySummary {
    #[serde(default)]
    pub model_catalog_kind: ModelCatalogKind,
    #[serde(default)]
    pub supported_models: Vec<String>,
    #[serde(default)]
    pub supports_service_tier: CapabilitySupport,
    #[serde(default)]
    pub supports_reasoning_effort: CapabilitySupport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StationOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub level: u8,
    #[serde(default)]
    pub configured_enabled: bool,
    #[serde(default)]
    pub configured_level: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_enabled_override: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_level_override: Option<u8>,
    #[serde(default)]
    pub runtime_state: RuntimeConfigState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_state_override: Option<RuntimeConfigState>,
    #[serde(default)]
    pub capabilities: StationCapabilitySummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlProfileOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderEndpointOption {
    pub provider_name: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub configured_enabled: bool,
    #[serde(default)]
    pub effective_enabled: bool,
    #[serde(default)]
    pub routable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_enabled_override: Option<bool>,
    #[serde(default)]
    pub runtime_state: RuntimeConfigState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_state_override: Option<RuntimeConfigState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default)]
    pub configured_enabled: bool,
    #[serde(default)]
    pub effective_enabled: bool,
    #[serde(default)]
    pub routable_endpoints: usize,
    #[serde(default)]
    pub endpoints: Vec<ProviderEndpointOption>,
}

#[cfg(test)]
mod tests {
    use super::ControlPlaneSurfaceCapabilities;

    #[test]
    fn control_plane_surface_capabilities_serializes_station_persisted_settings_canonically() {
        let caps = ControlPlaneSurfaceCapabilities {
            station_persisted_settings: true,
            ..Default::default()
        };

        let value = serde_json::to_value(caps).expect("serialize capabilities");
        assert_eq!(value["station_persisted_settings"].as_bool(), Some(true));
        assert!(value.get("station_persisted_config").is_none());
    }

    #[test]
    fn control_plane_surface_capabilities_reads_legacy_station_persisted_config_alias() {
        let caps: ControlPlaneSurfaceCapabilities = serde_json::from_value(serde_json::json!({
            "station_persisted_config": true
        }))
        .expect("deserialize legacy capability alias");

        assert!(caps.station_persisted_settings);
    }
}

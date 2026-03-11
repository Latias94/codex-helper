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
pub struct ApiV1Capabilities {
    #[serde(default)]
    pub api_version: u32,
    #[serde(default)]
    pub service_name: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub shared_capabilities: SharedControlPlaneCapabilities,
    #[serde(default)]
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
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
pub struct ConfigCapabilitySummary {
    #[serde(default)]
    pub model_catalog_kind: ModelCatalogKind,
    #[serde(default)]
    pub supported_models: Vec<String>,
    #[serde(default)]
    pub supports_service_tier: CapabilitySupport,
    #[serde(default)]
    pub supports_reasoning_effort: CapabilitySupport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOption {
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
    pub capabilities: ConfigCapabilitySummary,
}

pub type StationOption = ConfigOption;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlProfileOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

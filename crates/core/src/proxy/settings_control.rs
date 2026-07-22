use std::collections::BTreeMap;

use anyhow::Context;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{
    HelperConfig, ServiceControlProfile, ServiceRouteConfig, mutate_helper_config,
    resolve_service_profile_from_catalog,
};
pub use crate::dashboard_core::EffectiveDefaultProfileSource;

use super::runtime_config::RuntimeSnapshot;
use super::{ProfilesResponse, ProxyControlError, ProxyService};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDefaultProfileControlSnapshot {
    pub control_revision: u64,
    #[serde(default)]
    pub runtime_override: Option<String>,
    pub profile_catalog_key: String,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDefaultProfileControls {
    codex: RuntimeDefaultProfileControlSnapshot,
    claude: RuntimeDefaultProfileControlSnapshot,
}

impl RuntimeDefaultProfileControls {
    pub(super) fn from_config(config: &HelperConfig) -> anyhow::Result<Self> {
        Ok(Self {
            codex: initial_default_profile_control(&config.codex)?,
            claude: initial_default_profile_control(&config.claude)?,
        })
    }

    pub(super) fn get(
        &self,
        service_name: &str,
    ) -> anyhow::Result<&RuntimeDefaultProfileControlSnapshot> {
        match service_name {
            "codex" => Ok(&self.codex),
            "claude" => Ok(&self.claude),
            _ => anyhow::bail!("unsupported service '{service_name}'"),
        }
    }

    pub(super) fn entries(&self) -> [(&'static str, &RuntimeDefaultProfileControlSnapshot); 2] {
        [("codex", &self.codex), ("claude", &self.claude)]
    }

    fn get_mut(
        &mut self,
        service_name: &str,
    ) -> anyhow::Result<&mut RuntimeDefaultProfileControlSnapshot> {
        match service_name {
            "codex" => Ok(&mut self.codex),
            "claude" => Ok(&mut self.claude),
            _ => anyhow::bail!("unsupported service '{service_name}'"),
        }
    }

    pub(super) fn reconciled_for_config(
        &self,
        config: &HelperConfig,
        now_ms: u64,
    ) -> anyhow::Result<Self> {
        let mut next = self.clone();
        for (service_name, view) in [("codex", &config.codex), ("claude", &config.claude)] {
            let control = next.get_mut(service_name)?;
            control.profile_catalog_key = profile_catalog_key(view)?;
            if control
                .runtime_override
                .as_deref()
                .is_some_and(|profile_name| !view.profiles.contains_key(profile_name))
            {
                control.runtime_override = None;
                control.control_revision = control.control_revision.saturating_add(1);
                control.updated_at_ms = Some(now_ms);
            }
        }
        Ok(next)
    }

    pub(super) fn with_runtime_override(
        &self,
        service_name: &str,
        runtime_override: Option<String>,
        now_ms: u64,
    ) -> anyhow::Result<Self> {
        let mut next = self.clone();
        let control = next.get_mut(service_name)?;
        control.control_revision = control.control_revision.saturating_add(1);
        control.runtime_override = runtime_override;
        control.updated_at_ms = Some(now_ms);
        Ok(next)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorDefaultProfileScope {
    Configured,
    Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OperatorDefaultProfileMutationRequest {
    pub scope: OperatorDefaultProfileScope,
    #[serde(default)]
    pub profile_name: Option<String>,
    pub expected_profile_catalog_key: String,
    pub expected_control_revision: u64,
    #[serde(default)]
    pub expected_configured_profile: Option<String>,
    #[serde(default)]
    pub expected_runtime_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorDefaultProfileMutationStatus {
    Unchanged,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorDefaultProfileMutationResponse {
    pub service_name: String,
    pub status: OperatorDefaultProfileMutationStatus,
    pub runtime_revision: u64,
    pub control: RuntimeDefaultProfileControlSnapshot,
    pub profiles: ProfilesResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct OperatorRuntimeReloadRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorRuntimeReloadResponse {
    pub service_name: String,
    pub changed: bool,
    pub runtime_revision: u64,
    pub control: RuntimeDefaultProfileControlSnapshot,
    pub profiles: ProfilesResponse,
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeDefaultProfileMutationExpectation {
    pub(super) profile_catalog_key: String,
    pub(super) control_revision: u64,
    pub(super) configured_profile: Option<String>,
    pub(super) runtime_profile: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum RuntimeDefaultProfileMutationError {
    #[error("default profile state changed; refresh Settings and retry")]
    Conflict,
    #[error("{0}")]
    InvalidTarget(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ProxyService {
    pub async fn mutate_operator_default_profile(
        &self,
        request: OperatorDefaultProfileMutationRequest,
    ) -> Result<OperatorDefaultProfileMutationResponse, ProxyControlError> {
        let expected = RuntimeDefaultProfileMutationExpectation::from(&request);
        let target_profile = normalize_profile_name(request.profile_name.clone())?;
        let status = match request.scope {
            OperatorDefaultProfileScope::Runtime => self
                .config
                .mutate_runtime_default_profile(self.service_name, expected, target_profile)
                .await
                .map_err(runtime_profile_error)?,
            OperatorDefaultProfileScope::Configured => {
                ensure_expected_runtime_profile_state(self, &request).await?;
                let target = target_profile;
                let service_name = self.service_name;
                let (_, changed) = mutate_helper_config(move |config| {
                    let view = mutable_service_route_config(config, service_name)?;
                    ensure_expected_persisted_profile_state(view, &expected)?;
                    validate_target_profile_for_config(view, target.as_deref())?;
                    let changed = view.default_profile != target;
                    view.default_profile = target;
                    Ok(changed)
                })
                .await
                .map_err(persisted_profile_error)?;
                if changed {
                    self.reload_runtime_config().await.map_err(|error| {
                        ProxyControlError::new(
                            error.status(),
                            format!(
                                "configured default profile was persisted, but runtime reload failed; runtime remains on the last known good snapshot: {}",
                                error.message()
                            ),
                        )
                    })?;
                    OperatorDefaultProfileMutationStatus::Applied
                } else {
                    OperatorDefaultProfileMutationStatus::Unchanged
                }
            }
        };

        let runtime = self.config.capture().await;
        Ok(OperatorDefaultProfileMutationResponse {
            service_name: self.service_name.to_string(),
            status,
            runtime_revision: runtime.revision(),
            control: runtime
                .default_profile_control(self.service_name)
                .map_err(internal_profile_error)?,
            profiles: super::api_responses::make_profiles_response_from_snapshot(
                self,
                runtime.as_ref(),
            )
            .map_err(internal_profile_error)?,
        })
    }

    pub async fn operator_runtime_reload(
        &self,
        _request: OperatorRuntimeReloadRequest,
    ) -> Result<OperatorRuntimeReloadResponse, ProxyControlError> {
        let changed = self.reload_runtime_config().await?;
        let runtime = self.config.capture().await;
        Ok(OperatorRuntimeReloadResponse {
            service_name: self.service_name.to_string(),
            changed,
            runtime_revision: runtime.revision(),
            control: runtime
                .default_profile_control(self.service_name)
                .map_err(internal_profile_error)?,
            profiles: super::api_responses::make_profiles_response_from_snapshot(
                self,
                runtime.as_ref(),
            )
            .map_err(internal_profile_error)?,
        })
    }
}

impl From<&OperatorDefaultProfileMutationRequest> for RuntimeDefaultProfileMutationExpectation {
    fn from(request: &OperatorDefaultProfileMutationRequest) -> Self {
        Self {
            profile_catalog_key: request.expected_profile_catalog_key.clone(),
            control_revision: request.expected_control_revision,
            configured_profile: request.expected_configured_profile.clone(),
            runtime_profile: request.expected_runtime_profile.clone(),
        }
    }
}

pub(super) fn profile_catalog_key(view: &ServiceRouteConfig) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct ProfileCatalogDigestInput<'a> {
        default_profile: &'a Option<String>,
        profiles: &'a BTreeMap<String, ServiceControlProfile>,
    }

    let encoded = serde_json::to_vec(&ProfileCatalogDigestInput {
        default_profile: &view.default_profile,
        profiles: &view.profiles,
    })
    .context("serialize profile catalog for control key")?;
    let mut hasher = Sha256::new();
    hasher.update(b"codex-helper:default-profile-catalog:v1\0");
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn initial_default_profile_control(
    view: &ServiceRouteConfig,
) -> anyhow::Result<RuntimeDefaultProfileControlSnapshot> {
    Ok(RuntimeDefaultProfileControlSnapshot {
        control_revision: 0,
        runtime_override: None,
        profile_catalog_key: profile_catalog_key(view)?,
        updated_at_ms: None,
    })
}

pub(super) fn effective_default_profile_for_snapshot(
    snapshot: &RuntimeSnapshot,
    service_name: &str,
) -> anyhow::Result<(Option<String>, EffectiveDefaultProfileSource)> {
    let config = snapshot.config();
    let view = super::control_plane_service::service_route_config(config.as_ref(), service_name);
    let control = snapshot.default_profile_control(service_name)?;
    if let Some(profile_name) = control.runtime_override
        && view.profiles.contains_key(profile_name.as_str())
    {
        return Ok((
            Some(profile_name),
            EffectiveDefaultProfileSource::RuntimeOverride,
        ));
    }
    if let Some(profile_name) = view
        .default_profile
        .as_deref()
        .filter(|profile_name| view.profiles.contains_key(*profile_name))
    {
        return Ok((
            Some(profile_name.to_string()),
            EffectiveDefaultProfileSource::Configured,
        ));
    }
    Ok((None, EffectiveDefaultProfileSource::None))
}

async fn ensure_expected_runtime_profile_state(
    proxy: &ProxyService,
    request: &OperatorDefaultProfileMutationRequest,
) -> Result<(), ProxyControlError> {
    let runtime = proxy.config.capture().await;
    let config = runtime.config();
    let view =
        super::control_plane_service::service_route_config(config.as_ref(), proxy.service_name);
    let control = runtime
        .default_profile_control(proxy.service_name)
        .map_err(internal_profile_error)?;
    if request.expected_profile_catalog_key != control.profile_catalog_key
        || request.expected_control_revision != control.control_revision
        || request.expected_configured_profile != view.default_profile
        || request.expected_runtime_profile != control.runtime_override
    {
        return Err(conflict());
    }
    Ok(())
}

fn ensure_expected_persisted_profile_state(
    view: &ServiceRouteConfig,
    expected: &RuntimeDefaultProfileMutationExpectation,
) -> anyhow::Result<()> {
    if expected.profile_catalog_key != profile_catalog_key(view)?
        || expected.configured_profile != view.default_profile
    {
        return Err(StalePersistedDefaultProfile.into());
    }
    Ok(())
}

fn normalize_profile_name(value: Option<String>) -> Result<Option<String>, ProxyControlError> {
    let value = value.map(|value| value.trim().to_string());
    if value.as_deref().is_some_and(str::is_empty) {
        return Ok(None);
    }
    if value
        .as_ref()
        .is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control))
    {
        return Err(ProxyControlError::new(
            StatusCode::BAD_REQUEST,
            "profile name is invalid or exceeds 256 bytes",
        ));
    }
    Ok(value)
}

pub(super) fn validate_target_profile(
    view: &ServiceRouteConfig,
    profile_name: Option<&str>,
) -> anyhow::Result<()> {
    let Some(profile_name) = profile_name else {
        return Ok(());
    };
    resolve_service_profile_from_catalog(&view.profiles, profile_name).map(|_| ())
}

fn validate_target_profile_for_config(
    view: &ServiceRouteConfig,
    profile_name: Option<&str>,
) -> anyhow::Result<()> {
    validate_target_profile(view, profile_name)
}

fn mutable_service_route_config<'a>(
    config: &'a mut HelperConfig,
    service_name: &str,
) -> anyhow::Result<&'a mut ServiceRouteConfig> {
    match service_name {
        "codex" => Ok(&mut config.codex),
        "claude" => Ok(&mut config.claude),
        _ => anyhow::bail!("unsupported service '{service_name}'"),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("configured default profile changed concurrently")]
struct StalePersistedDefaultProfile;

fn persisted_profile_error(error: anyhow::Error) -> ProxyControlError {
    if error
        .downcast_ref::<StalePersistedDefaultProfile>()
        .is_some()
    {
        conflict()
    } else {
        ProxyControlError::new(StatusCode::CONFLICT, error.to_string())
    }
}

fn runtime_profile_error(error: RuntimeDefaultProfileMutationError) -> ProxyControlError {
    match error {
        RuntimeDefaultProfileMutationError::Conflict => conflict(),
        RuntimeDefaultProfileMutationError::InvalidTarget(message) => {
            ProxyControlError::new(StatusCode::NOT_FOUND, message)
        }
        RuntimeDefaultProfileMutationError::Internal(error) => internal_profile_error(error),
    }
}

fn internal_profile_error(error: anyhow::Error) -> ProxyControlError {
    ProxyControlError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

fn conflict() -> ProxyControlError {
    ProxyControlError::new(
        StatusCode::CONFLICT,
        "default profile state changed; refresh Settings and retry",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use reqwest::Client;

    use crate::config::{HelperConfig, ServiceControlProfile, ServiceRouteConfig};
    use crate::state::SessionContinuityMode;

    use super::*;

    fn proxy_with_profiles() -> ProxyService {
        ProxyService::new(
            Client::new(),
            Arc::new(HelperConfig {
                codex: ServiceRouteConfig {
                    default_profile: Some("daily".to_string()),
                    profiles: BTreeMap::from([
                        (
                            "daily".to_string(),
                            ServiceControlProfile {
                                model: Some("gpt-daily".to_string()),
                                ..ServiceControlProfile::default()
                            },
                        ),
                        (
                            "fast".to_string(),
                            ServiceControlProfile {
                                model: Some("gpt-fast".to_string()),
                                ..ServiceControlProfile::default()
                            },
                        ),
                    ]),
                    ..ServiceRouteConfig::default()
                },
                ..HelperConfig::default()
            }),
            "codex",
        )
    }

    async fn runtime_request(
        proxy: &ProxyService,
        profile_name: Option<&str>,
    ) -> OperatorDefaultProfileMutationRequest {
        let runtime = proxy.config.capture().await;
        let config = runtime.config();
        let view = super::super::control_plane_service::service_route_config(
            config.as_ref(),
            proxy.service_name,
        );
        let control = runtime
            .default_profile_control(proxy.service_name)
            .expect("default profile control");
        OperatorDefaultProfileMutationRequest {
            scope: OperatorDefaultProfileScope::Runtime,
            profile_name: profile_name.map(str::to_string),
            expected_profile_catalog_key: control.profile_catalog_key,
            expected_control_revision: control.control_revision,
            expected_configured_profile: view.default_profile.clone(),
            expected_runtime_profile: control.runtime_override,
        }
    }

    #[tokio::test]
    async fn runtime_default_profile_applies_only_to_new_default_bindings() {
        let proxy = proxy_with_profiles();
        let runtime = proxy.config.capture().await;
        let existing = proxy
            .ensure_default_session_binding(runtime.as_ref(), "existing", 1)
            .await
            .expect("configured default binding");
        assert_eq!(existing.profile_name.as_deref(), Some("daily"));

        let response = proxy
            .mutate_operator_default_profile(runtime_request(&proxy, Some("fast")).await)
            .await
            .expect("set runtime default");
        assert_eq!(
            response.status,
            OperatorDefaultProfileMutationStatus::Applied
        );

        let runtime = proxy.config.capture().await;
        let retained = proxy
            .ensure_default_session_binding(runtime.as_ref(), "existing", 2)
            .await
            .expect("existing binding");
        let created = proxy
            .ensure_default_session_binding(runtime.as_ref(), "new", 2)
            .await
            .expect("runtime default binding");
        assert_eq!(retained.profile_name.as_deref(), Some("daily"));
        assert_eq!(created.profile_name.as_deref(), Some("fast"));
        assert_eq!(
            created.continuity_mode,
            SessionContinuityMode::DefaultProfile
        );
    }

    #[tokio::test]
    async fn stale_default_profile_mutation_is_rejected_without_state_change() {
        let proxy = proxy_with_profiles();
        let mut request = runtime_request(&proxy, Some("fast")).await;
        request.expected_control_revision = 99;

        let error = proxy
            .mutate_operator_default_profile(request)
            .await
            .expect_err("stale mutation");
        assert_eq!(error.status(), StatusCode::CONFLICT);
        assert_eq!(
            proxy
                .config
                .capture()
                .await
                .default_profile_control("codex")
                .expect("control")
                .runtime_override,
            None
        );
    }
}

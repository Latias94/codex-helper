use super::editors::{
    config_provider_endpoint_editor_from_spec, config_station_member_editor_from_member,
};
use super::*;

pub(super) struct ConfigV2EditorDraft {
    pub(super) station_editor_name: Option<String>,
    pub(super) station_editor_alias: String,
    pub(super) station_editor_enabled: bool,
    pub(super) station_editor_level: u8,
    pub(super) station_editor_members: Vec<ConfigStationMemberEditorState>,
    pub(super) new_station_name: String,
    pub(super) selected_provider_name: Option<String>,
    pub(super) provider_editor_name: Option<String>,
    pub(super) provider_editor_alias: String,
    pub(super) provider_editor_enabled: bool,
    pub(super) provider_editor_auth_token_env: String,
    pub(super) provider_editor_api_key_env: String,
    pub(super) provider_editor_endpoints: Vec<ConfigProviderEndpointEditorState>,
    pub(super) new_provider_name: String,
    pub(super) selected_profile_name: Option<String>,
    pub(super) new_profile_name: String,
    pub(super) profile_editor_name: Option<String>,
    pub(super) profile_editor_extends: Option<String>,
    pub(super) profile_editor_station: Option<String>,
    pub(super) profile_editor_model: String,
    pub(super) profile_editor_reasoning_effort: String,
    pub(super) profile_editor_service_tier: String,
    pub(super) profile_info: Option<String>,
    pub(super) profile_error: Option<String>,
}

impl ConfigV2EditorDraft {
    pub(super) fn from_view(view: &ConfigViewState) -> Self {
        Self {
            station_editor_name: view.station_editor.station_name.clone(),
            station_editor_alias: view.station_editor.alias.clone(),
            station_editor_enabled: view.station_editor.enabled,
            station_editor_level: view.station_editor.level.max(1),
            station_editor_members: view.station_editor.members.clone(),
            new_station_name: view.station_editor.new_station_name.clone(),
            selected_provider_name: view.selected_provider_name.clone(),
            provider_editor_name: view.provider_editor.provider_name.clone(),
            provider_editor_alias: view.provider_editor.alias.clone(),
            provider_editor_enabled: view.provider_editor.enabled,
            provider_editor_auth_token_env: view.provider_editor.auth_token_env.clone(),
            provider_editor_api_key_env: view.provider_editor.api_key_env.clone(),
            provider_editor_endpoints: view.provider_editor.endpoints.clone(),
            new_provider_name: view.provider_editor.new_provider_name.clone(),
            selected_profile_name: view.selected_profile_name.clone(),
            new_profile_name: view.new_profile_name.clone(),
            profile_editor_name: view.profile_editor.profile_name.clone(),
            profile_editor_extends: view.profile_editor.extends.clone(),
            profile_editor_station: view.profile_editor.station.clone(),
            profile_editor_model: view.profile_editor.model.clone(),
            profile_editor_reasoning_effort: view.profile_editor.reasoning_effort.clone(),
            profile_editor_service_tier: view.profile_editor.service_tier.clone(),
            profile_info: None,
            profile_error: None,
        }
    }

    pub(super) fn sync_station_editor_from_specs(
        &mut self,
        selected_name: Option<&str>,
        station_specs: &BTreeMap<String, PersistedStationSpec>,
    ) {
        if self.station_editor_name.as_deref() == selected_name {
            return;
        }

        let selected_station = selected_name.and_then(|name| station_specs.get(name));
        self.station_editor_name = selected_name.map(ToOwned::to_owned);
        self.station_editor_alias = selected_station
            .and_then(|station| station.alias.clone())
            .unwrap_or_default();
        self.station_editor_enabled = selected_station
            .map(|station| station.enabled)
            .unwrap_or(true);
        self.station_editor_level = selected_station
            .map(|station| station.level)
            .unwrap_or(1)
            .clamp(1, 10);
        self.station_editor_members = selected_station
            .map(|station| {
                station
                    .members
                    .iter()
                    .map(config_station_member_editor_from_member)
                    .collect()
            })
            .unwrap_or_default();
    }

    pub(super) fn sync_station_editor_from_runtime(
        &mut self,
        selected_name: Option<&str>,
        station_catalog: &BTreeMap<String, StationOption>,
    ) {
        if self.station_editor_name.as_deref() == selected_name {
            return;
        }

        let selected_station = selected_name.and_then(|name| station_catalog.get(name));
        self.station_editor_name = selected_name.map(ToOwned::to_owned);
        self.station_editor_alias.clear();
        self.station_editor_enabled = selected_station
            .map(|station| station.enabled)
            .unwrap_or(false);
        self.station_editor_level = selected_station
            .map(|station| station.level)
            .unwrap_or(1)
            .clamp(1, 10);
        self.station_editor_members.clear();
    }

    pub(super) fn sync_selected_provider_name(&mut self, provider_display_names: &[String]) {
        if self
            .selected_provider_name
            .as_ref()
            .is_none_or(|name| !provider_display_names.iter().any(|item| item == name))
        {
            self.selected_provider_name = provider_display_names.first().cloned();
        }
    }

    pub(super) fn sync_provider_editor_from_specs(
        &mut self,
        provider_specs: &BTreeMap<String, PersistedProviderSpec>,
    ) {
        if self.provider_editor_name.as_deref() == self.selected_provider_name.as_deref() {
            return;
        }

        let selected_provider = self
            .selected_provider_name
            .as_deref()
            .and_then(|name| provider_specs.get(name));
        self.provider_editor_name = self.selected_provider_name.clone();
        self.provider_editor_alias = selected_provider
            .and_then(|provider| provider.alias.clone())
            .unwrap_or_default();
        self.provider_editor_enabled = selected_provider
            .map(|provider| provider.enabled)
            .unwrap_or(true);
        self.provider_editor_auth_token_env = selected_provider
            .and_then(|provider| provider.auth_token_env.clone())
            .unwrap_or_default();
        self.provider_editor_api_key_env = selected_provider
            .and_then(|provider| provider.api_key_env.clone())
            .unwrap_or_default();
        self.provider_editor_endpoints = selected_provider
            .map(|provider| {
                provider
                    .endpoints
                    .iter()
                    .map(config_provider_endpoint_editor_from_spec)
                    .collect()
            })
            .unwrap_or_default();
    }

    pub(super) fn sync_selected_profile_name_remote(
        &mut self,
        profile_catalog: &BTreeMap<String, crate::config::ServiceControlProfile>,
        default_profile: Option<&str>,
    ) {
        if self
            .selected_profile_name
            .as_ref()
            .is_none_or(|name| !profile_catalog.contains_key(name))
        {
            self.selected_profile_name = default_profile
                .map(ToOwned::to_owned)
                .or_else(|| profile_catalog.keys().next().cloned());
        }
    }

    pub(super) fn sync_selected_profile_name_local(
        &mut self,
        profile_names: &[String],
        default_profile: Option<&str>,
    ) {
        if self
            .selected_profile_name
            .as_ref()
            .is_none_or(|name| !profile_names.iter().any(|item| item == name))
        {
            self.selected_profile_name = default_profile
                .map(ToOwned::to_owned)
                .or_else(|| profile_names.first().cloned());
        }
    }

    pub(super) fn sync_profile_editor_from_remote(
        &mut self,
        profile_catalog: &BTreeMap<String, crate::config::ServiceControlProfile>,
    ) {
        if self.profile_editor_name.as_deref() == self.selected_profile_name.as_deref() {
            return;
        }

        let selected_profile = self
            .selected_profile_name
            .as_deref()
            .and_then(|name| profile_catalog.get(name));
        self.profile_editor_name = self.selected_profile_name.clone();
        self.profile_editor_extends = selected_profile.and_then(|profile| profile.extends.clone());
        self.profile_editor_station = selected_profile.and_then(|profile| profile.station.clone());
        self.profile_editor_model = selected_profile
            .and_then(|profile| profile.model.clone())
            .unwrap_or_default();
        self.profile_editor_reasoning_effort = selected_profile
            .and_then(|profile| profile.reasoning_effort.clone())
            .unwrap_or_default();
        self.profile_editor_service_tier = selected_profile
            .and_then(|profile| profile.service_tier.clone())
            .unwrap_or_default();
    }

    pub(super) fn persist_into_view(
        self,
        view: &mut ConfigViewState,
    ) -> (Option<String>, Option<String>) {
        view.selected_provider_name = self.selected_provider_name;
        view.selected_profile_name = self.selected_profile_name;
        view.new_profile_name = self.new_profile_name;
        view.station_editor.station_name = self.station_editor_name;
        view.station_editor.alias = self.station_editor_alias;
        view.station_editor.enabled = self.station_editor_enabled;
        view.station_editor.level = self.station_editor_level.clamp(1, 10);
        view.station_editor.members = self.station_editor_members;
        view.station_editor.new_station_name = self.new_station_name;
        view.provider_editor.provider_name = self.provider_editor_name;
        view.provider_editor.alias = self.provider_editor_alias;
        view.provider_editor.enabled = self.provider_editor_enabled;
        view.provider_editor.auth_token_env = self.provider_editor_auth_token_env;
        view.provider_editor.api_key_env = self.provider_editor_api_key_env;
        view.provider_editor.endpoints = self.provider_editor_endpoints;
        view.provider_editor.new_provider_name = self.new_provider_name;
        view.profile_editor.profile_name = self.profile_editor_name;
        view.profile_editor.extends = self.profile_editor_extends;
        view.profile_editor.station = self.profile_editor_station;
        view.profile_editor.model = self.profile_editor_model;
        view.profile_editor.reasoning_effort = self.profile_editor_reasoning_effort;
        view.profile_editor.service_tier = self.profile_editor_service_tier;
        (self.profile_info, self.profile_error)
    }
}

use std::collections::HashMap;
use std::time::Instant;

use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};

use super::model::{
    ProviderOption, Snapshot, provider_options_from_operator_data, snapshot_from_operator_data,
};
use super::state::UiState;

pub(super) fn apply_operator_read_model(
    ui: &mut UiState,
    snapshot: &mut Snapshot,
    providers: &mut Vec<ProviderOption>,
    model: OperatorReadModel,
    local_session_ids: &HashMap<String, String>,
) {
    ui.last_runtime_config_refresh_at = Some(Instant::now());
    ui.runtime_status_error = match model.status {
        OperatorReadStatus::Ready => None,
        OperatorReadStatus::Stale => {
            Some("operator read-model refresh failed; showing stale data".to_string())
        }
        OperatorReadStatus::Disconnected => Some("operator read-model is disconnected".to_string()),
        OperatorReadStatus::AuthRequired => {
            Some("operator read-model authentication is required".to_string())
        }
    };

    if let Some(data) = model.data.as_ref() {
        *snapshot = snapshot_from_operator_data(data, local_session_ids);
        *providers = provider_options_from_operator_data(data);
        let runtime = &data.summary.runtime;
        ui.operator_action_capabilities = runtime.operator_actions;
        ui.last_runtime_config_loaded_at_ms = runtime.runtime_loaded_at_ms;
        ui.last_runtime_config_source_mtime_ms = runtime.runtime_source_mtime_ms;
        ui.last_retry_summary = Some(data.summary.retry.clone());
        ui.profile_options = data.summary.profiles.clone();
        ui.configured_default_profile = runtime.configured_default_profile.clone();
        ui.effective_default_profile = runtime.default_profile.clone();
    } else {
        *snapshot = Snapshot::default();
        providers.clear();
        ui.last_runtime_config_loaded_at_ms = None;
        ui.last_runtime_config_source_mtime_ms = None;
        ui.last_retry_summary = None;
        ui.operator_action_capabilities = Default::default();
        ui.profile_options.clear();
        ui.configured_default_profile = None;
        ui.effective_default_profile = None;
        ui.fleet_snapshot = None;
    }

    ui.operator_read_model = Some(model);
    ui.clamp_selection(snapshot, providers.len());
}

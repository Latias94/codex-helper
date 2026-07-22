use anyhow::Result;

use super::runtime_config::RuntimeSnapshot;
use super::settings_control::{
    EffectiveDefaultProfileSource, effective_default_profile_for_snapshot,
};

pub(super) fn effective_default_profile(
    snapshot: &RuntimeSnapshot,
    service_name: &str,
) -> Result<(Option<String>, EffectiveDefaultProfileSource)> {
    effective_default_profile_for_snapshot(snapshot, service_name)
}

pub(super) fn effective_default_profile_name(
    snapshot: &RuntimeSnapshot,
    service_name: &str,
) -> Result<Option<String>> {
    effective_default_profile(snapshot, service_name).map(|(profile_name, _)| profile_name)
}

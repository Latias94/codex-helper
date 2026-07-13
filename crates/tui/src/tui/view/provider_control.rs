use crate::dashboard_core::{OperatorPolicyActionSummary, OperatorProviderEndpointSummary};
use crate::tui::model::shorten_middle;

fn append_policy_action_control_parts(
    action: &OperatorPolicyActionSummary,
    parts: &mut Vec<String>,
) {
    parts.push(action.code.clone());
    if action.active_cooldown {
        let cooldown = action
            .cooldown_remaining_secs
            .map(|secs| format!("{secs}s"))
            .unwrap_or_else(|| "?".to_string());
        parts.push(format!("cooldown={cooldown}"));
    }
}

pub(in crate::tui::view) fn policy_action_control_details(
    action: &OperatorPolicyActionSummary,
) -> String {
    let mut parts = Vec::new();
    append_policy_action_control_parts(action, &mut parts);
    parts.join(" ")
}

pub(in crate::tui::view) fn policy_action_control_summary(
    endpoint: &OperatorProviderEndpointSummary,
    action: &OperatorPolicyActionSummary,
    endpoint_width: usize,
) -> String {
    format!(
        "{} {}",
        shorten_middle(&endpoint.name, endpoint_width),
        policy_action_control_details(action)
    )
}

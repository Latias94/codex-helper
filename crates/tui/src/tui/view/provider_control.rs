use crate::policy_actions::PolicyActionProjection;
use crate::tui::model::shorten_middle;

fn append_policy_action_control_parts(
    action: &PolicyActionProjection,
    reason_width: usize,
    parts: &mut Vec<String>,
) {
    if action.active_cooldown {
        let cooldown = action
            .cooldown_remaining_secs
            .map(|secs| format!("{secs}s"))
            .unwrap_or_else(|| "?".to_string());
        parts.push(format!("cooldown={cooldown}"));
    }

    if let Some(reason) = action
        .reason
        .as_deref()
        .filter(|reason| !reason.trim().is_empty())
    {
        parts.push(format!("reason={}", shorten_middle(reason, reason_width)));
    }
}

pub(in crate::tui::view) fn policy_action_control_details(
    action: &PolicyActionProjection,
    reason_width: usize,
) -> String {
    let mut parts = Vec::new();
    append_policy_action_control_parts(action, reason_width, &mut parts);
    if parts.is_empty() {
        parts.push("action".to_string());
    }
    parts.join(" ")
}

pub(in crate::tui::view) fn policy_action_control_summary(
    action: &PolicyActionProjection,
    endpoint_width: usize,
    reason_width: usize,
) -> String {
    format!(
        "{} {}",
        shorten_middle(&action.provider_endpoint_key.stable_key(), endpoint_width),
        policy_action_control_details(action, reason_width)
    )
}

pub(in crate::tui::view) fn routing_policy_action_control_details(
    action: &PolicyActionProjection,
    reason_width: usize,
) -> String {
    let mut parts = vec![format!(
        "endpoint={}",
        action.provider_endpoint_key.endpoint_id.as_str()
    )];
    append_policy_action_control_parts(action, reason_width, &mut parts);
    parts.join(" ")
}

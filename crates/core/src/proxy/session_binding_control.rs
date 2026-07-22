use axum::http::StatusCode;

use crate::config::resolve_service_profile_from_catalog;
use crate::state::{
    SessionBinding, SessionBindingProjection, SessionContinuityMode, session_binding_revision,
};

use super::{ProxyControlError, ProxyService};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum OperatorSessionBindingCommand {
    SetProfile { profile_name: Option<String> },
    SetModel { model: Option<String> },
    SetReasoningEffort { reasoning_effort: Option<String> },
    SetServiceTier { service_tier: Option<String> },
    ResetManualOverrides,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorSessionBindingMutationRequest {
    pub session_key: String,
    pub expected_binding_revision: String,
    pub command: OperatorSessionBindingCommand,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorSessionBindingMutationStatus {
    Applied,
    Unchanged,
    Conflict,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorSessionBindingMutationResponse {
    pub status: OperatorSessionBindingMutationStatus,
    pub session_key: String,
    pub binding: SessionBindingProjection,
}

pub(super) async fn mutate_operator_session_binding(
    proxy: &ProxyService,
    request: OperatorSessionBindingMutationRequest,
) -> Result<OperatorSessionBindingMutationResponse, ProxyControlError> {
    let session_key = required_value(request.session_key.as_str(), "session_key")?;
    let expected_revision = required_value(
        request.expected_binding_revision.as_str(),
        "expected_binding_revision",
    )?;

    let capture = proxy.operator_read_capture().await?;
    let session_id = capture
        .local_sessions
        .get(session_key)
        .map(|session| session.raw_session_id.clone())
        .ok_or_else(|| {
            ProxyControlError::new(
                StatusCode::NOT_FOUND,
                "operator session key is not present in the current local read model",
            )
        })?;

    // The same per-session lock serializes this mutation with request admission and
    // binding capture. An in-flight request keeps its captured value; the next
    // request observes the committed binding.
    let _route_control_guard = proxy.state.lock_session_route_control(&session_id).await;
    let current = proxy.state.get_session_binding(&session_id).await;
    if session_binding_revision(current.as_ref()) != expected_revision {
        return Ok(response(
            OperatorSessionBindingMutationStatus::Conflict,
            session_key,
            current.as_ref(),
        ));
    }

    let runtime_snapshot = proxy.config.capture().await;
    let runtime_config = runtime_snapshot.config();
    let view = super::control_plane_service::service_route_config(
        runtime_config.as_ref(),
        proxy.service_name,
    );
    let now_ms = crate::logging::now_ms();
    let desired = desired_binding(
        current.as_ref(),
        &session_id,
        &request.command,
        view,
        now_ms,
    )?;

    if bindings_equivalent(current.as_ref(), desired.as_ref()) {
        return Ok(response(
            OperatorSessionBindingMutationStatus::Unchanged,
            session_key,
            desired.as_ref(),
        ));
    }

    if let Some(binding) = desired.as_ref() {
        proxy.state.set_session_binding(binding.clone()).await;
    } else {
        proxy.state.clear_session_binding(&session_id).await;
    }

    Ok(response(
        OperatorSessionBindingMutationStatus::Applied,
        session_key,
        desired.as_ref(),
    ))
}

fn desired_binding(
    current: Option<&SessionBinding>,
    session_id: &str,
    command: &OperatorSessionBindingCommand,
    view: &crate::config::ServiceRouteConfig,
    now_ms: u64,
) -> Result<Option<SessionBinding>, ProxyControlError> {
    match command {
        OperatorSessionBindingCommand::SetProfile { profile_name } => {
            let Some(profile_name) = profile_name.as_deref() else {
                return Ok(None);
            };
            let profile_name = normalized_value(profile_name, "profile_name", 128)?;
            let profile = resolve_service_profile_from_catalog(&view.profiles, &profile_name)
                .map_err(|_| conflict("profile_name is not present in the current runtime"))?;
            Ok(Some(SessionBinding {
                session_id: session_id.to_string(),
                profile_name: Some(profile_name),
                model: profile.model,
                reasoning_effort: profile.reasoning_effort,
                service_tier: normalize_service_tier(profile.service_tier.as_deref()),
                continuity_mode: SessionContinuityMode::ManualProfile,
                created_at_ms: current
                    .map(|binding| binding.created_at_ms)
                    .unwrap_or(now_ms),
                updated_at_ms: now_ms,
                last_seen_ms: now_ms,
            }))
        }
        OperatorSessionBindingCommand::ResetManualOverrides => Ok(None),
        OperatorSessionBindingCommand::SetModel { model } => {
            let mut binding = manual_binding_seed(current, session_id, now_ms);
            binding.model = model
                .as_deref()
                .map(|value| normalized_value(value, "model", 512))
                .transpose()?;
            Ok(collapse_empty_manual_binding(binding))
        }
        OperatorSessionBindingCommand::SetReasoningEffort { reasoning_effort } => {
            let mut binding = manual_binding_seed(current, session_id, now_ms);
            binding.reasoning_effort = reasoning_effort
                .as_deref()
                .map(|value| normalized_value(value, "reasoning_effort", 64))
                .transpose()?;
            Ok(collapse_empty_manual_binding(binding))
        }
        OperatorSessionBindingCommand::SetServiceTier { service_tier } => {
            let mut binding = manual_binding_seed(current, session_id, now_ms);
            binding.service_tier = service_tier
                .as_deref()
                .map(|value| normalized_value(value, "service_tier", 64))
                .transpose()
                .map(|value| {
                    value.and_then(|value| normalize_service_tier(Some(value.as_str())))
                })?;
            Ok(collapse_empty_manual_binding(binding))
        }
    }
}

fn manual_binding_seed(
    current: Option<&SessionBinding>,
    session_id: &str,
    now_ms: u64,
) -> SessionBinding {
    match current.filter(|binding| binding.continuity_mode == SessionContinuityMode::ManualProfile)
    {
        Some(binding) => SessionBinding {
            updated_at_ms: now_ms,
            last_seen_ms: now_ms,
            ..binding.clone()
        },
        None => SessionBinding {
            session_id: session_id.to_string(),
            profile_name: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_seen_ms: now_ms,
        },
    }
}

fn collapse_empty_manual_binding(binding: SessionBinding) -> Option<SessionBinding> {
    if binding.profile_name.is_none()
        && binding.model.is_none()
        && binding.reasoning_effort.is_none()
        && binding.service_tier.is_none()
    {
        None
    } else {
        Some(binding)
    }
}

fn bindings_equivalent(left: Option<&SessionBinding>, right: Option<&SessionBinding>) -> bool {
    session_binding_revision(left) == session_binding_revision(right)
}

fn response(
    status: OperatorSessionBindingMutationStatus,
    session_key: &str,
    binding: Option<&SessionBinding>,
) -> OperatorSessionBindingMutationResponse {
    OperatorSessionBindingMutationResponse {
        status,
        session_key: session_key.to_string(),
        binding: SessionBindingProjection::from_binding(binding),
    }
}

fn normalize_service_tier(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        None
    } else if value.eq_ignore_ascii_case("fast") {
        Some("priority".to_string())
    } else {
        Some(value.to_string())
    }
}

fn normalized_value(
    value: &str,
    field: &str,
    max_bytes: usize,
) -> Result<String, ProxyControlError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad_request(format!("{field} is empty")));
    }
    if value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(bad_request(format!("{field} is invalid or too long")));
    }
    Ok(value.to_string())
}

fn required_value<'a>(value: &'a str, field: &str) -> Result<&'a str, ProxyControlError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad_request(format!("{field} is empty")));
    }
    Ok(value)
}

fn bad_request(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::BAD_REQUEST, message)
}

fn conflict(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::CONFLICT, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_service_tier_is_normalized_to_priority() {
        assert_eq!(
            normalize_service_tier(Some("fast")),
            Some("priority".to_string())
        );
        assert_eq!(normalize_service_tier(Some("AUTO")), None);
    }

    #[test]
    fn clearing_the_only_manual_value_removes_the_binding() {
        let binding = SessionBinding {
            session_id: "sid".to_string(),
            profile_name: None,
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            service_tier: None,
            continuity_mode: SessionContinuityMode::ManualProfile,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_seen_ms: 1,
        };
        let desired = desired_binding(
            Some(&binding),
            "sid",
            &OperatorSessionBindingCommand::SetModel { model: None },
            &crate::config::ServiceRouteConfig::default(),
            10,
        )
        .expect("build desired binding");
        assert!(desired.is_none());
    }

    #[test]
    fn setting_a_field_on_default_profile_does_not_apply_other_defaults() {
        let binding = SessionBinding {
            session_id: "sid".to_string(),
            profile_name: Some("daily".to_string()),
            model: Some("profile-model".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: None,
            continuity_mode: SessionContinuityMode::DefaultProfile,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_seen_ms: 1,
        };
        let desired = desired_binding(
            Some(&binding),
            "sid",
            &OperatorSessionBindingCommand::SetModel {
                model: Some("manual-model".to_string()),
            },
            &crate::config::ServiceRouteConfig::default(),
            10,
        )
        .expect("build desired binding")
        .expect("manual binding");
        assert_eq!(desired.model.as_deref(), Some("manual-model"));
        assert!(desired.reasoning_effort.is_none());
        assert!(desired.profile_name.is_none());
        assert_eq!(
            desired.continuity_mode,
            SessionContinuityMode::ManualProfile
        );
    }
}

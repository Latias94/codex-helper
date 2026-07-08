use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("failed to resolve path: {0}")]
    Path(String),
    #[error("desktop config action failed: {0}")]
    Config(String),
    #[error("desktop lifecycle action failed: {0}")]
    Lifecycle(String),
    #[error("client switch action failed: {0}")]
    Switch(String),
}

impl DesktopError {
    pub fn code(&self) -> &'static str {
        match self {
            DesktopError::Path(_) => "desktop_path_error",
            DesktopError::Config(_) => "desktop_config_error",
            DesktopError::Lifecycle(_) => "desktop_lifecycle_error",
            DesktopError::Switch(_) => "desktop_switch_error",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, DesktopError::Lifecycle(_))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hint: Option<String>,
}

impl CommandError {
    pub(crate) fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            hint: None,
        }
    }

    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<DesktopError> for CommandError {
    fn from(value: DesktopError) -> Self {
        Self {
            code: value.code().to_string(),
            message: value.to_string(),
            retryable: value.retryable(),
            hint: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_error_serializes_stable_code_and_message() {
        let err: CommandError = DesktopError::Lifecycle("connection refused".to_string()).into();
        let value = serde_json::to_value(&err).expect("serialize command error");

        assert_eq!(value["code"].as_str(), Some("desktop_lifecycle_error"));
        assert_eq!(
            value["message"].as_str(),
            Some("desktop lifecycle action failed: connection refused")
        );
        assert_eq!(value["retryable"].as_bool(), Some(true));
    }

    #[test]
    fn command_error_builder_keeps_message_compatibility() {
        let err = CommandError::new("desktop_admin_http_403", "HTTP 403 forbidden", false)
            .with_hint("set CODEX_HELPER_ADMIN_TOKEN");
        let value = serde_json::to_value(&err).expect("serialize command error");

        assert_eq!(value["code"].as_str(), Some("desktop_admin_http_403"));
        assert_eq!(value["message"].as_str(), Some("HTTP 403 forbidden"));
        assert_eq!(value["retryable"].as_bool(), Some(false));
        assert_eq!(value["hint"].as_str(), Some("set CODEX_HELPER_ADMIN_TOKEN"));
    }
}

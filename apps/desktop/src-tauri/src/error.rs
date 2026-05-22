use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    #[error("failed to resolve path: {0}")]
    Path(String),
    #[error("admin API request failed: {0}")]
    AdminApi(String),
    #[error("desktop lifecycle action failed: {0}")]
    Lifecycle(String),
    #[error("client switch action failed: {0}")]
    Switch(String),
}

#[derive(Debug, Serialize)]
pub struct CommandError {
    message: String,
}

impl From<DesktopError> for CommandError {
    fn from(value: DesktopError) -> Self {
        Self {
            message: value.to_string(),
        }
    }
}

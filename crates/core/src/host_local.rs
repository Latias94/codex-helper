use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLocalSessionHistoryMode {
    Auto,
    Disabled,
    Enabled,
}

impl Default for HostLocalSessionHistoryMode {
    fn default() -> Self {
        Self::Auto
    }
}

pub fn host_local_session_history_available() -> bool {
    host_local_session_history_available_for_mode(HostLocalSessionHistoryMode::Auto)
}

pub fn host_local_session_history_available_for_mode(mode: HostLocalSessionHistoryMode) -> bool {
    host_local_session_history_available_in_dir(
        configured_mode(mode),
        &crate::config::codex_sessions_dir(),
    )
}

pub fn host_local_session_history_available_in_dir(
    mode: HostLocalSessionHistoryMode,
    sessions_dir: &Path,
) -> bool {
    match mode {
        HostLocalSessionHistoryMode::Disabled => false,
        HostLocalSessionHistoryMode::Auto | HostLocalSessionHistoryMode::Enabled => {
            std::fs::metadata(sessions_dir)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        }
    }
}

fn configured_mode(mode: HostLocalSessionHistoryMode) -> HostLocalSessionHistoryMode {
    match mode {
        HostLocalSessionHistoryMode::Auto => {
            mode_from_env().unwrap_or(HostLocalSessionHistoryMode::Auto)
        }
        mode => mode,
    }
}

fn mode_from_env() -> Option<HostLocalSessionHistoryMode> {
    let value = std::env::var("CODEX_HELPER_HOST_LOCAL_SESSION_HISTORY").ok()?;
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "0" | "false" | "off" | "disabled" | "disable" => {
            Some(HostLocalSessionHistoryMode::Disabled)
        }
        "1" | "true" | "on" | "enabled" | "enable" => Some(HostLocalSessionHistoryMode::Enabled),
        "" | "auto" => Some(HostLocalSessionHistoryMode::Auto),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_local_history_disabled_even_when_dir_exists() {
        let dir = std::env::temp_dir().join(format!(
            "codex-helper-host-local-disabled-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp sessions dir");

        assert!(!host_local_session_history_available_in_dir(
            HostLocalSessionHistoryMode::Disabled,
            &dir
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_local_history_auto_requires_existing_dir() {
        let dir = std::env::temp_dir().join(format!(
            "codex-helper-host-local-auto-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(!host_local_session_history_available_in_dir(
            HostLocalSessionHistoryMode::Auto,
            &dir
        ));

        std::fs::create_dir_all(&dir).expect("create temp sessions dir");
        assert!(host_local_session_history_available_in_dir(
            HostLocalSessionHistoryMode::Auto,
            &dir
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

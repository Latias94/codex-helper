use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLocalSessionHistoryMode {
    Auto,
    Disabled,
    Enabled,
}

const MODE_AUTO: u8 = 0;
const MODE_DISABLED: u8 = 1;
const MODE_ENABLED: u8 = 2;

static SESSION_HISTORY_MODE: AtomicU8 = AtomicU8::new(MODE_AUTO);

pub fn set_host_local_session_history_mode(mode: HostLocalSessionHistoryMode) {
    SESSION_HISTORY_MODE.store(mode_to_u8(mode), Ordering::Relaxed);
}

pub fn host_local_session_history_available() -> bool {
    host_local_session_history_available_in_dir(
        configured_host_local_session_history_mode(),
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

fn configured_host_local_session_history_mode() -> HostLocalSessionHistoryMode {
    match u8_to_mode(SESSION_HISTORY_MODE.load(Ordering::Relaxed)) {
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

fn mode_to_u8(mode: HostLocalSessionHistoryMode) -> u8 {
    match mode {
        HostLocalSessionHistoryMode::Auto => MODE_AUTO,
        HostLocalSessionHistoryMode::Disabled => MODE_DISABLED,
        HostLocalSessionHistoryMode::Enabled => MODE_ENABLED,
    }
}

fn u8_to_mode(value: u8) -> HostLocalSessionHistoryMode {
    match value {
        MODE_DISABLED => HostLocalSessionHistoryMode::Disabled,
        MODE_ENABLED => HostLocalSessionHistoryMode::Enabled,
        _ => HostLocalSessionHistoryMode::Auto,
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

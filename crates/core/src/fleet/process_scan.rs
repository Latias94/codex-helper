use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use super::model::FleetProcessSummary;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FleetProcessInfo {
    pub pid: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FleetProcessScan {
    pub processes: Vec<FleetProcessInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl FleetProcessScan {
    pub fn summary(&self) -> FleetProcessSummary {
        FleetProcessSummary {
            scan_available: self.error.is_none(),
            codex_like_processes: self.processes.len(),
            error: self.error.clone(),
        }
    }
}

pub fn scan_codex_processes() -> FleetProcessScan {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    let mut processes = system
        .processes()
        .values()
        .filter(|process| is_codex_like_process(process))
        .map(process_info)
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| process.pid);

    FleetProcessScan {
        processes,
        error: None,
    }
}

fn process_info(process: &sysinfo::Process) -> FleetProcessInfo {
    FleetProcessInfo {
        pid: pid_to_u32(process.pid()),
        name: process.name().to_string_lossy().into_owned(),
        command: command_line(process),
        cwd: process.cwd().map(|path| path.display().to_string()),
        started_at_ms: Some(process.start_time().saturating_mul(1_000)),
    }
}

fn is_codex_like_process(process: &sysinfo::Process) -> bool {
    let name = process.name().to_string_lossy().to_ascii_lowercase();
    let command = process
        .cmd()
        .iter()
        .map(|part| part.to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    is_codex_like_text(&name) || is_codex_like_text(&command)
}

fn is_codex_like_text(value: &str) -> bool {
    value.contains("codex")
        || value.contains("codex-helper")
        || value.contains("openai-codex")
        || value.contains("codex.exe")
}

fn command_line(process: &sysinfo::Process) -> Option<String> {
    let parts = process
        .cmd()
        .iter()
        .map(|part| part.to_string_lossy())
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn pid_to_u32(pid: Pid) -> u32 {
    pid.as_u32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_like_text_matches_expected_process_names() {
        assert!(is_codex_like_text("codex"));
        assert!(is_codex_like_text("codex-helper.exe"));
        assert!(is_codex_like_text("openai-codex"));
        assert!(!is_codex_like_text("cargo"));
    }
}

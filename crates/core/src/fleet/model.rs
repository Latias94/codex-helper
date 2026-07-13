use serde::{Deserialize, Serialize};

use crate::usage::UsageMetrics;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetNodeKind {
    #[default]
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetNodeHealth {
    #[default]
    Fresh,
    Stale,
    AuthFailed,
    RateLimited,
    Unsupported,
    Unreachable,
    ParseFailed,
}

impl FleetNodeHealth {
    pub fn is_current(self) -> bool {
        matches!(self, FleetNodeHealth::Fresh)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetWorkUnitKind {
    #[default]
    Root,
    Subagent,
    Process,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetWorkUnitState {
    #[default]
    Unknown,
    Running,
    WaitingInput,
    WaitingApproval,
    Idle,
    Interrupted,
    Completed,
    Errored,
    Exited,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetEvidenceSource {
    RuntimeStatus,
    SessionLog,
    ProcessScan,
    CachedSnapshot,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetConfidence {
    High,
    Medium,
    Low,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetEvidence {
    pub source: FleetEvidenceSource,
    pub confidence: FleetConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl FleetEvidence {
    pub fn high(source: FleetEvidenceSource) -> Self {
        Self {
            source,
            confidence: FleetConfidence::High,
            detail: None,
        }
    }

    pub fn with_detail(
        source: FleetEvidenceSource,
        confidence: FleetConfidence,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            source,
            confidence,
            detail: Some(detail.into()),
        }
    }
}

impl Default for FleetEvidence {
    fn default() -> Self {
        Self {
            source: FleetEvidenceSource::Unavailable,
            confidence: FleetConfidence::Unknown,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FleetUsageSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns_with_usage: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_output_tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetWorkUnit {
    pub node_id: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub kind: FleetWorkUnitKind,
    pub state: FleetWorkUnitState,
    pub evidence: FleetEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub usage: FleetUsageSummary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetGraphStatus {
    Available,
    #[default]
    Unavailable,
    Partial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FleetEdgeStatus {
    Open,
    Closed,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FleetSubagentEdge {
    pub node_id: String,
    pub parent_id: String,
    pub child_id: String,
    pub status: FleetEdgeStatus,
    pub evidence: FleetEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FleetTopology {
    pub status: FleetGraphStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<FleetSubagentEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FleetProcessSummary {
    #[serde(default)]
    pub scan_available: bool,
    #[serde(default)]
    pub codex_like_processes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetNodeSnapshot {
    pub node_id: String,
    pub label: String,
    pub kind: FleetNodeKind,
    pub health: FleetNodeHealth,
    pub refreshed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_since_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_age_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub processes: FleetProcessSummary,
    pub topology: FleetTopology,
    #[serde(default)]
    pub work_units: Vec<FleetWorkUnit>,
}

impl FleetNodeSnapshot {
    pub fn current_work_units(&self) -> impl Iterator<Item = &FleetWorkUnit> {
        self.work_units.iter().filter(|unit| {
            matches!(
                unit.state,
                FleetWorkUnitState::Running
                    | FleetWorkUnitState::WaitingInput
                    | FleetWorkUnitState::WaitingApproval
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetSnapshot {
    pub api_version: u32,
    pub service_name: String,
    pub refreshed_at_ms: u64,
    #[serde(default)]
    pub nodes: Vec<FleetNodeSnapshot>,
}

impl FleetSnapshot {
    pub fn active_work_units(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| node.current_work_units().count())
            .sum()
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

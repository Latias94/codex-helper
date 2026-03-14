use std::collections::{HashMap, HashSet};

use super::history_external::ExternalHistoryFocus;
use crate::sessions::{SessionDayDir, SessionIndexItem, SessionSummary, SessionTranscriptMessage};

#[derive(Debug)]
pub struct HistoryViewState {
    pub scope: HistoryScope,
    pub query: String,
    pub sessions_all: Vec<SessionSummary>,
    pub sessions: Vec<SessionSummary>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub selected_idx: usize,
    pub selected_id: Option<String>,
    pub(super) applied_scope: HistoryScope,
    pub(super) applied_query: String,
    pub recent_since_minutes: u32,
    pub recent_limit: usize,
    pub infer_git_root: bool,
    pub resume_cmd: String,
    pub shell: String,
    pub keep_open: bool,
    pub layout_mode: String,
    pub sessions_panel_height: f32,
    pub wt_window: i32,
    pub batch_selected_ids: HashSet<String>,
    pub group_by_workdir: bool,
    pub collapsed_workdirs: HashSet<String>,
    pub group_open_recent_n: usize,
    pub all_days_limit: usize,
    pub all_dates: Vec<SessionDayDir>,
    pub all_selected_date: Option<String>,
    pub all_day_limit: usize,
    pub all_day_sessions: Vec<SessionIndexItem>,
    pub(super) loaded_day_for: Option<String>,
    pub search_transcript_tail: bool,
    pub search_transcript_tail_n: usize,
    pub(super) search_transcript_applied: Option<(HistoryScope, String, usize)>,
    pub hide_tool_calls: bool,
    pub transcript_view: TranscriptViewMode,
    pub transcript_selected_msg_idx: usize,
    pub transcript_find_query: String,
    pub transcript_find_case_sensitive: bool,
    pub(super) transcript_scroll_to_msg_idx: Option<usize>,
    pub(super) transcript_plain_key: Option<(String, Option<usize>, bool)>,
    pub(super) transcript_plain_text: String,
    pub(super) transcript_load_seq: u64,
    pub(super) transcript_load: Option<TranscriptLoad>,
    pub auto_load_transcript: bool,
    pub transcript_full: bool,
    pub transcript_tail: usize,
    pub transcript_raw_messages: Vec<SessionTranscriptMessage>,
    pub transcript_messages: Vec<SessionTranscriptMessage>,
    pub transcript_error: Option<String>,
    pub(super) loaded_for: Option<(String, Option<usize>)>,
    pub branch_by_workdir: HashMap<String, Option<String>>,
    pub data_source: HistoryDataSource,
    pub(super) external_focus: Option<ExternalHistoryFocus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryScope {
    CurrentProject,
    GlobalRecent,
    AllByDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryDataSource {
    #[default]
    LocalFiles,
    ObservedFallback,
}

pub(super) const RECENT_WINDOWS: &[(u32, &str)] = &[
    (30, "30m"),
    (60, "1h"),
    (3 * 60, "3h"),
    (8 * 60, "8h"),
    (12 * 60, "12h"),
    (24 * 60, "24h"),
    (7 * 24 * 60, "7d"),
];

impl Default for HistoryViewState {
    fn default() -> Self {
        Self {
            scope: HistoryScope::CurrentProject,
            query: String::new(),
            sessions_all: Vec::new(),
            sessions: Vec::new(),
            last_error: None,
            loaded_at_ms: None,
            selected_idx: 0,
            selected_id: None,
            applied_scope: HistoryScope::CurrentProject,
            applied_query: String::new(),
            recent_since_minutes: 12 * 60,
            recent_limit: 50,
            infer_git_root: false,
            resume_cmd: "codex resume {id}".to_string(),
            shell: "pwsh".to_string(),
            keep_open: true,
            layout_mode: "auto".to_string(),
            sessions_panel_height: 280.0,
            wt_window: -1,
            batch_selected_ids: HashSet::new(),
            group_by_workdir: true,
            collapsed_workdirs: HashSet::new(),
            group_open_recent_n: 5,
            all_days_limit: 120,
            all_dates: Vec::new(),
            all_selected_date: None,
            all_day_limit: 500,
            all_day_sessions: Vec::new(),
            loaded_day_for: None,
            search_transcript_tail: false,
            search_transcript_tail_n: 80,
            search_transcript_applied: None,
            hide_tool_calls: true,
            transcript_view: TranscriptViewMode::Messages,
            transcript_selected_msg_idx: 0,
            transcript_find_query: String::new(),
            transcript_find_case_sensitive: false,
            transcript_scroll_to_msg_idx: None,
            transcript_plain_key: None,
            transcript_plain_text: String::new(),
            transcript_load_seq: 0,
            transcript_load: None,
            auto_load_transcript: true,
            transcript_full: false,
            transcript_tail: 80,
            transcript_raw_messages: Vec::new(),
            transcript_messages: Vec::new(),
            transcript_error: None,
            loaded_for: None,
            branch_by_workdir: HashMap::new(),
            data_source: HistoryDataSource::LocalFiles,
            external_focus: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptViewMode {
    Messages,
    PlainText,
}

#[derive(Debug)]
pub(in crate::gui::pages) struct TranscriptLoad {
    pub(super) seq: u64,
    pub(super) key: (String, Option<usize>),
    pub(super) rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Vec<SessionTranscriptMessage>>)>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::super::i18n::{Language, pick};
use super::components::{history_controls, history_sessions, history_transcript};
use super::{Page, PageCtx, remote_attached_proxy_active, remote_local_only_warning_message};
use super::{
    build_wt_items_from_session_summaries, format_age, history_workdir_from_cwd, now_ms,
    open_wt_items, sort_session_summaries_by_mtime_desc,
};

use crate::gui::proxy_control::GuiRuntimeSnapshot;
use crate::sessions::{
    SessionDayDir, SessionIndexItem, SessionSummary, SessionSummarySource, SessionTranscriptMessage,
};
use crate::state::SessionIdentityCard;

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
    applied_scope: HistoryScope,
    applied_query: String,
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
    search_transcript_applied: Option<(HistoryScope, String, usize)>,
    pub hide_tool_calls: bool,
    pub transcript_view: TranscriptViewMode,
    pub transcript_selected_msg_idx: usize,
    pub transcript_find_query: String,
    pub transcript_find_case_sensitive: bool,
    pub(super) transcript_scroll_to_msg_idx: Option<usize>,
    pub(super) transcript_plain_key: Option<(String, Option<usize>, bool)>,
    pub(super) transcript_plain_text: String,
    transcript_load_seq: u64,
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
    external_focus: Option<ExternalHistoryFocus>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExternalHistoryOrigin {
    Sessions,
}

#[derive(Debug, Clone)]
struct ExternalHistoryFocus {
    summary: SessionSummary,
    origin: ExternalHistoryOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedHistoryLayout {
    Horizontal,
    Vertical,
}

fn resolve_history_layout(layout_mode: &str, available_width: f32) -> ResolvedHistoryLayout {
    match layout_mode.trim().to_ascii_lowercase().as_str() {
        "horizontal" | "h" => ResolvedHistoryLayout::Horizontal,
        "vertical" | "v" => ResolvedHistoryLayout::Vertical,
        "auto" | "" => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
        _ => {
            if available_width < 980.0 {
                ResolvedHistoryLayout::Vertical
            } else {
                ResolvedHistoryLayout::Horizontal
            }
        }
    }
}

const RECENT_WINDOWS: &[(u32, &str)] = &[
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

fn find_git_root_upward(workdir: &str) -> Option<std::path::PathBuf> {
    let trimmed = workdir.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }
    let path = std::path::PathBuf::from(trimmed);
    if !path.is_absolute() {
        return None;
    }
    if !path.exists() {
        return None;
    }

    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let mut cur = canonical.clone();
    loop {
        if cur.join(".git").exists() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

fn read_git_branch_shallow(workdir: &str) -> Option<String> {
    let root = find_git_root_upward(workdir)?;
    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return None;
    }

    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = std::fs::read_to_string(&dot_git).ok()?;
        let first = content.lines().next()?.trim();
        let path = first.strip_prefix("gitdir:")?.trim();
        let mut p = std::path::PathBuf::from(path);
        if p.is_relative() {
            p = root.join(p);
        }
        p
    };

    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.lines().next().unwrap_or("").trim();
    if let Some(r) = head.strip_prefix("ref:") {
        let r = r.trim();
        return Some(r.rsplit('/').next().unwrap_or(r).to_string());
    }
    if head.len() >= 8 {
        Some(head[..8].to_string())
    } else if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}

fn refresh_branch_cache_for_sessions(
    branch_by_workdir: &mut HashMap<String, Option<String>>,
    infer_git_root: bool,
    sessions: &[SessionSummary],
) {
    branch_by_workdir.clear();
    for s in sessions {
        let Some(cwd) = s.cwd.as_deref() else {
            continue;
        };
        let workdir = history_workdir_from_cwd(cwd, infer_git_root);
        if workdir == "-" || workdir.trim().is_empty() {
            continue;
        }
        if branch_by_workdir.contains_key(workdir.as_str()) {
            continue;
        }
        let b = read_git_branch_shallow(workdir.as_str());
        branch_by_workdir.insert(workdir, b);
    }
}

fn refresh_branch_cache_for_day_items(
    branch_by_workdir: &mut HashMap<String, Option<String>>,
    infer_git_root: bool,
    items: &[SessionIndexItem],
) {
    for s in items {
        let Some(cwd) = s.cwd.as_deref() else {
            continue;
        };
        let workdir = history_workdir_from_cwd(cwd, infer_git_root);
        if workdir == "-" || workdir.trim().is_empty() {
            continue;
        }
        if branch_by_workdir.contains_key(workdir.as_str()) {
            continue;
        }
        let b = read_git_branch_shallow(workdir.as_str());
        branch_by_workdir.insert(workdir, b);
    }
}

fn merge_external_focus_session(list: &mut Vec<SessionSummary>, focus: &ExternalHistoryFocus) {
    if let Some(existing) = list
        .iter_mut()
        .find(|summary| summary.id == focus.summary.id)
    {
        let prefer_focus = matches!(focus.summary.source, SessionSummarySource::LocalFile)
            && !matches!(existing.source, SessionSummarySource::LocalFile);
        if prefer_focus {
            *existing = focus.summary.clone();
            return;
        }

        if existing.cwd.is_none() {
            existing.cwd = focus.summary.cwd.clone();
        }
        if existing.first_user_message.is_none() {
            existing.first_user_message = focus.summary.first_user_message.clone();
        }
        if existing.sort_hint_ms.is_none() {
            existing.sort_hint_ms = focus.summary.sort_hint_ms;
        }
        if existing.updated_at.is_none() {
            existing.updated_at = focus.summary.updated_at.clone();
        }
        if existing.last_response_at.is_none() {
            existing.last_response_at = focus.summary.last_response_at.clone();
        }
        if existing.path.as_os_str().is_empty() && !focus.summary.path.as_os_str().is_empty() {
            existing.path = focus.summary.path.clone();
        }
        return;
    }

    list.insert(0, focus.summary.clone());
}

fn ensure_external_focus_visible(state: &mut HistoryViewState) {
    let Some(focus) = state.external_focus.as_ref() else {
        return;
    };
    if state.selected_id.as_deref() != Some(focus.summary.id.as_str()) {
        return;
    }
    if !state
        .sessions
        .iter()
        .any(|summary| summary.id == focus.summary.id)
    {
        state.sessions.insert(0, focus.summary.clone());
    }
}

pub(super) fn prepare_select_session_from_external(
    state: &mut HistoryViewState,
    summary: SessionSummary,
    origin: ExternalHistoryOrigin,
) {
    let sid = summary.id.clone();
    state.scope = HistoryScope::GlobalRecent;
    state.query.clear();
    state.applied_scope = HistoryScope::GlobalRecent;
    state.applied_query.clear();
    state.search_transcript_applied = None;
    state.external_focus = Some(ExternalHistoryFocus { summary, origin });
    if let Some(focus) = state.external_focus.as_ref() {
        merge_external_focus_session(&mut state.sessions_all, focus);
        merge_external_focus_session(&mut state.sessions, focus);
    }
    state.selected_idx = 0;
    state.selected_id = Some(sid.clone());
    ensure_external_focus_visible(state);
    state.loaded_at_ms = None;
    state.loaded_for = None;
    state.auto_load_transcript = true;
    cancel_transcript_load(state);
    state.transcript_raw_messages.clear();
    state.transcript_messages.clear();
    state.transcript_error = None;
    state.transcript_scroll_to_msg_idx = None;
    state.transcript_plain_key = None;
    state.transcript_plain_text.clear();
    state.transcript_selected_msg_idx = 0;
}

pub(super) fn history_session_supports_local_actions(summary: &SessionSummary) -> bool {
    matches!(summary.source, SessionSummarySource::LocalFile)
}

fn observed_summary_sort_ms(card: &SessionIdentityCard) -> Option<u64> {
    card.last_ended_at_ms.or(card.active_started_at_ms_min)
}

fn observed_route_summary_from_card(card: &SessionIdentityCard, lang: Language) -> String {
    let station = card
        .effective_config_name
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_config_name.as_deref())
        .unwrap_or("auto");
    let model = card
        .effective_model
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_model.as_deref())
        .unwrap_or("auto");
    let tier = card
        .effective_service_tier
        .as_ref()
        .map(|value| value.value.as_str())
        .or(card.last_service_tier.as_deref())
        .unwrap_or("auto");

    let mut parts = vec![
        format!("station={station}"),
        format!("model={model}"),
        format!("tier={tier}"),
    ];
    if let Some(provider) = card.last_provider_id.as_deref() {
        parts.push(format!("provider={provider}"));
    }
    if let Some(client) = super::format_observed_client_identity(
        card.last_client_name.as_deref(),
        card.last_client_addr.as_deref(),
    ) {
        parts.push(format!("client={client}"));
    }
    if let Some(profile) = card.binding_profile_name.as_deref() {
        parts.push(format!("profile={profile}"));
    }
    if let Some(status) = card.last_status {
        parts.push(format!("status={status}"));
    }
    if card.active_count > 0 {
        parts.push(format!("active={}", card.active_count));
    }
    format!(
        "{}: {}",
        pick(lang, "共享观测", "Observed"),
        parts.join(", ")
    )
}

fn build_observed_summary_from_card(
    card: &SessionIdentityCard,
    lang: Language,
    now: u64,
) -> Option<SessionSummary> {
    let sid = card.session_id.clone()?;
    let sort_hint_ms = observed_summary_sort_ms(card);
    let updated_at = sort_hint_ms.map(|ms| format_age(now, Some(ms)));
    let turns = card.turns_total.unwrap_or(0).min(usize::MAX as u64) as usize;
    Some(SessionSummary {
        id: sid,
        path: PathBuf::new(),
        cwd: card.cwd.clone(),
        created_at: None,
        updated_at: updated_at.clone(),
        last_response_at: updated_at,
        user_turns: turns,
        assistant_turns: turns,
        rounds: turns,
        first_user_message: Some(observed_route_summary_from_card(card, lang)),
        source: SessionSummarySource::ObservedOnly,
        sort_hint_ms,
    })
}

fn build_observed_history_summaries(
    snapshot: &GuiRuntimeSnapshot,
    lang: Language,
) -> Vec<SessionSummary> {
    let now = now_ms();
    if !snapshot.session_cards.is_empty() {
        let mut out = snapshot
            .session_cards
            .iter()
            .filter_map(|card| build_observed_summary_from_card(card, lang, now))
            .collect::<Vec<_>>();
        sort_session_summaries_by_mtime_desc(&mut out);
        return out;
    }

    #[derive(Debug, Default)]
    struct ObservedAggregate {
        cwd: Option<String>,
        sort_hint_ms: Option<u64>,
        client_name: Option<String>,
        client_addr: Option<String>,
        model: Option<String>,
        tier: Option<String>,
        station: Option<String>,
        provider: Option<String>,
        status: Option<u16>,
        active_count: u64,
    }

    let mut map: HashMap<String, ObservedAggregate> = HashMap::new();
    for req in snapshot.active.iter() {
        let Some(sid) = req.session_id.as_deref().map(str::to_owned) else {
            continue;
        };
        let entry = map.entry(sid).or_default();
        if entry.cwd.is_none() {
            entry.cwd = req.cwd.clone();
        }
        if entry.client_name.is_none() {
            entry.client_name = req.client_name.clone();
        }
        if entry.client_addr.is_none() {
            entry.client_addr = req.client_addr.clone();
        }
        entry.sort_hint_ms = Some(
            entry
                .sort_hint_ms
                .unwrap_or(req.started_at_ms)
                .max(req.started_at_ms),
        );
        if entry.model.is_none() {
            entry.model = req.model.clone();
        }
        if entry.tier.is_none() {
            entry.tier = req.service_tier.clone();
        }
        if entry.station.is_none() {
            entry.station = req.config_name.clone();
        }
        if entry.provider.is_none() {
            entry.provider = req.provider_id.clone();
        }
        entry.active_count = entry.active_count.saturating_add(1);
    }

    for req in snapshot.recent.iter() {
        let Some(sid) = req.session_id.as_deref().map(str::to_owned) else {
            continue;
        };
        let entry = map.entry(sid).or_default();
        if entry.cwd.is_none() {
            entry.cwd = req.cwd.clone();
        }
        entry.client_name = req.client_name.clone().or(entry.client_name.clone());
        entry.client_addr = req.client_addr.clone().or(entry.client_addr.clone());
        entry.sort_hint_ms = Some(
            entry
                .sort_hint_ms
                .unwrap_or(req.ended_at_ms)
                .max(req.ended_at_ms),
        );
        entry.model = req.model.clone().or(entry.model.clone());
        entry.tier = req.service_tier.clone().or(entry.tier.clone());
        entry.station = req.config_name.clone().or(entry.station.clone());
        entry.provider = req.provider_id.clone().or(entry.provider.clone());
        entry.status = Some(req.status_code);
    }

    let mut out = map
        .into_iter()
        .map(|(sid, aggregate)| {
            let updated_at = aggregate.sort_hint_ms.map(|ms| format_age(now, Some(ms)));
            let mut parts = vec![
                format!("station={}", aggregate.station.as_deref().unwrap_or("auto")),
                format!("model={}", aggregate.model.as_deref().unwrap_or("auto")),
                format!("tier={}", aggregate.tier.as_deref().unwrap_or("auto")),
            ];
            if let Some(provider) = aggregate.provider.as_deref() {
                parts.push(format!("provider={provider}"));
            }
            if let Some(client) = super::format_observed_client_identity(
                aggregate.client_name.as_deref(),
                aggregate.client_addr.as_deref(),
            ) {
                parts.push(format!("client={client}"));
            }
            if let Some(status) = aggregate.status {
                parts.push(format!("status={status}"));
            }
            if aggregate.active_count > 0 {
                parts.push(format!("active={}", aggregate.active_count));
            }

            SessionSummary {
                id: sid,
                path: PathBuf::new(),
                cwd: aggregate.cwd,
                created_at: None,
                updated_at: updated_at.clone(),
                last_response_at: updated_at,
                user_turns: 0,
                assistant_turns: 0,
                rounds: 0,
                first_user_message: Some(format!(
                    "{}: {}",
                    pick(lang, "共享观测", "Observed"),
                    parts.join(", ")
                )),
                source: SessionSummarySource::ObservedOnly,
                sort_hint_ms: aggregate.sort_hint_ms,
            }
        })
        .collect::<Vec<_>>();
    sort_session_summaries_by_mtime_desc(&mut out);
    out
}

fn history_summary_source_label(source: SessionSummarySource, lang: Language) -> &'static str {
    match source {
        SessionSummarySource::LocalFile => {
            pick(lang, "本地 transcript 文件", "Local transcript file")
        }
        SessionSummarySource::ObservedOnly => pick(lang, "共享观测摘要", "Shared observed summary"),
    }
}

fn external_history_origin_label(origin: ExternalHistoryOrigin, lang: Language) -> &'static str {
    match origin {
        ExternalHistoryOrigin::Sessions => pick(lang, "来自 Sessions", "Opened from Sessions"),
    }
}

fn render_history_selection_context(
    ui: &mut egui::Ui,
    lang: Language,
    state: &HistoryViewState,
    summary: &SessionSummary,
) {
    let color = match summary.source {
        SessionSummarySource::LocalFile => egui::Color32::from_rgb(60, 160, 90),
        SessionSummarySource::ObservedOnly => egui::Color32::from_rgb(200, 120, 40),
    };
    let focus_origin = state
        .external_focus
        .as_ref()
        .filter(|focus| focus.summary.id == summary.id)
        .map(|focus| focus.origin);

    ui.group(|ui| {
        if let Some(origin) = focus_origin {
            ui.small(format!(
                "{}: {}",
                pick(lang, "入口", "Entry"),
                external_history_origin_label(origin, lang)
            ));
        }
        ui.colored_label(
            color,
            format!(
                "{}: {}",
                pick(lang, "来源", "Source"),
                history_summary_source_label(summary.source, lang)
            ),
        );
        match summary.source {
            SessionSummarySource::LocalFile => {
                ui.small(pick(
                    lang,
                    "当前条目映射到这台设备可读取的本地 session 文件；resume、open file 和 transcript 动作都可用。",
                    "This item maps to a local session file readable on this device; resume, open-file, and transcript actions are available.",
                ));
                if !summary.path.as_os_str().is_empty() {
                    ui.small(format!("file: {}", summary.path.display()));
                }
            }
            SessionSummarySource::ObservedOnly => {
                ui.small(pick(
                    lang,
                    "当前条目只带共享观测摘要；可以浏览 session 标识和路由线索，但不能假设这台设备有对应 transcript 文件。",
                    "This item carries shared observed metadata only; you can inspect session identity and routing clues, but this device cannot assume a matching transcript file exists.",
                ));
            }
        }
    });
}

fn refresh_history_sessions_with_fallback(
    ctx: &mut PageCtx<'_>,
    scope: HistoryScope,
    observed_fallback_supported: bool,
) -> anyhow::Result<(Vec<SessionSummary>, HistoryDataSource)> {
    let recent_since_minutes = ctx.view.history.recent_since_minutes;
    let recent_limit = ctx.view.history.recent_limit;
    let local_result = ctx.rt.block_on(async move {
        match scope {
            HistoryScope::CurrentProject => {
                crate::sessions::find_codex_sessions_for_current_dir(200).await
            }
            HistoryScope::GlobalRecent => {
                let since = std::time::Duration::from_secs(
                    (recent_since_minutes as u64).saturating_mul(60),
                );
                crate::sessions::find_recent_codex_session_summaries(since, recent_limit).await
            }
            HistoryScope::AllByDate => Ok(Vec::new()),
        }
    });

    match local_result {
        Ok(mut list) => {
            if !list.is_empty() || !observed_fallback_supported {
                sort_session_summaries_by_mtime_desc(&mut list);
                return Ok((list, HistoryDataSource::LocalFiles));
            }

            let observed = ctx
                .proxy
                .snapshot()
                .map(|snapshot| build_observed_history_summaries(&snapshot, ctx.lang))
                .unwrap_or_default();
            if !observed.is_empty() {
                Ok((observed, HistoryDataSource::ObservedFallback))
            } else {
                sort_session_summaries_by_mtime_desc(&mut list);
                Ok((list, HistoryDataSource::LocalFiles))
            }
        }
        Err(err) => {
            if !observed_fallback_supported {
                return Err(err);
            }
            let observed = ctx
                .proxy
                .snapshot()
                .map(|snapshot| build_observed_history_summaries(&snapshot, ctx.lang))
                .unwrap_or_default();
            if !observed.is_empty() {
                Ok((observed, HistoryDataSource::ObservedFallback))
            } else {
                Err(err)
            }
        }
    }
}

fn render_observed_session_placeholder(
    ui: &mut egui::Ui,
    lang: Language,
    summary: &SessionSummary,
) {
    ui.colored_label(
        egui::Color32::from_rgb(200, 120, 40),
        pick(
            lang,
            "当前会话只有共享观测摘要，没有可直接读取的 host-local transcript 文件。",
            "This session currently has only shared observed metadata; no host-local transcript file is available to read directly.",
        ),
    );
    ui.small(pick(
        lang,
        "你仍然可以在这里查看 session 标识、cwd 和最近路由摘要；需要更完整控制时请回到 Sessions / Requests。",
        "You can still inspect the session identity, cwd, and recent route summary here; return to Sessions / Requests for broader control data.",
    ));
    ui.add_space(6.0);

    if let Some(summary_line) = summary.first_user_message.as_deref() {
        let mut text = summary_line.to_string();
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .desired_rows(4)
                .font(egui::TextStyle::Monospace)
                .interactive(false),
        );
    } else {
        ui.label(pick(lang, "（无可用摘要）", "(no summary available)"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptViewMode {
    Messages,
    PlainText,
}

#[derive(Debug)]
pub(in crate::gui::pages) struct TranscriptLoad {
    seq: u64,
    key: (String, Option<usize>),
    rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Vec<SessionTranscriptMessage>>)>,
    join: tokio::task::JoinHandle<()>,
}

pub(super) fn render_history(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    poll_transcript_loader(ctx);
    let mut refresh_requested = false;
    let remote_attached = remote_attached_proxy_active(ctx.proxy);
    let (shared_observed_history_available, attached_host_local_history_advertised) = ctx
        .proxy
        .snapshot()
        .map(|snapshot| {
            (
                snapshot.shared_capabilities.session_observability,
                snapshot.host_local_capabilities.session_history,
            )
        })
        .unwrap_or((false, false));

    ui.heading(pick(ctx.lang, "历史会话", "History"));
    ui.label(pick(
        ctx.lang,
        "读取 Codex 的本地 sessions（~/.codex/sessions）。",
        "Reads local Codex sessions (~/.codex/sessions).",
    ));
    if remote_attached {
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                if shared_observed_history_available {
                    pick(
                        ctx.lang,
                        "当前附着的是远端代理。本页优先读取这台设备自己的 ~/.codex/sessions；若本机 history 为空，会退到共享观测摘要，但那仍不代表远端 host 的原始 transcript 文件。",
                        "A remote proxy is attached. This page prefers this device's ~/.codex/sessions; if local history is empty it falls back to shared observed summaries, which still do not represent the remote host's raw transcript files.",
                    )
                } else {
                    pick(
                        ctx.lang,
                        "当前附着的是远端代理。本页仍只会读取这台设备自己的 ~/.codex/sessions；当前附着目标未声明共享 history 观测能力，因此无法退到共享观测摘要。",
                        "A remote proxy is attached. This page still reads only this device's ~/.codex/sessions; the attached target does not advertise shared history observability, so observed fallback is unavailable.",
                    )
                },
            );
            ui.small(if shared_observed_history_available {
                pick(
                    ctx.lang,
                    "当本机 history 为空时，本页会退到共享观测摘要；更完整的 session / route / request 观测仍建议看 Sessions 或 Requests。",
                    "When local history is empty, this page falls back to shared observed summaries; use Sessions or Requests for fuller session/route/request observability.",
                )
            } else {
                pick(
                    ctx.lang,
                    "当前模式下 host-local transcript / cwd 仍不会直接映射到远端机器；如需更完整的共享观测，请查看 Sessions 或 Requests。",
                    "In this mode, host-local transcript/cwd access still does not map to the remote machine; use Sessions or Requests for fuller shared observability.",
                )
            });
            if let Some(att) = ctx.proxy.attached()
                && let Some(warning) = remote_local_only_warning_message(
                    att.admin_base_url.as_str(),
                    &att.host_local_capabilities,
                    ctx.lang,
                    &[
                        pick(ctx.lang, "resume", "resume"),
                        pick(ctx.lang, "open file", "open file"),
                        pick(ctx.lang, "transcript", "transcript"),
                    ],
                )
            {
                ui.small(warning);
            }
            if attached_host_local_history_advertised {
                ui.small(pick(
                    ctx.lang,
                    "附着目标声明其代理主机本地具备 session history 能力，但那不会自动映射为当前设备可读的 transcript 文件。",
                    "The attached target advertises host-local session history on its own machine, but that does not automatically map to transcript files readable from this device.",
                ));
            }
            ui.horizontal(|ui| {
                if ui.button(pick(ctx.lang, "转到会话", "Go to Sessions")).clicked() {
                    ctx.view.requested_page = Some(Page::Sessions);
                }
                if ui.button(pick(ctx.lang, "转到请求", "Go to Requests")).clicked() {
                    ctx.view.requested_page = Some(Page::Requests);
                }
            });
        });
    }

    if remote_attached
        && ctx.view.history.scope != HistoryScope::AllByDate
        && ctx.view.history.loaded_at_ms.is_none()
        && ctx.view.history.sessions_all.is_empty()
    {
        refresh_requested = true;
    }
    if ctx
        .view
        .history
        .external_focus
        .as_ref()
        .is_some_and(|focus| {
            !ctx.view
                .history
                .sessions_all
                .iter()
                .any(|summary| summary.id == focus.summary.id)
        })
    {
        refresh_requested = true;
    }

    ui.add_space(6.0);

    // Shortcuts (Global recent only):
    // - Ctrl+Y: copy visible root+id list
    // - Ctrl+Enter: copy selected root+id
    if ctx.view.history.scope == HistoryScope::GlobalRecent {
        let copy_list = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Y);
        if ui.ctx().input_mut(|i| i.consume_shortcut(&copy_list)) {
            let mut out = String::new();
            for s in ctx.view.history.sessions.iter() {
                let cwd = s.cwd.as_deref().unwrap_or("-");
                if cwd == "-" {
                    continue;
                }
                let root = history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root);
                out.push_str(root.trim());
                out.push(' ');
                out.push_str(s.id.as_str());
                out.push('\n');
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }

        let copy_selected = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Enter);
        if ui.ctx().input_mut(|i| i.consume_shortcut(&copy_selected)) {
            if let Some(s) = ctx
                .view
                .history
                .selected_id
                .as_deref()
                .and_then(|id| ctx.view.history.sessions.iter().find(|s| s.id == id))
            {
                let cwd = s.cwd.as_deref().unwrap_or("-");
                if cwd == "-" {
                    *ctx.last_error =
                        Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
                } else {
                    let workdir = history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root);
                    ui.ctx().copy_text(format!("{} {}", workdir.trim(), s.id));
                    *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
                }
            } else {
                *ctx.last_error =
                    Some(pick(ctx.lang, "未选中任何会话", "No session selected").to_string());
            }
        }
    }

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "范围", "Scope"));
        egui::ComboBox::from_id_salt("history_scope")
            .selected_text(match ctx.view.history.scope {
                HistoryScope::CurrentProject => pick(ctx.lang, "当前项目", "Current project"),
                HistoryScope::GlobalRecent => pick(ctx.lang, "全局最近", "Global recent"),
                HistoryScope::AllByDate => pick(ctx.lang, "全部(按日期)", "All (by date)"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::CurrentProject,
                    pick(ctx.lang, "当前项目", "Current project"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::GlobalRecent,
                    pick(ctx.lang, "全局最近", "Global recent"),
                );
                ui.selectable_value(
                    &mut ctx.view.history.scope,
                    HistoryScope::AllByDate,
                    pick(ctx.lang, "全部(按日期)", "All (by date)"),
                );
            });

        if ctx.view.history.scope == HistoryScope::GlobalRecent {
            let mut window_changed = false;
            ui.label(pick(ctx.lang, "窗口", "Window"));
            for (mins, label) in RECENT_WINDOWS.iter().copied() {
                let selected = ctx.view.history.recent_since_minutes == mins;
                if ui.selectable_label(selected, label).clicked() {
                    ctx.view.history.recent_since_minutes = mins;
                    window_changed = true;
                }
            }

            ui.label(pick(ctx.lang, "最近(分钟)", "Since (minutes)"));
            let before = ctx.view.history.recent_since_minutes;
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_since_minutes)
                    .range(5..=10_080)
                    .speed(5),
            )
            .on_hover_text(pick(
                ctx.lang,
                "建议优先用“窗口”快速切换；这里用于精确自定义。",
                "Prefer Window presets; use this for fine-grained customization.",
            ));
            if ctx.view.history.recent_since_minutes != before {
                window_changed = true;
            }
            let approx_h = (ctx.view.history.recent_since_minutes as f32) / 60.0;
            ui.label(format!("≈{approx_h:.1}h"));
            ui.label(pick(ctx.lang, "条数", "Limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.recent_limit)
                    .range(1..=500)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "工作目录", "Workdir"));
            let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
            if mode != "cwd" && mode != "git_root" {
                mode = "cwd".to_string();
            }
            let mut selected_mode = mode.clone();
            egui::ComboBox::from_id_salt("history_workdir_mode")
                .selected_text(match selected_mode.as_str() {
                    "git_root" => pick(ctx.lang, "git 根目录", "git root"),
                    _ => pick(ctx.lang, "会话 cwd", "session cwd"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_mode,
                        "cwd".to_string(),
                        pick(ctx.lang, "会话 cwd", "session cwd"),
                    )
                    .on_hover_text(pick(
                        ctx.lang,
                        "使用会话记录中的 cwd 作为恢复/复制的工作目录（推荐）",
                        "Use the session's cwd as workdir (recommended).",
                    ));
                    ui.selectable_value(
                        &mut selected_mode,
                        "git_root".to_string(),
                        pick(ctx.lang, "git 根目录", "git root"),
                    )
                    .on_hover_text(pick(
                        ctx.lang,
                        "在 cwd 上向上查找 .git 作为项目根目录（用于复制/打开）",
                        "Find .git upward from cwd as project root (for copy/open).",
                    ));
                });
            if selected_mode != mode {
                ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
                ctx.view.history.infer_git_root = selected_mode == "git_root";
                let infer_git_root = ctx.view.history.infer_git_root;
                let sessions = ctx.view.history.sessions_all.as_slice();
                refresh_branch_cache_for_sessions(
                    &mut ctx.view.history.branch_by_workdir,
                    infer_git_root,
                    sessions,
                );
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
                window_changed = true;
            }

            ui.checkbox(
                &mut ctx.view.history.group_by_workdir,
                pick(ctx.lang, "按项目分组", "Group by project"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "按工作目录分组并折叠，适合“第二天继续昨天的一堆会话”的批量恢复。",
                "Group by workdir with collapsible headers; great for batch resume next day.",
            ));

            if window_changed {
                refresh_requested = true;
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "窗口已更新（将影响下次刷新）",
                        "Window updated (affects next refresh)",
                    )
                    .to_string(),
                );
            }
        } else if ctx.view.history.scope == HistoryScope::AllByDate {
            ui.label(pick(ctx.lang, "最近天数", "Recent days"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_days_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "当日上限", "Day limit"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.all_day_limit)
                    .range(1..=10_000)
                    .speed(1),
            );
            ui.label(pick(ctx.lang, "工作目录", "Workdir"));
            let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
            if mode != "cwd" && mode != "git_root" {
                mode = "cwd".to_string();
            }
            let mut selected_mode = mode.clone();
            egui::ComboBox::from_id_salt("history_workdir_mode_all_by_date")
                .selected_text(match selected_mode.as_str() {
                    "git_root" => pick(ctx.lang, "git 根目录", "git root"),
                    _ => pick(ctx.lang, "会话 cwd", "session cwd"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut selected_mode,
                        "cwd".to_string(),
                        pick(ctx.lang, "会话 cwd", "session cwd"),
                    );
                    ui.selectable_value(
                        &mut selected_mode,
                        "git_root".to_string(),
                        pick(ctx.lang, "git 根目录", "git root"),
                    );
                });
            if selected_mode != mode {
                ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
                ctx.view.history.infer_git_root = selected_mode == "git_root";
                ctx.view.history.branch_by_workdir.clear();
                let infer_git_root = ctx.view.history.infer_git_root;
                let items = ctx.view.history.all_day_sessions.as_slice();
                refresh_branch_cache_for_day_items(
                    &mut ctx.view.history.branch_by_workdir,
                    infer_git_root,
                    items,
                );
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
            }
        }

        ui.separator();
        ui.label(pick(ctx.lang, "布局", "Layout"));
        let mut mode = ctx.view.history.layout_mode.trim().to_ascii_lowercase();
        if mode != "auto" && mode != "horizontal" && mode != "vertical" {
            mode = "auto".to_string();
        }
        let mut selected_mode = mode.clone();
        egui::ComboBox::from_id_salt("history_layout_mode")
            .selected_text(match selected_mode.as_str() {
                "horizontal" => pick(ctx.lang, "左右", "Horizontal"),
                "vertical" => pick(ctx.lang, "上下", "Vertical"),
                _ => pick(ctx.lang, "自动", "Auto"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut selected_mode,
                    "auto".to_string(),
                    pick(ctx.lang, "自动", "Auto"),
                );
                ui.selectable_value(
                    &mut selected_mode,
                    "horizontal".to_string(),
                    pick(ctx.lang, "左右", "Horizontal"),
                );
                ui.selectable_value(
                    &mut selected_mode,
                    "vertical".to_string(),
                    pick(ctx.lang, "上下", "Vertical"),
                );
            });
        if selected_mode != mode {
            ctx.view.history.layout_mode = selected_mode.clone();
            ctx.gui_cfg.history.layout_mode = selected_mode;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    if ctx.view.history.scope == HistoryScope::AllByDate {
        render_history_all_by_date(ui, ctx);
        return;
    }

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(240.0)
                .hint_text(pick(
                    ctx.lang,
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "输入关键词（匹配 cwd 或首条用户消息）"
                    } else {
                        "输入关键词（匹配首条用户消息）"
                    },
                    if ctx.view.history.scope == HistoryScope::GlobalRecent {
                        "keyword (cwd or first user message)"
                    } else {
                        "keyword (first user message)"
                    },
                )),
        );

        let mut action_apply_tail_search = false;
        let observed_fallback_active =
            ctx.view.history.data_source == HistoryDataSource::ObservedFallback;
        ui.add_enabled_ui(!observed_fallback_active, |ui| {
            ui.checkbox(
                &mut ctx.view.history.search_transcript_tail,
                pick(ctx.lang, "搜对话(尾部)", "Transcript (tail)"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "可选：在元信息不命中时，再扫描每个会话文件尾部的 N 条消息（更慢，但更像 cc-switch 的全文搜索）。",
                "Optional: if metadata doesn't match, scan the last N messages (slower, closer to cc-switch full-text).",
            ));
        });
        if observed_fallback_active {
            ui.small(pick(
                ctx.lang,
                "共享观测模式下没有本地 transcript 文件，因此这里只做元信息过滤。",
                "Observed mode has no local transcript files, so only metadata filtering is available here.",
            ));
        }
        if ctx.view.history.search_transcript_tail && !observed_fallback_active {
            ui.label(pick(ctx.lang, "N", "N"));
            ui.add(
                egui::DragValue::new(&mut ctx.view.history.search_transcript_tail_n)
                    .range(10..=500)
                    .speed(1),
            );
            if ui
                .button(pick(ctx.lang, "应用", "Apply"))
                .clicked()
            {
                action_apply_tail_search = true;
            }
        }

        if refresh_requested || ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let scope = ctx.view.history.scope;
            match refresh_history_sessions_with_fallback(
                ctx,
                scope,
                shared_observed_history_available,
            ) {
                Ok((mut list, data_source)) => {
                    if let Some(focus) = ctx.view.history.external_focus.as_ref() {
                        merge_external_focus_session(&mut list, focus);
                    }
                    ctx.view.history.sessions_all = list;
                    ctx.view.history.data_source = data_source;
                    let infer_git_root = ctx.view.history.infer_git_root;
                    let sessions = ctx.view.history.sessions_all.as_slice();
                    refresh_branch_cache_for_sessions(
                        &mut ctx.view.history.branch_by_workdir,
                        infer_git_root,
                        sessions,
                    );
                    ctx.view.history.search_transcript_applied = None;
                    ctx.view.history.loaded_at_ms = Some(now_ms());
                    ctx.view.history.last_error = None;

                    // Re-apply current metadata filter without hitting disk again.
                    let q = ctx.view.history.query.trim().to_lowercase();
                    let scope = ctx.view.history.scope;
                    ctx.view.history.sessions = if q.is_empty() {
                        ctx.view.history.sessions_all.clone()
                    } else {
                        ctx.view
                            .history
                            .sessions_all
                            .iter()
                            .filter(|&s| match scope {
                                HistoryScope::GlobalRecent => s
                                    .cwd
                                    .as_deref()
                                    .is_some_and(|cwd| cwd.to_lowercase().contains(q.as_str()))
                                    || s.first_user_message.as_deref().is_some_and(|msg| {
                                        msg.to_lowercase().contains(q.as_str())
                                    }),
                                _ => s.first_user_message.as_deref().is_some_and(|msg| {
                                    msg.to_lowercase().contains(q.as_str())
                                }),
                            })
                            .cloned()
                            .collect()
                    };
                    ensure_external_focus_visible(&mut ctx.view.history);
                    ctx.view.history.applied_scope = scope;
                    ctx.view.history.applied_query = ctx.view.history.query.clone();

                    if ctx.view.history.sessions.is_empty() {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id = None;
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = None;
                        ctx.view.history.transcript_plain_key = None;
                        ctx.view.history.transcript_plain_text.clear();
                    } else if ctx
                        .view
                        .history
                        .selected_id
                        .as_deref()
                        .is_none_or(|id| !ctx.view.history.sessions.iter().any(|s| s.id == id))
                    {
                        ctx.view.history.selected_idx = 0;
                        ctx.view.history.selected_id =
                            Some(ctx.view.history.sessions[0].id.clone());
                        ctx.view.history.loaded_for = None;
                        cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.transcript_plain_key = None;
                        ctx.view.history.transcript_plain_text.clear();
                    }
                    *ctx.last_info = Some(
                        if data_source == HistoryDataSource::ObservedFallback {
                            pick(ctx.lang, "已刷新（共享观测）", "Refreshed (observed)").to_string()
                        } else {
                            pick(ctx.lang, "已刷新", "Refreshed").to_string()
                        },
                    );
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ctx.view.history.scope == HistoryScope::GlobalRecent
            && ui
                .button(pick(ctx.lang, "复制 root+id 列表", "Copy root+id list"))
                .clicked()
        {
            let mut out = String::new();
            for s in ctx.view.history.sessions.iter() {
                let cwd = s.cwd.as_deref().unwrap_or("-");
                if cwd == "-" {
                    continue;
                }
                let root = if ctx.view.history.infer_git_root {
                    crate::sessions::infer_project_root_from_cwd(cwd)
                } else {
                    cwd.to_string()
                };
                out.push_str(root.trim());
                out.push(' ');
                out.push_str(s.id.as_str());
                out.push('\n');
            }
            ui.ctx().copy_text(out);
            *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
        }

        ui.add_enabled_ui(
            ctx.view.history.data_source == HistoryDataSource::LocalFiles,
            |ui| {
                ui.checkbox(
                    &mut ctx.view.history.auto_load_transcript,
                    pick(ctx.lang, "自动加载对话", "Auto load transcript"),
                );
            },
        );

        if action_apply_tail_search {
            if ctx.view.history.data_source == HistoryDataSource::ObservedFallback {
                *ctx.last_error = Some(pick(
                    ctx.lang,
                    "共享观测模式下没有本地 transcript 文件，不能执行尾部对话搜索。",
                    "Observed mode has no local transcript files, so transcript tail search is unavailable.",
                ).to_string());
            } else {
                let q = ctx.view.history.query.trim().to_string();
                if q.is_empty() {
                    *ctx.last_error = Some(pick(
                        ctx.lang,
                        "请输入关键词后再应用“搜对话(尾部)”",
                        "Enter a query before applying transcript search",
                    ).to_string());
                } else {
                    let scope = ctx.view.history.scope;
                    let tail = ctx.view.history.search_transcript_tail_n;
                    let all = ctx.view.history.sessions_all.clone();
                    let needle = q.clone();
                    let mut out: Vec<SessionSummary> = Vec::new();
                    let needle_lc = needle.to_lowercase();
                    let meta_match = |s: &SessionSummary| -> bool {
                        match scope {
                            HistoryScope::GlobalRecent => s
                                .cwd
                                .as_deref()
                                .is_some_and(|cwd| cwd.to_lowercase().contains(needle_lc.as_str()))
                                || s.first_user_message.as_deref().is_some_and(|msg| {
                                    msg.to_lowercase().contains(needle_lc.as_str())
                                }),
                            _ => s.first_user_message.as_deref().is_some_and(|msg| {
                                msg.to_lowercase().contains(needle_lc.as_str())
                            }),
                        }
                    };

                    let fut = async move {
                        for s in all.into_iter() {
                            if meta_match(&s) {
                                out.push(s);
                                continue;
                            }
                            if crate::sessions::codex_session_transcript_tail_contains_query(
                                &s.path,
                                &needle,
                                tail,
                            )
                            .await?
                            {
                                out.push(s);
                            }
                        }
                        Ok::<Vec<SessionSummary>, anyhow::Error>(out)
                    };
                    match ctx.rt.block_on(fut) {
                        Ok(list) => {
                            ctx.view.history.sessions = list;
                            ensure_external_focus_visible(&mut ctx.view.history);
                            ctx.view.history.search_transcript_applied = Some((scope, q, tail));
                            ctx.view.history.applied_scope = scope;
                            ctx.view.history.applied_query = ctx.view.history.query.clone();
                            ctx.view.history.selected_idx = 0;
                            ctx.view.history.selected_id =
                                ctx.view.history.sessions.first().map(|s| s.id.clone());
                            ctx.view.history.loaded_for = None;
                            cancel_transcript_load(&mut ctx.view.history);
                            ctx.view.history.transcript_raw_messages.clear();
                            ctx.view.history.transcript_messages.clear();
                            ctx.view.history.transcript_error = None;
                            ctx.view.history.transcript_plain_key = None;
                            ctx.view.history.transcript_plain_text.clear();
                            *ctx.last_info =
                                Some(pick(ctx.lang, "已应用全文过滤", "Applied").to_string());
                        }
                        Err(e) => {
                            ctx.view.history.last_error = Some(e.to_string());
                        }
                    }
                }
            }
        }
    });

    // Apply lightweight (metadata-only) filtering immediately when query/scope changes.
    if (ctx.view.history.applied_scope != ctx.view.history.scope
        || ctx.view.history.applied_query != ctx.view.history.query)
        && !matches!(ctx.view.history.scope, HistoryScope::AllByDate)
    {
        ctx.view.history.applied_scope = ctx.view.history.scope;
        ctx.view.history.applied_query = ctx.view.history.query.clone();
        ctx.view.history.search_transcript_applied = None;

        let q = ctx.view.history.query.trim().to_lowercase();
        if q.is_empty() {
            ctx.view.history.sessions = ctx.view.history.sessions_all.clone();
        } else {
            let scope = ctx.view.history.scope;
            ctx.view.history.sessions = ctx
                .view
                .history
                .sessions_all
                .iter()
                .filter(|s| match scope {
                    HistoryScope::GlobalRecent => {
                        s.cwd
                            .as_deref()
                            .is_some_and(|cwd| cwd.to_lowercase().contains(q.as_str()))
                            || s.first_user_message
                                .as_deref()
                                .is_some_and(|msg| msg.to_lowercase().contains(q.as_str()))
                    }
                    _ => s
                        .first_user_message
                        .as_deref()
                        .is_some_and(|msg| msg.to_lowercase().contains(q.as_str())),
                })
                .cloned()
                .collect();
        }
        ensure_external_focus_visible(&mut ctx.view.history);

        // If selection falls out, reset and clear transcript.
        let selected_ok = ctx
            .view
            .history
            .selected_id
            .as_deref()
            .is_some_and(|id| ctx.view.history.sessions.iter().any(|s| s.id == id));
        if !selected_ok {
            ctx.view.history.selected_idx = 0;
            ctx.view.history.selected_id = ctx.view.history.sessions.first().map(|s| s.id.clone());
            reset_transcript_view_after_session_switch(ctx);
        }
    }

    if ctx.view.history.data_source == HistoryDataSource::ObservedFallback {
        ui.add_space(4.0);
        ui.group(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "当前显示的是共享观测会话摘要，不是本机 ~/.codex/sessions 文件列表。",
                    "The current list is built from shared observed sessions, not this device's ~/.codex/sessions files.",
                ),
            );
            ui.small(pick(
                ctx.lang,
                "可用：筛选、选择、查看 route 摘要。不可用：transcript、resume、open file 这类 host-local 文件动作。",
                "Available: filtering, selection, and route summary browsing. Unavailable: transcript, resume, and open-file actions that require host-local files.",
            ));
        });
    }

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    if ctx.view.history.sessions.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无会话。点击“刷新”加载。",
            "No sessions loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selection stable.
    let selected_idx = ctx
        .view
        .history
        .selected_id
        .as_deref()
        .and_then(|id| ctx.view.history.sessions.iter().position(|s| s.id == id))
        .unwrap_or(
            ctx.view
                .history
                .selected_idx
                .min(ctx.view.history.sessions.len().saturating_sub(1)),
        );
    ctx.view.history.selected_idx = selected_idx;
    ctx.view.history.selected_id = Some(ctx.view.history.sessions[selected_idx].id.clone());

    if ctx.view.history.auto_load_transcript
        && ctx
            .view
            .history
            .sessions
            .get(selected_idx)
            .is_some_and(history_session_supports_local_actions)
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (id.clone(), tail);
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        ensure_transcript_loading(ctx, path, key);
    }

    ui.add_space(6.0);
    let layout =
        resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
    if layout == ResolvedHistoryLayout::Vertical {
        render_history_vertical(ui, ctx);
        return;
    }
    ui.columns(2, |cols| {
        let pending_select = history_sessions::render_sessions_panel_horizontal(&mut cols[0], ctx);

        if let Some((idx, id)) = pending_select {
            select_session_and_reset_transcript(ctx, idx, id);
        }

        cols[1].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[1].add_space(4.0);

        let selected_idx = ctx
            .view
            .history
            .selected_idx
            .min(ctx.view.history.sessions.len().saturating_sub(1));
        let selected = ctx.view.history.sessions[selected_idx].clone();
        let selected_id = selected.id.clone();
        let selected_source = selected.source;
        let workdir = history_workdir_from_cwd(
            selected.cwd.as_deref().unwrap_or("-"),
            ctx.view.history.infer_git_root,
        );
        let mut open_selected_clicked = false;

        cols[1].group(|ui| {
            open_selected_clicked =
                history_controls::render_resume_group(ui, ctx, "history_wt_batch_mode");

            ui.horizontal(|ui| {
                history_controls::render_selected_session_actions(
                    ui,
                    ctx,
                    selected_id.as_str(),
                    workdir.as_str(),
                    selected.path.as_path(),
                    selected_source,
                );
            });

            render_history_selection_context(ui, ctx.lang, &ctx.view.history, &selected);
            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        if open_selected_clicked {
            let selected_ids = ctx.view.history.batch_selected_ids.clone();
            let infer_git_root = ctx.view.history.infer_git_root;
            let resume_cmd = ctx.view.history.resume_cmd.clone();
            let items = build_wt_items_from_session_summaries(
                ctx.view
                    .history
                    .sessions
                    .iter()
                    .filter(|s| selected_ids.contains(&s.id)),
                infer_git_root,
                resume_cmd.as_str(),
            );
            open_wt_items(ctx, items);
        }

        if matches!(selected_source, SessionSummarySource::LocalFile) {
            history_transcript::render_transcript_toolbar(
                &mut cols[1],
                ctx,
                "history_transcript_view",
            );

            history_transcript::render_transcript_body(
                &mut cols[1],
                ctx.lang,
                &mut ctx.view.history,
                480.0,
            );
        } else {
            render_observed_session_placeholder(&mut cols[1], ctx.lang, &selected);
        }
    });
}

fn render_history_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(160.0, max_h * 0.55);

    let mut pending_select: Option<(usize, String)> = None;

    let resp = egui::TopBottomPanel::top("history_vertical_sessions_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(160.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            pending_select = history_sessions::render_sessions_panel_vertical(ui, ctx);
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
    if !pointer_down
        && (ctx.gui_cfg.history.sessions_panel_height - ctx.view.history.sessions_panel_height)
            .abs()
            > 2.0
    {
        ctx.gui_cfg.history.sessions_panel_height = ctx.view.history.sessions_panel_height;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        }
    }

    if let Some((idx, id)) = pending_select.take() {
        select_session_and_reset_transcript(ctx, idx, id);
    }

    let selected_idx = ctx
        .view
        .history
        .selected_idx
        .min(ctx.view.history.sessions.len().saturating_sub(1));
    if ctx.view.history.auto_load_transcript
        && ctx
            .view
            .history
            .sessions
            .get(selected_idx)
            .is_some_and(history_session_supports_local_actions)
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        ensure_transcript_loading(ctx, path, (id, tail));
    }

    ui.add_space(6.0);

    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let selected_idx = ctx
        .view
        .history
        .selected_idx
        .min(ctx.view.history.sessions.len().saturating_sub(1));
    let selected = ctx.view.history.sessions[selected_idx].clone();
    let selected_id = selected.id.clone();
    let selected_source = selected.source;
    let workdir = history_workdir_from_cwd(
        selected.cwd.as_deref().unwrap_or("-"),
        ctx.view.history.infer_git_root,
    );
    let mut open_selected_clicked = false;

    ui.horizontal(|ui| {
        history_controls::render_selected_session_actions(
            ui,
            ctx,
            selected_id.as_str(),
            workdir.as_str(),
            selected.path.as_path(),
            selected_source,
        );
        open_selected_clicked = history_controls::render_open_selected_in_wt_button(ui, ctx);
    });

    if open_selected_clicked {
        let selected_ids = ctx.view.history.batch_selected_ids.clone();
        let infer_git_root = ctx.view.history.infer_git_root;
        let resume_cmd = ctx.view.history.resume_cmd.clone();
        let items = build_wt_items_from_session_summaries(
            ctx.view
                .history
                .sessions
                .iter()
                .filter(|s| selected_ids.contains(&s.id)),
            infer_git_root,
            resume_cmd.as_str(),
        );
        open_wt_items(ctx, items);
    }

    render_history_selection_context(ui, ctx.lang, &ctx.view.history, &selected);
    ui.label(format!("id: {}", selected_id));
    ui.label(format!("dir: {}", workdir));

    if matches!(selected_source, SessionSummarySource::LocalFile) {
        history_transcript::render_transcript_toolbar(ui, ctx, "history_transcript_view");
        let transcript_max_h = ui.available_height();
        history_transcript::render_transcript_body(
            ui,
            ctx.lang,
            &mut ctx.view.history,
            transcript_max_h,
        );
    } else {
        render_observed_session_placeholder(ui, ctx.lang, &selected);
    }
}

fn render_history_all_by_date(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(260.0)
                .hint_text(pick(
                    ctx.lang,
                    "关键词（匹配 cwd 或首条用户消息）",
                    "keyword (cwd or first user message)",
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    ctx.view.history.loaded_day_for = None;
                    ctx.view.history.all_day_sessions.clear();
                    ctx.view.history.selected_id = None;
                    cancel_transcript_load(&mut ctx.view.history);
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        if ui
            .button(pick(ctx.lang, "加载更多天", "Load more days"))
            .clicked()
        {
            ctx.view.history.all_days_limit = ctx.view.history.all_days_limit.saturating_add(120);
            let limit = ctx.view.history.all_days_limit;
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_session_day_dirs(limit))
            {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }

        ui.checkbox(
            &mut ctx.view.history.auto_load_transcript,
            pick(ctx.lang, "自动加载对话", "Auto load transcript"),
        );
    });

    if let Some(err) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), err);
    }

    if ctx.view.history.all_dates.is_empty() {
        ui.add_space(8.0);
        ui.label(pick(
            ctx.lang,
            "暂无日期索引。点击“刷新”加载。",
            "No date index loaded. Click Refresh.",
        ));
        return;
    }

    // Keep selected date stable.
    if ctx
        .view
        .history
        .all_selected_date
        .as_deref()
        .is_none_or(|d| !ctx.view.history.all_dates.iter().any(|x| x.date == d))
    {
        ctx.view.history.all_selected_date = Some(ctx.view.history.all_dates[0].date.clone());
        ctx.view.history.loaded_day_for = None;
    }

    // Auto-load day sessions when date changes.
    if let Some(date) = ctx.view.history.all_selected_date.clone()
        && ctx.view.history.loaded_day_for.as_deref() != Some(date.as_str())
    {
        let limit = ctx.view.history.all_day_limit;
        let day_dir = ctx
            .view
            .history
            .all_dates
            .iter()
            .find(|x| x.date == date)
            .map(|x| x.path.clone());
        if let Some(day_dir) = day_dir {
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_sessions_in_day_dir(
                    &day_dir, limit,
                )) {
                Ok(mut list) => {
                    list.sort_by_key(|s| std::cmp::Reverse(s.mtime_ms));
                    ctx.view.history.all_day_sessions = list;
                    let infer_git_root = ctx.view.history.infer_git_root;
                    let items = ctx.view.history.all_day_sessions.as_slice();
                    refresh_branch_cache_for_day_items(
                        &mut ctx.view.history.branch_by_workdir,
                        infer_git_root,
                        items,
                    );
                    ctx.view.history.loaded_day_for = Some(date.clone());
                    ctx.view.history.selected_id = None;
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = None;
                }
                Err(e) => {
                    ctx.view.history.last_error = Some(e.to_string());
                }
            }
        }
    }

    let q = ctx.view.history.query.trim().to_lowercase();

    ui.add_space(6.0);
    let layout =
        resolve_history_layout(ctx.view.history.layout_mode.as_str(), ui.available_width());
    if layout == ResolvedHistoryLayout::Vertical {
        render_history_all_by_date_vertical(ui, ctx, q.as_str());
        return;
    }
    ui.columns(3, |cols| {
        history_sessions::render_all_by_date_dates_panel(
            &mut cols[0],
            ctx,
            520.0,
            "history_all_by_date_dates_scroll",
        );
        let pending_select = history_sessions::render_all_by_date_sessions_panel(
            &mut cols[1],
            ctx,
            q.as_str(),
            520.0,
            "history_all_by_date_sessions_scroll",
        );
        if let Some((idx, id)) = pending_select {
            select_session_and_reset_transcript(ctx, idx, id);
        }

        cols[2].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[2].add_space(4.0);

        let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
            ctx.view
                .history
                .all_day_sessions
                .iter()
                .position(|s| s.id == id)
        });
        let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx));

        if selected.is_none() {
            cols[2].label(pick(
                ctx.lang,
                "选择一个会话以预览对话。",
                "Select a session to preview.",
            ));
            return;
        }
        let (selected_id, selected_cwd, selected_path, selected_first) = {
            let selected = selected.unwrap();
            (
                selected.id.clone(),
                selected.cwd.clone().unwrap_or_else(|| "-".to_string()),
                selected.path.clone(),
                selected.first_user_message.clone(),
            )
        };

        let workdir =
            history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);
        let mut open_selected_clicked = false;

        cols[2].group(|ui| {
            open_selected_clicked =
                history_controls::render_resume_group(ui, ctx, "history_wt_batch_mode_all_by_date");

            ui.horizontal(|ui| {
                history_controls::render_selected_session_actions(
                    ui,
                    ctx,
                    selected_id.as_str(),
                    workdir.as_str(),
                    selected_path.as_path(),
                    SessionSummarySource::LocalFile,
                );
            });

            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected_first.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        if open_selected_clicked {
            let selected_ids = ctx.view.history.batch_selected_ids.clone();
            let infer_git_root = ctx.view.history.infer_git_root;
            let resume_cmd = ctx.view.history.resume_cmd.clone();
            let items = history_controls::build_wt_items_from_day_sessions(
                ctx.view
                    .history
                    .all_day_sessions
                    .iter()
                    .filter(|s| selected_ids.contains(&s.id)),
                infer_git_root,
                resume_cmd.as_str(),
            );
            open_wt_items(ctx, items);
        }

        if ctx.view.history.auto_load_transcript {
            let tail = if ctx.view.history.transcript_full {
                None
            } else {
                Some(ctx.view.history.transcript_tail)
            };
            let key = (selected_id.clone(), tail);
            ensure_transcript_loading(ctx, selected_path.clone(), key);
        }

        cols[2].add_space(6.0);
        history_transcript::render_transcript_toolbar(
            &mut cols[2],
            ctx,
            "history_transcript_view_all",
        );

        history_transcript::render_transcript_body(
            &mut cols[2],
            ctx.lang,
            &mut ctx.view.history,
            360.0,
        );
    });
}

fn render_history_all_by_date_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, q: &str) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(200.0, max_h * 0.55);

    let q = q.trim();
    let mut pending_select: Option<(usize, String)> = None;

    let resp = egui::TopBottomPanel::top("history_all_vertical_nav_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(200.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            ui.columns(2, |cols| {
                let max_h = cols[0].available_height().max(160.0);
                history_sessions::render_all_by_date_dates_panel(
                    &mut cols[0],
                    ctx,
                    max_h,
                    "history_all_by_date_dates_scroll",
                );

                let max_h = cols[1].available_height().max(160.0);
                pending_select = history_sessions::render_all_by_date_sessions_panel(
                    &mut cols[1],
                    ctx,
                    q,
                    max_h,
                    "history_all_by_date_sessions_scroll",
                );
            });
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
    if !pointer_down
        && (ctx.gui_cfg.history.sessions_panel_height - ctx.view.history.sessions_panel_height)
            .abs()
            > 2.0
    {
        ctx.gui_cfg.history.sessions_panel_height = ctx.view.history.sessions_panel_height;
        if let Err(e) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {e}"));
        }
    }

    if let Some((idx, id)) = pending_select {
        select_session_and_reset_transcript(ctx, idx, id);
    }

    ui.add_space(6.0);
    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
        ctx.view
            .history
            .all_day_sessions
            .iter()
            .position(|s| s.id == id)
    });
    let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx));
    if selected.is_none() {
        ui.label(pick(
            ctx.lang,
            "选择一个会话以预览对话。",
            "Select a session to preview.",
        ));
        return;
    }
    let (selected_id, selected_cwd, selected_path) = {
        let s = selected.unwrap();
        (
            s.id.clone(),
            s.cwd.clone().unwrap_or_else(|| "-".to_string()),
            s.path.clone(),
        )
    };

    let workdir = history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);

    if ctx.view.history.auto_load_transcript {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (selected_id.clone(), tail);
        ensure_transcript_loading(ctx, selected_path.clone(), key);
    }

    let mut open_selected_clicked = false;
    ui.horizontal(|ui| {
        history_controls::render_selected_session_actions(
            ui,
            ctx,
            selected_id.as_str(),
            workdir.as_str(),
            selected_path.as_path(),
            SessionSummarySource::LocalFile,
        );
        open_selected_clicked = history_controls::render_open_selected_in_wt_button(ui, ctx);
    });

    if open_selected_clicked {
        let selected_ids = ctx.view.history.batch_selected_ids.clone();
        let infer_git_root = ctx.view.history.infer_git_root;
        let resume_cmd = ctx.view.history.resume_cmd.clone();
        let items = history_controls::build_wt_items_from_day_sessions(
            ctx.view
                .history
                .all_day_sessions
                .iter()
                .filter(|s| selected_ids.contains(&s.id)),
            infer_git_root,
            resume_cmd.as_str(),
        );
        open_wt_items(ctx, items);
    }

    ui.label(format!("id: {}", selected_id));
    ui.label(format!("dir: {}", workdir));

    history_transcript::render_transcript_toolbar(ui, ctx, "history_transcript_view_all");

    let transcript_max_h = ui.available_height();
    history_transcript::render_transcript_body(
        ui,
        ctx.lang,
        &mut ctx.view.history,
        transcript_max_h,
    );
}

pub(in crate::gui::pages) fn cancel_transcript_load(state: &mut HistoryViewState) {
    if let Some(load) = state.transcript_load.take() {
        load.join.abort();
    }
}

fn poll_transcript_loader(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history.transcript_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history.transcript_load = None;
                return;
            }

            let key = load.key.clone();
            ctx.view.history.transcript_load = None;

            match res {
                Ok(msgs) => {
                    ctx.view.history.transcript_raw_messages = msgs;
                    ctx.view.history.transcript_messages = history_transcript::filter_tool_calls(
                        ctx.view.history.transcript_raw_messages.clone(),
                        ctx.view.history.hide_tool_calls,
                    );
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = Some(key);
                    ctx.view.history.transcript_selected_msg_idx = 0;
                    ctx.view.history.transcript_scroll_to_msg_idx = Some(0);
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
                Err(e) => {
                    ctx.view.history.transcript_raw_messages.clear();
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = Some(e.to_string());
                    ctx.view.history.loaded_for = None;
                    ctx.view.history.transcript_scroll_to_msg_idx = None;
                    ctx.view.history.transcript_plain_key = None;
                    ctx.view.history.transcript_plain_text.clear();
                }
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            ctx.view.history.transcript_load = None;
        }
    }
}

fn select_session_and_reset_transcript(ctx: &mut PageCtx<'_>, idx: usize, id: String) {
    if ctx
        .view
        .history
        .external_focus
        .as_ref()
        .is_some_and(|focus| focus.summary.id != id)
    {
        ctx.view.history.external_focus = None;
    }
    ctx.view.history.selected_idx = idx;
    ctx.view.history.selected_id = Some(id);
    reset_transcript_view_after_session_switch(ctx);
}

fn reset_transcript_view_after_session_switch(ctx: &mut PageCtx<'_>) {
    ctx.view.history.loaded_for = None;
    cancel_transcript_load(&mut ctx.view.history);
    ctx.view.history.transcript_raw_messages.clear();
    ctx.view.history.transcript_messages.clear();
    ctx.view.history.transcript_error = None;
    ctx.view.history.transcript_plain_key = None;
    ctx.view.history.transcript_plain_text.clear();
    ctx.view.history.transcript_selected_msg_idx = 0;
    ctx.view.history.transcript_scroll_to_msg_idx = None;
}

fn ensure_transcript_loading(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    if ctx.view.history.loaded_for.as_ref() == Some(&key) {
        return;
    }
    if let Some(load) = ctx.view.history.transcript_load.as_ref()
        && load.key == key
    {
        return;
    }

    start_transcript_load(ctx, path, key);
}

fn start_transcript_load(
    ctx: &mut PageCtx<'_>,
    path: std::path::PathBuf,
    key: (String, Option<usize>),
) {
    cancel_transcript_load(&mut ctx.view.history);

    ctx.view.history.transcript_load_seq = ctx.view.history.transcript_load_seq.saturating_add(1);
    let seq = ctx.view.history.transcript_load_seq;
    let tail = key.1;

    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let res = crate::sessions::read_codex_session_transcript(&path, tail).await;
        let _ = tx.send((seq, res));
    });

    ctx.view.history.transcript_load = Some(TranscriptLoad { seq, key, rx, join });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard_core::{
        ConfigOption, ControlProfileOption, HostLocalControlPlaneCapabilities,
        RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, WindowStats,
    };
    use crate::state::{FinishedRequest, ResolvedRouteValue, RouteValueSource, UsageRollupView};

    fn sample_summary(id: &str, source: SessionSummarySource) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            path: match source {
                SessionSummarySource::LocalFile => PathBuf::from(format!("/tmp/{id}.jsonl")),
                SessionSummarySource::ObservedOnly => PathBuf::new(),
            },
            cwd: Some("/workdir".to_string()),
            created_at: None,
            updated_at: Some("1m".to_string()),
            last_response_at: Some("1m".to_string()),
            user_turns: 1,
            assistant_turns: 1,
            rounds: 1,
            first_user_message: Some("summary".to_string()),
            source,
            sort_hint_ms: Some(1_000),
        }
    }

    fn empty_snapshot() -> GuiRuntimeSnapshot {
        GuiRuntimeSnapshot {
            kind: crate::gui::proxy_control::ProxyModeKind::Attached,
            base_url: Some("http://127.0.0.1:3210".to_string()),
            port: Some(3210),
            service_name: Some("codex".to_string()),
            last_error: None,
            active: Vec::new(),
            recent: Vec::new(),
            session_cards: Vec::new(),
            global_override: None,
            configured_active_station: None,
            effective_active_station: None,
            configured_default_profile: None,
            default_profile: None,
            profiles: Vec::<ControlProfileOption>::new(),
            session_model_overrides: HashMap::new(),
            session_config_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            configs: Vec::<ConfigOption>::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            configured_retry: None,
            resolved_retry: None,
            supports_v1: true,
            supports_retry_config_api: true,
            supports_persisted_station_config: true,
            supports_default_profile_override: true,
            supports_config_runtime_override: true,
            shared_capabilities: SharedControlPlaneCapabilities {
                session_observability: true,
                request_history: true,
            },
            host_local_capabilities: HostLocalControlPlaneCapabilities {
                session_history: true,
                transcript_read: true,
                cwd_enrichment: true,
            },
            remote_admin_access: RemoteAdminAccessCapabilities::default(),
        }
    }

    #[test]
    fn prepare_select_session_from_external_resets_scope_and_focus() {
        let mut state = HistoryViewState::default();
        state.scope = HistoryScope::CurrentProject;
        state.query = "old".to_string();
        state.applied_query = "old".to_string();

        prepare_select_session_from_external(
            &mut state,
            sample_summary("sid-ext", SessionSummarySource::ObservedOnly),
            ExternalHistoryOrigin::Sessions,
        );

        assert_eq!(state.scope, HistoryScope::GlobalRecent);
        assert!(state.query.is_empty());
        assert_eq!(state.selected_id.as_deref(), Some("sid-ext"));
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].id, "sid-ext");
        assert!(state.external_focus.is_some());
        assert!(state.loaded_at_ms.is_none());
    }

    #[test]
    fn merge_external_focus_session_preserves_local_file_when_richer() {
        let mut list = vec![sample_summary("sid-1", SessionSummarySource::LocalFile)];
        let focus = ExternalHistoryFocus {
            summary: sample_summary("sid-1", SessionSummarySource::ObservedOnly),
            origin: ExternalHistoryOrigin::Sessions,
        };

        merge_external_focus_session(&mut list, &focus);

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].source, SessionSummarySource::LocalFile);
        assert!(!list[0].path.as_os_str().is_empty());
    }

    #[test]
    fn ensure_external_focus_visible_inserts_selected_external_summary() {
        let mut state = HistoryViewState::default();
        state.external_focus = Some(ExternalHistoryFocus {
            summary: sample_summary("sid-ext", SessionSummarySource::ObservedOnly),
            origin: ExternalHistoryOrigin::Sessions,
        });
        state.selected_id = Some("sid-ext".to_string());

        ensure_external_focus_visible(&mut state);

        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].id, "sid-ext");
        assert_eq!(state.sessions[0].source, SessionSummarySource::ObservedOnly);
    }

    #[test]
    fn observed_history_summaries_from_cards_are_marked_observed_only() {
        let mut snapshot = empty_snapshot();
        snapshot.session_cards = vec![SessionIdentityCard {
            session_id: Some("sid-card".to_string()),
            last_client_name: Some("Frank-Desk".to_string()),
            last_client_addr: Some("100.64.0.12".to_string()),
            cwd: Some("/remote/workdir".to_string()),
            last_ended_at_ms: Some(2_000),
            last_status: Some(200),
            last_provider_id: Some("right".to_string()),
            binding_profile_name: Some("fast".to_string()),
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.4-fast".to_string(),
                source: RouteValueSource::StationMapping,
            }),
            effective_config_name: Some(ResolvedRouteValue {
                value: "right".to_string(),
                source: RouteValueSource::RuntimeFallback,
            }),
            effective_service_tier: Some(ResolvedRouteValue {
                value: "priority".to_string(),
                source: RouteValueSource::ProfileDefault,
            }),
            ..SessionIdentityCard::default()
        }];

        let summaries = build_observed_history_summaries(&snapshot, Language::Zh);

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "sid-card");
        assert_eq!(summaries[0].source, SessionSummarySource::ObservedOnly);
        assert_eq!(summaries[0].sort_hint_ms, Some(2_000));
        assert!(
            summaries[0]
                .first_user_message
                .as_deref()
                .is_some_and(|msg| {
                    msg.contains("station=right")
                        && msg.contains("model=gpt-5.4-fast")
                        && msg.contains("client=Frank-Desk @ 100.64.0.12")
                })
        );
        assert!(!history_session_supports_local_actions(&summaries[0]));
    }

    #[test]
    fn observed_history_summaries_fall_back_to_recent_requests() {
        let mut snapshot = empty_snapshot();
        snapshot.recent = vec![FinishedRequest {
            id: 1,
            session_id: Some("sid-recent".to_string()),
            client_name: Some("Tablet".to_string()),
            client_addr: Some("100.64.0.13".to_string()),
            cwd: Some("/remote/recent".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: None,
            service_tier: Some("priority".to_string()),
            config_name: Some("vibe".to_string()),
            provider_id: Some("vibe".to_string()),
            upstream_base_url: None,
            usage: None,
            retry: None,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 500,
            ttfb_ms: None,
            ended_at_ms: 9_000,
        }];

        let summaries = build_observed_history_summaries(&snapshot, Language::En);

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "sid-recent");
        assert_eq!(summaries[0].source, SessionSummarySource::ObservedOnly);
        assert_eq!(summaries[0].sort_hint_ms, Some(9_000));
        assert!(
            summaries[0]
                .first_user_message
                .as_deref()
                .is_some_and(|msg| {
                    msg.contains("station=vibe")
                        && msg.contains("provider=vibe")
                        && msg.contains("client=Tablet @ 100.64.0.13")
                })
        );
    }
}

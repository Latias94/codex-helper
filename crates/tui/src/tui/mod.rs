mod i18n;
mod input;
mod model;
mod report;
mod state;
mod terminal;
mod types;
mod view;

pub use i18n::Language;
pub use i18n::{detect_system_language, parse_language};
#[allow(unused_imports)]
pub use model::{ProviderOption, UpstreamSummary, build_provider_options};

use std::io;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crossterm::ExecutableCommand;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::LeaveAlternateScreen;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::watch;

use crate::config::ProxyConfig;
use crate::config::storage::load_config;
use crate::state::ProxyState;

use self::model::{Palette, now_ms, refresh_snapshot};
use self::state::{UiState, merge_codex_history_external_focus};
use self::terminal::TerminalGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderInvalidation {
    None,
    Redraw,
    FullClear,
}

fn request_redraw(invalidation: &mut RenderInvalidation) {
    if matches!(invalidation, RenderInvalidation::None) {
        *invalidation = RenderInvalidation::Redraw;
    }
}

fn request_full_clear(invalidation: &mut RenderInvalidation) {
    *invalidation = RenderInvalidation::FullClear;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderSurfaceKey {
    page: types::Page,
    overlay: types::Overlay,
    focus: types::Focus,
    stats_focus: types::StatsFocus,
    selected_station_idx: usize,
    selected_session_idx: usize,
    selected_request_idx: usize,
    selected_request_page_idx: usize,
    selected_sessions_page_idx: usize,
    selected_codex_history_idx: usize,
    codex_recent_selected_idx: usize,
    selected_stats_station_idx: usize,
    selected_stats_provider_idx: usize,
    effort_menu_idx: usize,
    model_menu_idx: usize,
    service_tier_menu_idx: usize,
    profile_menu_idx: usize,
    provider_menu_idx: usize,
    station_info_scroll: u16,
    session_transcript_scroll: u16,
}

impl RenderSurfaceKey {
    fn capture(ui: &UiState) -> Self {
        Self {
            page: ui.page,
            overlay: ui.overlay,
            focus: ui.focus,
            stats_focus: ui.stats_focus,
            selected_station_idx: ui.selected_station_idx,
            selected_session_idx: ui.selected_session_idx,
            selected_request_idx: ui.selected_request_idx,
            selected_request_page_idx: ui.selected_request_page_idx,
            selected_sessions_page_idx: ui.selected_sessions_page_idx,
            selected_codex_history_idx: ui.selected_codex_history_idx,
            codex_recent_selected_idx: ui.codex_recent_selected_idx,
            selected_stats_station_idx: ui.selected_stats_station_idx,
            selected_stats_provider_idx: ui.selected_stats_provider_idx,
            effort_menu_idx: ui.effort_menu_idx,
            model_menu_idx: ui.model_menu_idx,
            service_tier_menu_idx: ui.service_tier_menu_idx,
            profile_menu_idx: ui.profile_menu_idx,
            provider_menu_idx: ui.provider_menu_idx,
            station_info_scroll: ui.station_info_scroll,
            session_transcript_scroll: ui.session_transcript_scroll,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_dashboard(
    state: Arc<ProxyState>,
    cfg: Arc<ProxyConfig>,
    service_name: &'static str,
    port: u16,
    admin_port: u16,
    providers: Vec<ProviderOption>,
    language: Language,
    shutdown: watch::Sender<bool>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let refresh_ms = std::env::var("CODEX_HELPER_TUI_REFRESH_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(500)
        .clamp(100, 5_000);

    let io_timeout = Duration::from_millis((refresh_ms / 2).clamp(50, 250));

    let mut term_guard = TerminalGuard::enter()?;
    let stdout = io::stdout();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut ui = UiState {
        service_name,
        admin_port,
        language,
        refresh_ms,
        config_version: cfg.version,
        ..Default::default()
    };
    let _ = input::refresh_profile_control_state(&mut ui).await;
    let palette = Palette::default();

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(refresh_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut ctrl_c = Box::pin(tokio::signal::ctrl_c());

    let mut cfg = cfg;
    let mut snapshot = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days).await;
    let mut providers = providers;
    ui.clamp_selection(&snapshot, providers.len());

    let mut render_invalidation = RenderInvalidation::FullClear;
    let mut last_drawn_page = ui.page;
    loop {
        if render_invalidation != RenderInvalidation::None {
            if ui.page != last_drawn_page {
                // Defensive: some terminals occasionally leave stale cells when only a small
                // region changes (e.g., switching tabs). A full clear on page switch keeps the
                // UI visually consistent without clearing on every tick.
                request_full_clear(&mut render_invalidation);
                ui.reset_table_viewports();
                last_drawn_page = ui.page;
            }
            if matches!(render_invalidation, RenderInvalidation::FullClear) {
                terminal.clear()?;
            }
            terminal.draw(|f| {
                view::render_app(
                    f,
                    palette,
                    &mut ui,
                    &snapshot,
                    service_name,
                    port,
                    &providers,
                )
            })?;
            render_invalidation = RenderInvalidation::None;
        }

        if ui.should_exit || *shutdown_rx.borrow() {
            let _ = shutdown.send(true);
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {
                let refresh = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days);
                if let Ok(new_snapshot) = tokio::time::timeout(io_timeout, refresh).await {
                    snapshot = new_snapshot;
                    ui.clamp_selection(&snapshot, providers.len());
                }
                if ui.page == crate::tui::types::Page::Settings
                    && ui
                        .last_runtime_config_refresh_at
                        .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    let url = format!(
                        "http://127.0.0.1:{}/__codex_helper/api/v1/runtime/status",
                        ui.admin_port
                    );
                    let fetch = async {
                        let client = reqwest::Client::new();
                        client
                            .get(&url)
                            .send()
                            .await?
                            .error_for_status()?
                            .json::<serde_json::Value>()
                            .await
                    };
                    if let Ok(Ok(v)) = tokio::time::timeout(io_timeout, fetch).await {
                        ui.last_runtime_config_loaded_at_ms =
                            v.get("loaded_at_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_config_source_mtime_ms =
                            v.get("source_mtime_ms").and_then(|x| x.as_u64());
                        ui.last_runtime_retry = v
                            .get("retry")
                            .and_then(|x| serde_json::from_value(x.clone()).ok());
                    }
                    ui.last_runtime_config_refresh_at = Some(Instant::now());
                }
                request_redraw(&mut render_invalidation);
            }
            changed = shutdown_rx.changed() => {
                let _ = changed;
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            _ = &mut ctrl_c => {
                ui.should_exit = true;
                let _ = shutdown.send(true);
                break;
            }
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { continue; };
                match event {
                    Event::Key(key) if input::should_accept_key_event(&key) => {
                        let before_surface = RenderSurfaceKey::capture(&ui);
                        if input::handle_key_event(state.clone(), &mut providers, &mut ui, &snapshot, key).await {
                            if ui.needs_config_refresh {
                                match load_config().await {
                                    Ok(new_cfg) => {
                                        cfg = Arc::new(new_cfg);
                                        ui.config_version = cfg.version;
                                        providers = build_provider_options(&cfg, service_name);
                                        ui.clamp_selection(&snapshot, providers.len());
                                    }
                                    Err(err) => {
                                        ui.toast = Some((
                                            format!("config refresh failed: {err}"),
                                            Instant::now(),
                                        ));
                                    }
                                }
                                ui.needs_config_refresh = false;
                            }
                            if ui.needs_snapshot_refresh {
                                snapshot = refresh_snapshot(&state, cfg.clone(), service_name, ui.stats_days).await;
                                ui.clamp_selection(&snapshot, providers.len());
                                ui.needs_snapshot_refresh = false;
                            }
                            if ui.needs_codex_history_refresh {
                                ui.codex_history_error = None;
                                match crate::sessions::find_codex_sessions_for_current_dir(200).await {
                                    Ok(list) => {
                                        ui.codex_history_sessions = list;
                                        if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                                            merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
                                        }
                                        ui.codex_history_loaded_at_ms = Some(now_ms());
                                        ui.sync_codex_history_selection();
                                        let count = ui.codex_history_sessions.len();
                                        let zh = format!("history: 已加载 {count} 个会话");
                                        let en = format!("history: loaded {count} sessions");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en).to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                    Err(e) => {
                                        ui.codex_history_sessions.clear();
                                        if let Some(focus) = ui.codex_history_external_focus.as_ref() {
                                            merge_codex_history_external_focus(&mut ui.codex_history_sessions, focus);
                                        }
                                        ui.codex_history_loaded_at_ms = Some(now_ms());
                                        ui.codex_history_error = Some(e.to_string());
                                        ui.sync_codex_history_selection();
                                        let zh = format!("history: 加载失败：{e}");
                                        let en = format!("history: load failed: {e}");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en).to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                }
                                ui.needs_codex_history_refresh = false;
                            }
                            if ui.needs_codex_recent_refresh {
                                ui.codex_recent_error = None;
                                let since = Duration::from_secs(24 * 60 * 60);
                                match crate::sessions::find_recent_codex_sessions(since, 500).await {
                                    Ok(list) => {
                                        let mut rows = Vec::with_capacity(list.len());
                                        for s in list {
                                            let cwd_opt = s.cwd.clone();
                                            let cwd = cwd_opt.as_deref().unwrap_or("-");
                                            let root = if ui.codex_recent_raw_cwd {
                                                cwd.to_string()
                                            } else {
                                                crate::sessions::infer_project_root_from_cwd(cwd)
                                            };
                                            let branch = if root.trim().is_empty()
                                                || root == "-"
                                                || !std::path::Path::new(&root).exists()
                                            {
                                                None
                                            } else if let Some(v) = ui.codex_recent_branch_cache.get(&root) {
                                                v.clone()
                                            } else {
                                                let v = read_git_branch_shallow(&root).await;
                                                ui.codex_recent_branch_cache
                                                    .insert(root.clone(), v.clone());
                                                v
                                            };
                                            rows.push(crate::tui::state::RecentCodexRow {
                                                root,
                                                branch,
                                                session_id: s.id,
                                                cwd: cwd_opt,
                                                mtime_ms: s.mtime_ms,
                                            });
                                        }
                                        ui.codex_recent_rows = rows;
                                        ui.codex_recent_loaded_at_ms = Some(now_ms());
                                        ui.codex_recent_selected_idx = 0;
                                        ui.codex_recent_selected_id = ui
                                            .codex_recent_rows
                                            .first()
                                            .map(|r| r.session_id.clone());
                                        ui.codex_recent_table.select(if ui.codex_recent_rows.is_empty() {
                                            None
                                        } else {
                                            Some(0)
                                        });
                                        let count = ui.codex_recent_rows.len();
                                        let zh = format!("recent: 已加载 {count} 个会话");
                                        let en = format!("recent: loaded {count} sessions");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en)
                                                .to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                    Err(e) => {
                                        ui.codex_recent_rows.clear();
                                        ui.codex_recent_loaded_at_ms = Some(now_ms());
                                        ui.codex_recent_error = Some(e.to_string());
                                        let zh = format!("recent: 加载失败：{e}");
                                        let en = format!("recent: load failed: {e}");
                                        ui.toast = Some((
                                            crate::tui::i18n::pick(ui.language, &zh, &en)
                                                .to_string(),
                                            Instant::now(),
                                        ));
                                    }
                                }
                                ui.needs_codex_recent_refresh = false;
                            }
                            let after_surface = RenderSurfaceKey::capture(&ui);
                            if before_surface != after_surface {
                                request_full_clear(&mut render_invalidation);
                            } else {
                                request_redraw(&mut render_invalidation);
                            }
                        }
                    }
                    Event::Resize(_, _) => {
                        ui.reset_table_viewports();
                        request_full_clear(&mut render_invalidation);
                    }
                    _ => {}
                }
            }
        }
    }

    terminal.show_cursor()?;
    crossterm::terminal::disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    term_guard.disarm();
    Ok(())
}

async fn read_git_branch_shallow(workdir: &str) -> Option<String> {
    use tokio::fs;

    let root = std::path::PathBuf::from(workdir);
    if !root.is_absolute() {
        return None;
    }

    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return None;
    }

    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let content = fs::read_to_string(&dot_git).await.ok()?;
        let first = content.lines().next()?.trim();
        let path = first.strip_prefix("gitdir:")?.trim();
        let mut p = std::path::PathBuf::from(path);
        if p.is_relative() {
            p = root.join(p);
        }
        p
    };

    let head = fs::read_to_string(gitdir.join("HEAD")).await.ok()?;
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

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Url;
use tokio::sync::{OnceCell, Semaphore};

use crate::config::{UpstreamConfig, load_config, proxy_home_dir, save_config};
use crate::state::{ConfigHealth, ProxyState, UpstreamHealth};

use super::model::{ProviderOption, Snapshot, filtered_requests_len, now_ms};
use super::report::build_stats_report;
use super::state::{UiState, adjust_table_selection};
use super::types::{EffortChoice, Focus, Overlay, Page, StatsFocus};

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) async fn handle_key_event(
    state: Arc<ProxyState>,
    providers: &[ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if ui.overlay == Overlay::None && apply_page_shortcuts(ui, key.code) {
        return true;
    }

    match ui.overlay {
        Overlay::None => handle_key_normal(&state, providers, ui, snapshot, key).await,
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ui.overlay = Overlay::None;
                true
            }
            _ => false,
        },
        Overlay::EffortMenu => handle_key_effort_menu(&state, ui, snapshot, key).await,
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(&state, providers, ui, snapshot, key).await
        }
    }
}

fn apply_page_shortcuts(ui: &mut UiState, code: KeyCode) -> bool {
    let page = match code {
        KeyCode::Char('1') => Some(Page::Dashboard),
        KeyCode::Char('2') => Some(Page::Configs),
        KeyCode::Char('3') => Some(Page::Sessions),
        KeyCode::Char('4') => Some(Page::Requests),
        KeyCode::Char('5') => Some(Page::Stats),
        KeyCode::Char('6') => Some(Page::Settings),
        _ => None,
    };
    if let Some(p) = page {
        ui.page = p;
        if ui.page == Page::Configs {
            ui.focus = Focus::Configs;
        } else if ui.page == Page::Requests {
            ui.focus = Focus::Requests;
        } else if ui.page == Page::Sessions {
            ui.focus = Focus::Sessions;
        } else if ui.page == Page::Dashboard && ui.focus == Focus::Configs {
            ui.focus = Focus::Sessions;
        }
        return true;
    }
    false
}

fn apply_selected_session(ui: &mut UiState, snapshot: &Snapshot, idx: usize) {
    ui.selected_session_idx = idx.min(snapshot.rows.len().saturating_sub(1));
    ui.selected_session_id = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.clone());

    ui.sessions_table.select(if snapshot.rows.is_empty() {
        None
    } else {
        Some(ui.selected_session_idx)
    });

    ui.selected_request_idx = 0;
    let req_len = filtered_requests_len(snapshot, ui.selected_session_idx);
    ui.requests_table
        .select(if req_len == 0 { None } else { Some(0) });
}

async fn apply_effort_override(state: &ProxyState, sid: String, effort: Option<String>) {
    let now = now_ms();
    if let Some(eff) = effort {
        state.set_session_effort_override(sid, eff, now).await;
    } else {
        state.clear_session_effort_override(&sid).await;
    }
}

async fn apply_session_provider_override(state: &ProxyState, sid: String, cfg: Option<String>) {
    let now = now_ms();
    if let Some(cfg) = cfg {
        state.set_session_config_override(sid, cfg, now).await;
    } else {
        state.clear_session_config_override(&sid).await;
    }
}

async fn apply_global_provider_override(state: &ProxyState, cfg: Option<String>) {
    if let Some(cfg) = cfg {
        state.set_global_config_override(cfg).await;
    } else {
        state.clear_global_config_override().await;
    }
}

async fn persist_config_meta(
    ui: &UiState,
    config_name: &str,
    enabled: Option<bool>,
    level: Option<u8>,
) -> anyhow::Result<()> {
    let mut cfg = load_config().await?;
    let mgr = if ui.service_name == "claude" {
        &mut cfg.claude
    } else {
        &mut cfg.codex
    };
    let Some(svc) = mgr.configs.get_mut(config_name) else {
        anyhow::bail!("config '{config_name}' not found");
    };
    if let Some(enabled) = enabled {
        svc.enabled = enabled;
    }
    if let Some(level) = level {
        svc.level = level.clamp(1, 10);
    }
    save_config(&cfg).await?;
    Ok(())
}

fn shorten_err(err: &str, max: usize) -> String {
    if err.chars().count() <= max {
        return err.to_string();
    }
    err.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

fn health_check_timeout() -> Duration {
    let ms = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_500)
        .clamp(300, 20_000);
    Duration::from_millis(ms)
}

fn health_check_upstream_concurrency() -> usize {
    std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_UPSTREAM_CONCURRENCY")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4)
        .min(32)
}

fn health_check_max_inflight_configs() -> usize {
    std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_MAX_INFLIGHT")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2)
        .min(16)
}

fn health_check_config_semaphore() -> &'static OnceCell<Arc<Semaphore>> {
    static SEM: OnceCell<Arc<Semaphore>> = OnceCell::const_new();
    &SEM
}

fn health_check_url(base_url: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(base_url)?;
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url.join("models")?)
}

async fn probe_upstream(client: &reqwest::Client, upstream: &UpstreamConfig) -> UpstreamHealth {
    let mut out = UpstreamHealth {
        base_url: upstream.base_url.clone(),
        ..UpstreamHealth::default()
    };

    let url = match health_check_url(&upstream.base_url) {
        Ok(u) => u,
        Err(e) => {
            out.ok = Some(false);
            out.error = Some(shorten_err(&format!("invalid base_url: {e}"), 140));
            return out;
        }
    };

    let start = Instant::now();
    let mut req = client.get(url).header("Accept", "application/json");
    if let Some(token) = upstream.auth.resolve_auth_token() {
        req = req.header("Authorization", format!("Bearer {}", token));
    } else if let Some(key) = upstream.auth.resolve_api_key() {
        req = req.header("X-API-Key", key);
    }

    match req.send().await {
        Ok(resp) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.status_code = Some(resp.status().as_u16());
            out.ok = Some(resp.status().is_success());
            if !resp.status().is_success() {
                out.error = Some(shorten_err(&format!("HTTP {}", resp.status()), 140));
            }
        }
        Err(e) => {
            out.latency_ms = Some(start.elapsed().as_millis() as u64);
            out.ok = Some(false);
            out.error = Some(shorten_err(&e.to_string(), 140));
        }
    }
    out
}

async fn load_upstreams_for_config(
    service_name: &str,
    config_name: &str,
) -> anyhow::Result<Vec<UpstreamConfig>> {
    let cfg = load_config().await?;
    let mgr = if service_name == "claude" {
        &cfg.claude
    } else {
        &cfg.codex
    };
    let Some(svc) = mgr.configs.get(config_name) else {
        anyhow::bail!("config '{config_name}' not found");
    };
    Ok(svc.upstreams.clone())
}

async fn run_health_check_for_config(
    state: Arc<ProxyState>,
    service_name: &'static str,
    config_name: String,
    upstreams: Vec<UpstreamConfig>,
) {
    let timeout = health_check_timeout();
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let now = now_ms();
            state
                .record_health_check_result(
                    service_name,
                    &config_name,
                    now,
                    UpstreamHealth {
                        base_url: "<client>".to_string(),
                        ok: Some(false),
                        status_code: None,
                        latency_ms: None,
                        error: Some(shorten_err(&err.to_string(), 140)),
                    },
                )
                .await;
            state
                .finish_health_check(service_name, &config_name, now, false)
                .await;
            return;
        }
    };

    let upstream_conc = health_check_upstream_concurrency();
    let sem = Arc::new(Semaphore::new(upstream_conc));
    let mut futs = FuturesUnordered::new();
    for upstream in upstreams {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        futs.push(async move {
            let _permit = sem.acquire().await;
            probe_upstream(&client, &upstream).await
        });
    }

    let mut canceled = false;
    while let Some(up) = futs.next().await {
        let now = now_ms();
        state
            .record_health_check_result(service_name, &config_name, now, up)
            .await;
        if state
            .is_health_check_cancel_requested(service_name, &config_name)
            .await
        {
            canceled = true;
            break;
        }
    }

    let now = now_ms();
    state
        .finish_health_check(service_name, &config_name, now, canceled)
        .await;
}

fn reports_dir() -> std::path::PathBuf {
    proxy_home_dir().join("reports")
}

fn write_report(report: &str, now_ms: u64) -> anyhow::Result<std::path::PathBuf> {
    let dir = reports_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("tui_stats_report.{now_ms}.txt"));
    std::fs::write(&path, report.as_bytes())?;
    Ok(path)
}

fn try_copy_to_clipboard(report: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run(mut cmd: Command, report: &str) -> anyhow::Result<()> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        {
            let Some(mut stdin) = child.stdin.take() else {
                anyhow::bail!("no stdin");
            };
            stdin.write_all(report.as_bytes())?;
        }
        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("clipboard command failed")
        }
    }

    #[cfg(target_os = "macos")]
    {
        return run(Command::new("pbcopy"), report);
    }

    #[cfg(target_os = "windows")]
    {
        return run(Command::new("cmd").args(["/C", "clip"]), report);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(()) = run(Command::new("wl-copy"), report) {
            return Ok(());
        }
        run(
            Command::new("xclip").args(["-selection", "clipboard"]),
            report,
        )
    }
}

async fn handle_key_normal(
    state: &Arc<ProxyState>,
    providers: &[ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        ui.should_exit = true;
        return true;
    }

    match key.code {
        KeyCode::Char('q') => {
            ui.should_exit = true;
            true
        }
        KeyCode::Char('?') => {
            ui.overlay = Overlay::Help;
            true
        }
        KeyCode::Tab => {
            if ui.page == Page::Dashboard {
                ui.focus = match ui.focus {
                    Focus::Sessions => Focus::Requests,
                    Focus::Requests => Focus::Sessions,
                    Focus::Configs => Focus::Sessions,
                };
            } else if ui.page == Page::Configs {
                ui.focus = Focus::Configs;
            } else if ui.page == Page::Stats {
                ui.stats_focus = match ui.stats_focus {
                    StatsFocus::Configs => StatsFocus::Providers,
                    StatsFocus::Providers => StatsFocus::Configs,
                };
                ui.toast = Some((
                    format!(
                        "stats focus: {}",
                        match ui.stats_focus {
                            StatsFocus::Configs => "configs",
                            StatsFocus::Providers => "providers",
                        }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Configs => {
            if let Some(next) = adjust_table_selection(&mut ui.configs_table, -1, providers.len()) {
                ui.selected_config_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Configs => {
            if let Some(next) = adjust_table_selection(&mut ui.configs_table, 1, providers.len()) {
                ui.selected_config_idx = next;
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Stats => {
            let (table, len) = match ui.stats_focus {
                StatsFocus::Configs => (
                    &mut ui.stats_configs_table,
                    snapshot.usage_rollup.by_config.len(),
                ),
                StatsFocus::Providers => (
                    &mut ui.stats_providers_table,
                    snapshot.usage_rollup.by_provider.len(),
                ),
            };
            if let Some(next) = adjust_table_selection(table, -1, len) {
                match ui.stats_focus {
                    StatsFocus::Configs => ui.selected_stats_config_idx = next,
                    StatsFocus::Providers => ui.selected_stats_provider_idx = next,
                }
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Stats => {
            let (table, len) = match ui.stats_focus {
                StatsFocus::Configs => (
                    &mut ui.stats_configs_table,
                    snapshot.usage_rollup.by_config.len(),
                ),
                StatsFocus::Providers => (
                    &mut ui.stats_providers_table,
                    snapshot.usage_rollup.by_provider.len(),
                ),
            };
            if let Some(next) = adjust_table_selection(table, 1, len) {
                match ui.stats_focus {
                    StatsFocus::Configs => ui.selected_stats_config_idx = next,
                    StatsFocus::Providers => ui.selected_stats_provider_idx = next,
                }
                return true;
            }
            false
        }
        KeyCode::Char('d') if ui.page == Page::Stats => {
            let options = [7usize, 21usize, 60usize];
            let idx = options
                .iter()
                .position(|&n| n == ui.stats_days)
                .unwrap_or(1);
            let next = options[(idx + 1) % options.len()];
            ui.stats_days = next;
            ui.needs_snapshot_refresh = true;
            ui.toast = Some((format!("stats days: {next}"), Instant::now()));
            true
        }
        KeyCode::Char('e') if ui.page == Page::Stats => {
            ui.stats_errors_only = !ui.stats_errors_only;
            ui.toast = Some((
                format!("stats: errors_only={}", ui.stats_errors_only),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('y') if ui.page == Page::Stats => {
            let now = now_ms();
            let Some(report) = build_stats_report(ui, snapshot, now) else {
                ui.toast = Some(("stats report: no selection".to_string(), Instant::now()));
                return true;
            };
            let saved = write_report(&report, now);
            let copied = try_copy_to_clipboard(&report);

            match (saved, copied) {
                (Ok(path), Ok(())) => {
                    ui.toast = Some((
                        format!("stats report: copied + saved {}", path.display()),
                        Instant::now(),
                    ));
                }
                (Ok(path), Err(err)) => {
                    ui.toast = Some((
                        format!(
                            "stats report: saved {} (copy failed: {err})",
                            path.display()
                        ),
                        Instant::now(),
                    ));
                }
                (Err(err), Ok(())) => {
                    ui.toast = Some((
                        format!("stats report: copied (save failed: {err})"),
                        Instant::now(),
                    ));
                }
                (Err(err1), Err(err2)) => {
                    ui.toast = Some((
                        format!("stats report: copy failed: {err2} (save failed: {err1})"),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Enter if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            apply_global_provider_override(state, Some(pvd.name.clone())).await;
            ui.toast = Some((format!("global cfg override: {}", pvd.name), Instant::now()));
            true
        }
        KeyCode::Backspace | KeyCode::Delete if ui.page == Page::Configs => {
            apply_global_provider_override(state, None).await;
            ui.toast = Some(("global cfg override: <clear>".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('o') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session cfg override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, Some(pvd.name.clone())).await;
            ui.toast = Some((
                format!("session cfg override: {}", pvd.name),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('O') if ui.page == Page::Configs => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.toast = Some((
                    "session cfg override: <no session>".to_string(),
                    Instant::now(),
                ));
                return true;
            };
            apply_session_provider_override(state, sid, None).await;
            ui.toast = Some(("session cfg override: <clear>".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('t') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let (enabled_ovr, _) = snapshot
                .config_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = enabled_ovr.unwrap_or(pvd.enabled);
            let next = !current;
            let now = now_ms();
            state
                .set_config_enabled_override(ui.service_name, pvd.name.clone(), next, now)
                .await;

            if let Err(err) = persist_config_meta(ui, &pvd.name, Some(next), None).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((
                    format!(
                        "config {} enabled={}",
                        pvd.name,
                        if next { "true" } else { "false" }
                    ),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('+') | KeyCode::Char('=') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let (_, level_ovr) = snapshot
                .config_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = level_ovr.unwrap_or(pvd.level).clamp(1, 10);
            let next = (current + 1).min(10);
            let now = now_ms();
            state
                .set_config_level_override(ui.service_name, pvd.name.clone(), next, now)
                .await;
            if let Err(err) = persist_config_meta(ui, &pvd.name, None, Some(next)).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((format!("config {} level={next}", pvd.name), Instant::now()));
            }
            true
        }
        KeyCode::Char('-') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let (_, level_ovr) = snapshot
                .config_meta_overrides
                .get(pvd.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let current = level_ovr.unwrap_or(pvd.level).clamp(1, 10);
            let next = current.saturating_sub(1).max(1);
            let now = now_ms();
            state
                .set_config_level_override(ui.service_name, pvd.name.clone(), next, now)
                .await;
            if let Err(err) = persist_config_meta(ui, &pvd.name, None, Some(next)).await {
                ui.toast = Some((format!("save failed: {err}"), Instant::now()));
            } else {
                ui.toast = Some((format!("config {} level={next}", pvd.name), Instant::now()));
            }
            true
        }
        KeyCode::Char('h') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let service_name = ui.service_name;
            let config_name = pvd.name.clone();

            let upstreams = match load_upstreams_for_config(service_name, &config_name).await {
                Ok(v) => v,
                Err(err) => {
                    ui.toast = Some((format!("health check load failed: {err}"), Instant::now()));
                    return true;
                }
            };

            let now = now_ms();
            if !state
                .try_begin_health_check(service_name, &config_name, upstreams.len(), now)
                .await
            {
                ui.toast = Some((
                    format!("health check already running: {config_name}"),
                    Instant::now(),
                ));
                return true;
            }

            state
                .record_config_health(
                    service_name,
                    config_name.clone(),
                    ConfigHealth {
                        checked_at_ms: now,
                        upstreams: Vec::new(),
                    },
                )
                .await;

            let state = Arc::clone(state);
            ui.toast = Some((
                format!("health check queued: {config_name}"),
                Instant::now(),
            ));
            let upstreams_for_task = upstreams;
            tokio::spawn(async move {
                let sem = health_check_config_semaphore()
                    .get_or_init(|| async {
                        Arc::new(Semaphore::new(health_check_max_inflight_configs()))
                    })
                    .await;
                let _permit = sem.clone().acquire_owned().await;
                run_health_check_for_config(state, service_name, config_name, upstreams_for_task)
                    .await;
            });
            true
        }
        KeyCode::Char('H') if ui.page == Page::Configs => {
            let service_name = ui.service_name;
            let configs = providers.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
            let state = Arc::clone(state);
            ui.toast = Some((
                format!("health check queued: {} configs", configs.len()),
                Instant::now(),
            ));
            tokio::spawn(async move {
                let sem = health_check_config_semaphore()
                    .get_or_init(|| async {
                        Arc::new(Semaphore::new(health_check_max_inflight_configs()))
                    })
                    .await
                    .clone();

                let cfg = match load_config().await {
                    Ok(c) => c,
                    Err(err) => {
                        let now = now_ms();
                        for config_name in configs {
                            state
                                .try_begin_health_check(service_name, &config_name, 1, now)
                                .await;
                            state
                                .record_health_check_result(
                                    service_name,
                                    &config_name,
                                    now,
                                    UpstreamHealth {
                                        base_url: "<load_config>".to_string(),
                                        ok: Some(false),
                                        status_code: None,
                                        latency_ms: None,
                                        error: Some(shorten_err(&err.to_string(), 140)),
                                    },
                                )
                                .await;
                            state
                                .finish_health_check(service_name, &config_name, now, false)
                                .await;
                        }
                        return;
                    }
                };

                let mgr = if service_name == "claude" {
                    &cfg.claude
                } else {
                    &cfg.codex
                };
                for config_name in configs {
                    let Some(svc) = mgr.configs.get(&config_name) else {
                        continue;
                    };
                    let upstreams = svc.upstreams.clone();
                    let now = now_ms();
                    if !state
                        .try_begin_health_check(service_name, &config_name, upstreams.len(), now)
                        .await
                    {
                        continue;
                    }
                    state
                        .record_config_health(
                            service_name,
                            config_name.clone(),
                            ConfigHealth {
                                checked_at_ms: now,
                                upstreams: Vec::new(),
                            },
                        )
                        .await;

                    let state = Arc::clone(&state);
                    let sem = sem.clone();
                    tokio::spawn(async move {
                        let _permit = sem.acquire_owned().await;
                        run_health_check_for_config(state, service_name, config_name, upstreams)
                            .await;
                    });

                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            });
            true
        }
        KeyCode::Char('c') if ui.page == Page::Configs => {
            let Some(pvd) = providers.get(ui.selected_config_idx) else {
                return true;
            };
            let now = now_ms();
            if state
                .request_cancel_health_check(ui.service_name, pvd.name.as_str(), now)
                .await
            {
                ui.toast = Some((
                    format!("health check cancel requested: {}", pvd.name),
                    Instant::now(),
                ));
            } else {
                ui.toast = Some((
                    format!("health check not running: {}", pvd.name),
                    Instant::now(),
                ));
            }
            true
        }
        KeyCode::Char('C') if ui.page == Page::Configs => {
            let now = now_ms();
            let mut count = 0usize;
            for p in providers {
                if state
                    .request_cancel_health_check(ui.service_name, p.name.as_str(), now)
                    .await
                {
                    count += 1;
                }
            }
            ui.toast = Some((
                format!("health check cancel requested: {count} configs"),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('a') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = !ui.sessions_page_active_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: active_only={}",
                    ui.sessions_page_active_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('e') if ui.page == Page::Sessions => {
            ui.sessions_page_errors_only = !ui.sessions_page_errors_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: errors_only={}",
                    ui.sessions_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('v') if ui.page == Page::Sessions => {
            ui.sessions_page_overrides_only = !ui.sessions_page_overrides_only;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some((
                format!(
                    "sessions filter: overrides_only={}",
                    ui.sessions_page_overrides_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('r') if ui.page == Page::Sessions => {
            ui.sessions_page_active_only = false;
            ui.sessions_page_errors_only = false;
            ui.sessions_page_overrides_only = false;
            ui.selected_sessions_page_idx = 0;
            ui.toast = Some(("sessions filter: reset".to_string(), Instant::now()));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Sessions => {
            let filtered = snapshot
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    if ui.sessions_page_active_only && row.active_count == 0 {
                        return false;
                    }
                    if ui.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                        return false;
                    }
                    if ui.sessions_page_overrides_only
                        && row.override_effort.is_none()
                        && row.override_config_name.is_none()
                    {
                        return false;
                    }
                    true
                })
                .take(200)
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();

            let len = filtered.len();
            if let Some(next) = adjust_table_selection(&mut ui.sessions_page_table, -1, len) {
                ui.selected_sessions_page_idx = next;
                if let Some(&row_idx) = filtered.get(next) {
                    apply_selected_session(ui, snapshot, row_idx);
                }
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Sessions => {
            let filtered = snapshot
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    if ui.sessions_page_active_only && row.active_count == 0 {
                        return false;
                    }
                    if ui.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                        return false;
                    }
                    if ui.sessions_page_overrides_only
                        && row.override_effort.is_none()
                        && row.override_config_name.is_none()
                    {
                        return false;
                    }
                    true
                })
                .take(200)
                .map(|(idx, _)| idx)
                .collect::<Vec<_>>();

            let len = filtered.len();
            if let Some(next) = adjust_table_selection(&mut ui.sessions_page_table, 1, len) {
                ui.selected_sessions_page_idx = next;
                if let Some(&row_idx) = filtered.get(next) {
                    apply_selected_session(ui, snapshot, row_idx);
                }
                return true;
            }
            false
        }
        KeyCode::Char('e') if ui.page == Page::Requests => {
            ui.request_page_errors_only = !ui.request_page_errors_only;
            ui.toast = Some((
                format!(
                    "requests filter: errors_only={}",
                    ui.request_page_errors_only
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('s') if ui.page == Page::Requests => {
            ui.request_page_scope_session = !ui.request_page_scope_session;
            ui.toast = Some((
                format!(
                    "requests scope: {}",
                    if ui.request_page_scope_session {
                        "selected session"
                    } else {
                        "all"
                    }
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Up | KeyCode::Char('k') if ui.page == Page::Requests => {
            let selected_sid = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.as_deref());
            let filtered_len = snapshot
                .recent
                .iter()
                .filter(|r| {
                    if ui.request_page_errors_only && r.status_code < 400 {
                        return false;
                    }
                    if ui.request_page_scope_session {
                        match (selected_sid, r.session_id.as_deref()) {
                            (Some(sid), Some(rid)) => sid == rid,
                            (Some(_), None) => false,
                            (None, _) => true,
                        }
                    } else {
                        true
                    }
                })
                .count();
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, -1, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') if ui.page == Page::Requests => {
            let selected_sid = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.as_deref());
            let filtered_len = snapshot
                .recent
                .iter()
                .filter(|r| {
                    if ui.request_page_errors_only && r.status_code < 400 {
                        return false;
                    }
                    if ui.request_page_scope_session {
                        match (selected_sid, r.session_id.as_deref()) {
                            (Some(sid), Some(rid)) => sid == rid,
                            (Some(_), None) => false,
                            (None, _) => true,
                        }
                    } else {
                        true
                    }
                })
                .count();
            if let Some(next) = adjust_table_selection(&mut ui.request_page_table, 1, filtered_len)
            {
                ui.selected_request_page_idx = next;
                return true;
            }
            false
        }
        KeyCode::Up | KeyCode::Char('k') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, -1, snapshot.rows.len())
                {
                    apply_selected_session(ui, snapshot, next);
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, -1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Configs => false,
        },
        KeyCode::Down | KeyCode::Char('j') => match ui.focus {
            Focus::Sessions => {
                if let Some(next) =
                    adjust_table_selection(&mut ui.sessions_table, 1, snapshot.rows.len())
                {
                    apply_selected_session(ui, snapshot, next);
                    return true;
                }
                false
            }
            Focus::Requests => {
                let filtered_len = filtered_requests_len(snapshot, ui.selected_session_idx);
                if let Some(next) = adjust_table_selection(&mut ui.requests_table, 1, filtered_len)
                {
                    ui.selected_request_idx = next;
                    return true;
                }
                false
            }
            Focus::Configs => false,
        },
        KeyCode::Enter => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            if snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.as_deref())
                .is_none()
            {
                return false;
            }

            ui.overlay = Overlay::EffortMenu;
            let current = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.override_effort.as_deref())
                .unwrap_or("");
            ui.effort_menu_idx = match current {
                "low" => 1,
                "medium" => 2,
                "high" => 3,
                "xhigh" => 4,
                _ => 0,
            };
            true
        }
        KeyCode::Char('l') | KeyCode::Char('m') | KeyCode::Char('h') | KeyCode::Char('X') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let eff = match key.code {
                KeyCode::Char('l') => Some("low"),
                KeyCode::Char('m') => Some("medium"),
                KeyCode::Char('h') => Some("high"),
                KeyCode::Char('X') => Some("xhigh"),
                _ => None,
            }
            .map(|s| s.to_string());
            apply_effort_override(state, sid, eff.clone()).await;
            ui.toast = Some((
                format!("effort override: {}", eff.as_deref().unwrap_or("<clear>")),
                Instant::now(),
            ));
            true
        }
        KeyCode::Char('x') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            apply_effort_override(state, sid, None).await;
            ui.toast = Some(("effort override cleared".to_string(), Instant::now()));
            true
        }
        KeyCode::Char('p') => {
            if ui.focus != Focus::Sessions {
                return false;
            }
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                return false;
            };
            let current = snapshot
                .config_overrides
                .get(&sid)
                .map(|s| s.as_str())
                .unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuSession;
            true
        }
        KeyCode::Char('P') => {
            let current = snapshot.global_override.as_deref().unwrap_or("");
            ui.provider_menu_idx = providers
                .iter()
                .position(|p| p.name == current)
                .map(|i| i + 1)
                .unwrap_or(0);
            ui.overlay = Overlay::ProviderMenuGlobal;
            true
        }
        _ => false,
    }
}

async fn handle_key_effort_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.effort_menu_idx = ui.effort_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.effort_menu_idx = (ui.effort_menu_idx + 1).min(4);
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let choice = match ui.effort_menu_idx {
                1 => EffortChoice::Low,
                2 => EffortChoice::Medium,
                3 => EffortChoice::High,
                4 => EffortChoice::XHigh,
                _ => EffortChoice::Clear,
            };
            apply_effort_override(state, sid, choice.value().map(|s| s.to_string())).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((format!("effort set: {}", choice.label()), Instant::now()));
            true
        }
        _ => false,
    }
}

async fn handle_key_provider_menu(
    state: &ProxyState,
    providers: &[ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.provider_menu_idx = ui.provider_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = providers.len();
            ui.provider_menu_idx = (ui.provider_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            let idx = ui.provider_menu_idx;
            let chosen = if idx == 0 {
                None
            } else {
                providers.get(idx - 1).map(|p| p.name.clone())
            };

            match ui.overlay {
                Overlay::ProviderMenuGlobal => {
                    apply_global_provider_override(state, chosen.clone()).await;
                    ui.toast = Some((
                        format!(
                            "global cfg override: {}",
                            chosen.as_deref().unwrap_or("<clear>")
                        ),
                        Instant::now(),
                    ));
                }
                Overlay::ProviderMenuSession => {
                    let Some(sid) = snapshot
                        .rows
                        .get(ui.selected_session_idx)
                        .and_then(|r| r.session_id.clone())
                    else {
                        ui.overlay = Overlay::None;
                        return true;
                    };
                    apply_session_provider_override(state, sid, chosen.clone()).await;
                    ui.toast = Some((
                        format!(
                            "session cfg override: {}",
                            chosen.as_deref().unwrap_or("<clear>")
                        ),
                        Instant::now(),
                    ));
                }
                _ => {}
            }

            ui.overlay = Overlay::None;
            true
        }
        _ => false,
    }
}

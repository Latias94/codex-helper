use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};

use codex_helper_core::codex_switch::{self, CodexSwitchPhase, CodexSwitchStatus};

use crate::config::{
    ReasoningGuardAction, ReasoningGuardRetryExhaustedAction, ReasoningGuardStreamMode,
    RetryProfileName, RetryStrategy,
};
use crate::dashboard_core::operator_summary::{
    OperatorReasoningGuardSummary, OperatorRetryLayerSummary, OperatorRetryPolicySummary,
};
use crate::dashboard_core::{OperatorReadStatus, OperatorRetrySummary, WindowStats};
use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_rank, provider_balance_brief_lang,
    request_cache_hit_rate_label, shorten, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::max_wrapped_vertical_scroll;

fn retry_profile_name(profile: Option<RetryProfileName>) -> &'static str {
    match profile {
        Some(RetryProfileName::Balanced) => "balanced",
        Some(RetryProfileName::SameUpstream) => "same-upstream",
        Some(RetryProfileName::AggressiveFailover) => "aggressive-failover",
        Some(RetryProfileName::CostPrimary) => "cost-primary",
        None => "default",
    }
}

fn retry_strategy_name(strategy: RetryStrategy) -> &'static str {
    match strategy {
        RetryStrategy::Failover => "failover",
        RetryStrategy::SameUpstream => "same-upstream",
    }
}

fn reasoning_guard_action_name(action: ReasoningGuardAction) -> &'static str {
    match action {
        ReasoningGuardAction::Observe => "observe",
        ReasoningGuardAction::Block => "block",
        ReasoningGuardAction::Retry => "retry",
    }
}

fn reasoning_guard_stream_mode_name(mode: ReasoningGuardStreamMode) -> &'static str {
    match mode {
        ReasoningGuardStreamMode::Off => "off",
        ReasoningGuardStreamMode::Observe => "observe",
        ReasoningGuardStreamMode::StrictBuffer => "strict-buffer",
    }
}

fn reasoning_guard_exhausted_name(action: ReasoningGuardRetryExhaustedAction) -> &'static str {
    match action {
        ReasoningGuardRetryExhaustedAction::Pass => "pass",
        ReasoningGuardRetryExhaustedAction::Block => "block",
    }
}

fn string_list(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(",")
    }
}

fn integer_list(values: &[i64]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn value_or_dash(value: &str) -> &str {
    if value.trim().is_empty() { "-" } else { value }
}

fn retry_layer_lines(
    label: &str,
    retry_on_label: &str,
    layer: &OperatorRetryLayerSummary,
) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "  {label}: strategy={} attempts={} backoff={}..{}ms jitter={}ms",
            retry_strategy_name(layer.strategy),
            layer.max_attempts,
            layer.backoff_ms,
            layer.backoff_max_ms,
            layer.jitter_ms
        )),
        Line::from(format!(
            "    {retry_on_label}: status=[{}] class=[{}]",
            value_or_dash(&layer.on_status),
            string_list(&layer.on_class)
        )),
    ]
}

fn reasoning_guard_line(
    guard: &OperatorReasoningGuardSummary,
    language: Language,
) -> Line<'static> {
    let (label, enabled, disabled) = match language {
        Language::Zh => ("推理护栏", "开", "关"),
        Language::En => ("reasoning_guard", "on", "off"),
    };
    Line::from(format!(
        "  {label}: {} tokens=[{}] boundary={} action={} stream={} retries={} exhausted={} paths=[{}] log={}",
        if guard.enabled { enabled } else { disabled },
        integer_list(&guard.reasoning_equals),
        guard.boundary_sequence_max_n,
        reasoning_guard_action_name(guard.action),
        reasoning_guard_stream_mode_name(guard.stream_mode),
        guard.max_guard_retries,
        reasoning_guard_exhausted_name(guard.on_retry_exhausted),
        string_list(&guard.paths),
        if guard.log_matches { enabled } else { disabled }
    ))
}

fn retry_policy_lines(
    policy: &OperatorRetryPolicySummary,
    language: Language,
) -> Vec<Line<'static>> {
    let (upstream, provider, retry_on, never, cooldown) = match language {
        Language::Zh => ("上游", "提供商", "重试条件", "禁止", "冷却"),
        Language::En => ("upstream", "provider", "retry_on", "never", "cooldown"),
    };
    let mut lines = retry_layer_lines(upstream, retry_on, &policy.upstream);
    lines.extend(retry_layer_lines(provider, retry_on, &policy.provider));
    lines.push(Line::from(format!(
        "  {never}: status=[{}] class=[{}]",
        value_or_dash(&policy.never_on_status),
        string_list(&policy.never_on_class)
    )));
    lines.push(Line::from(format!(
        "  {cooldown}: transport={}s cf_challenge={}s cf_timeout={}s factor={} max={}s",
        policy.transport_cooldown_secs,
        policy.cloudflare_challenge_cooldown_secs,
        policy.cloudflare_timeout_cooldown_secs,
        policy.cooldown_backoff_factor,
        policy.cooldown_backoff_max_secs
    )));
    lines.push(reasoning_guard_line(&policy.reasoning_guard, language));
    lines
}

fn read_status_label(status: OperatorReadStatus, language: Language) -> &'static str {
    match (status, language) {
        (OperatorReadStatus::Ready, Language::Zh) => "就绪",
        (OperatorReadStatus::Stale, Language::Zh) => "陈旧",
        (OperatorReadStatus::Disconnected, Language::Zh) => "离线",
        (OperatorReadStatus::AuthRequired, Language::Zh) => "需要认证",
        (OperatorReadStatus::Ready, Language::En) => "ready",
        (OperatorReadStatus::Stale, Language::En) => "stale",
        (OperatorReadStatus::Disconnected, Language::En) => "offline",
        (OperatorReadStatus::AuthRequired, Language::En) => "auth required",
    }
}

fn codex_switch_status_lines(
    p: Palette,
    language: Language,
    status: &CodexSwitchStatus,
) -> Vec<Line<'static>> {
    let yes_no = |value| match (language, value) {
        (Language::Zh, true) => "是",
        (Language::Zh, false) => "否",
        (Language::En, true) => "yes",
        (Language::En, false) => "no",
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("  phase: ", Style::default().fg(p.muted)),
        Span::styled(
            status.phase.as_str(),
            Style::default().fg(if status.phase == CodexSwitchPhase::RecoveryRequired {
                p.warn
            } else {
                p.text
            }),
        ),
        Span::styled("  enabled: ", Style::default().fg(p.muted)),
        Span::styled(yes_no(status.enabled), Style::default().fg(p.text)),
        Span::styled("  managed: ", Style::default().fg(p.muted)),
        Span::styled(yes_no(status.managed), Style::default().fg(p.text)),
    ])];
    lines.push(Line::from(vec![
        Span::styled("  base_url: ", Style::default().fg(p.muted)),
        Span::styled(
            status.base_url.as_deref().unwrap_or("-").to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    if let Some(client_patch) = status.client_patch.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("  preset: ", Style::default().fg(p.muted)),
            Span::styled(client_patch.preset.to_string(), Style::default().fg(p.text)),
            Span::styled("  compaction: ", Style::default().fg(p.muted)),
            Span::styled(
                client_patch.compaction.to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  websocket: ", Style::default().fg(p.muted)),
            Span::styled(
                yes_no(client_patch.responses_websocket),
                Style::default().fg(p.text),
            ),
            Span::styled("  translate_models: ", Style::default().fg(p.muted)),
            Span::styled(
                yes_no(client_patch.translate_models),
                Style::default().fg(p.text),
            ),
            Span::styled("  hosted_image: ", Style::default().fg(p.muted)),
            Span::styled(
                client_patch.hosted_image_generation.to_string(),
                Style::default().fg(p.text),
            ),
        ]));
    }
    if let Some(reason) = status.recovery_reason.as_deref() {
        lines.push(Line::from(vec![
            Span::styled("  recovery: ", Style::default().fg(p.warn)),
            Span::styled(reason.to_string(), Style::default().fg(p.warn)),
        ]));
    }
    lines
}

fn retry_lines(retry: &OperatorRetrySummary, language: Language) -> Vec<Line<'static>> {
    let configured = match language {
        Language::Zh => format!(
            "  配置：profile={} upstream_attempts={} provider_attempts={}",
            retry_profile_name(retry.configured_profile),
            retry.upstream_max_attempts,
            retry.provider_max_attempts
        ),
        Language::En => format!(
            "  configured: profile={} upstream_attempts={} provider_attempts={}",
            retry_profile_name(retry.configured_profile),
            retry.upstream_max_attempts,
            retry.provider_max_attempts
        ),
    };
    let observed = match language {
        Language::Zh => format!(
            "  近期观测：retried={} same_provider={} cross_provider={} fast={}",
            retry.recent_retried_requests,
            retry.recent_same_provider_retries,
            retry.recent_cross_provider_failovers,
            retry.recent_fast_mode_requests
        ),
        Language::En => format!(
            "  observed: retried={} same_provider={} cross_provider={} fast={}",
            retry.recent_retried_requests,
            retry.recent_same_provider_retries,
            retry.recent_cross_provider_failovers,
            retry.recent_fast_mode_requests
        ),
    };
    let mut lines = vec![Line::from(configured)];
    if let Some(policy) = retry.policy.as_ref() {
        lines.extend(retry_policy_lines(policy, language));
    }
    lines.push(Line::from(observed));
    lines
}

fn success_percent(ok: usize, total: usize) -> String {
    if total == 0 {
        "-".to_string()
    } else {
        format!("{:.0}%", ok as f64 * 100.0 / total as f64)
    }
}

fn window_stats_line(label: &str, stats: &WindowStats) -> Line<'static> {
    Line::from(format!(
        "  {label} ok={} p95={} att={} 429={} 5xx={} n={}",
        success_percent(stats.ok_2xx, stats.total),
        stats
            .p95_ms
            .map(|value| format!("{value}ms"))
            .unwrap_or_else(|| "-".to_string()),
        stats
            .avg_attempts
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".to_string()),
        stats.err_429,
        stats.err_5xx,
        stats.total
    ))
}

fn runtime_activity_lines(
    p: Palette,
    snapshot: &Snapshot,
    language: Language,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            match language {
                Language::Zh => "运行概览",
                Language::En => "Runtime activity",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )),
        window_stats_line("5m", &snapshot.stats_5m),
        window_stats_line("1h", &snapshot.stats_1h),
    ];
    if let Some((provider, count)) = snapshot.stats_5m.top_provider.as_ref() {
        lines.push(Line::from(format!(
            "  5m top provider: {} n={count}",
            shorten(provider, 64)
        )));
    }
    if let Some((endpoint, count)) = snapshot.stats_5m.top_provider_endpoint.as_ref() {
        lines.push(Line::from(format!(
            "  5m top endpoint: {} n={count}",
            shorten(endpoint, 72)
        )));
    }
    lines
}

fn telemetry_lines(p: Palette, snapshot: &Snapshot, language: Language) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        match language {
            Language::Zh => "余额与计量",
            Language::En => "Balances and metering",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    ))];

    let mut providers = snapshot.provider_balances.iter().collect::<Vec<_>>();
    providers.sort_by(|(left_id, left_balances), (right_id, right_balances)| {
        let left_attention = left_balances
            .iter()
            .map(balance_snapshot_rank)
            .max()
            .unwrap_or_default();
        let right_attention = right_balances
            .iter()
            .map(balance_snapshot_rank)
            .max()
            .unwrap_or_default();
        right_attention
            .cmp(&left_attention)
            .then_with(|| left_id.cmp(right_id))
    });
    let sample_count = snapshot
        .provider_balances
        .values()
        .map(Vec::len)
        .sum::<usize>();
    lines.push(Line::from(match language {
        Language::Zh => format!(
            "  提供商={} 样本={} 快照年龄={}ms",
            providers.len(),
            sample_count,
            snapshot.refreshed_at.elapsed().as_millis()
        ),
        Language::En => format!(
            "  providers={} samples={} snapshot_age={}ms",
            providers.len(),
            sample_count,
            snapshot.refreshed_at.elapsed().as_millis()
        ),
    }));
    if providers.is_empty() {
        lines.push(Line::from("  -"));
    } else {
        for (provider, _) in providers {
            lines.push(Line::from(format!(
                "  {}: {}",
                shorten(provider, 32),
                provider_balance_brief_lang(&snapshot.provider_balances, provider, 88, language)
            )));
        }
    }

    let pricing_source = if snapshot.pricing_catalog.source.trim().is_empty() {
        "-"
    } else {
        snapshot.pricing_catalog.source.as_str()
    };
    lines.push(Line::from(match language {
        Language::Zh => format!(
            "  价格目录：source={} models={}",
            shorten(pricing_source, 64),
            snapshot.pricing_catalog.model_count
        ),
        Language::En => format!(
            "  pricing: source={} models={}",
            shorten(pricing_source, 64),
            snapshot.pricing_catalog.model_count
        ),
    }));
    if let Some(request) = snapshot
        .recent
        .iter()
        .max_by_key(|request| request.ended_at_ms)
    {
        lines.push(Line::from(match language {
            Language::Zh => format!(
                "  最近请求缓存命中率：{}",
                request_cache_hit_rate_label(request)
            ),
            Language::En => format!(
                "  latest request cache hit rate: {}",
                request_cache_hit_rate_label(request)
            ),
        }));
    }
    lines
}

pub(super) fn render_settings_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let mut lines = Vec::new();
    let status = ui.operator_read_model.as_ref().map(|model| model.status);

    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => "只读运行态",
            Language::En => "Read-only runtime",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            match ui.language {
                Language::Zh => "  连接：",
                Language::En => "  connection: ",
            },
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_connection.label(ui.language),
            Style::default().fg(p.text),
        ),
        Span::styled(
            match ui.language {
                Language::Zh => "  bundle：",
                Language::En => "  bundle: ",
            },
            Style::default().fg(p.muted),
        ),
        Span::styled(
            status
                .map(|status| read_status_label(status, ui.language))
                .unwrap_or("-"),
            Style::default().fg(match status {
                Some(OperatorReadStatus::Ready) => p.good,
                Some(OperatorReadStatus::Stale) => p.warn,
                Some(OperatorReadStatus::Disconnected | OperatorReadStatus::AuthRequired) => p.bad,
                None => p.muted,
            }),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  loaded_at_ms: ", Style::default().fg(p.muted)),
        Span::styled(
            ui.last_runtime_config_loaded_at_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            Style::default().fg(p.text),
        ),
        Span::styled("  source_mtime_ms: ", Style::default().fg(p.muted)),
        Span::styled(
            ui.last_runtime_config_source_mtime_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string()),
            Style::default().fg(p.text),
        ),
    ]));
    if let Some(error) = ui.runtime_status_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("  {}", shorten(error, 120)),
            Style::default().fg(p.warn),
        )));
    }

    lines.push(Line::from(""));
    lines.extend(runtime_activity_lines(p, snapshot, ui.language));

    lines.push(Line::from(""));
    lines.extend(telemetry_lines(p, snapshot, ui.language));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match ui.language {
            Language::Zh => "重试摘要",
            Language::En => "Retry summary",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    if let Some(retry) = ui.last_retry_summary.as_ref() {
        lines.extend(retry_lines(retry, ui.language));
    } else {
        lines.push(Line::from(Span::styled(
            "  -",
            Style::default().fg(p.muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        i18n::text(ui.language, msg::PROFILE_CONTROL_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::CONFIGURED_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.configured_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            i18n::text(ui.language, msg::EFFECTIVE_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.effective_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
    ]));
    let profiles = if ui.profile_options.is_empty() {
        i18n::text(ui.language, msg::NO_PROFILES).to_string()
    } else {
        shorten_middle(
            &ui.profile_options
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            110,
        )
    };
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::AVAILABLE_PROFILES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(profiles, Style::default().fg(p.text)),
    ]));
    let mut profile_actions = Vec::new();
    if ui.can_mutate_default_profile() {
        profile_actions.push("p/P");
    }
    if ui.can_reload_runtime() {
        profile_actions.push("R");
    }
    lines.push(Line::from(vec![
        Span::styled("  runtime override: ", Style::default().fg(p.muted)),
        Span::styled(
            ui.runtime_default_profile_override
                .as_deref()
                .unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            if profile_actions.is_empty() {
                match ui.language {
                    Language::Zh => "  只读".to_string(),
                    Language::En => "  read-only".to_string(),
                }
            } else {
                format!("  {}", profile_actions.join("/"))
            },
            Style::default().fg(if profile_actions.is_empty() {
                p.muted
            } else {
                p.accent
            }),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        i18n::text(ui.language, msg::TUI_OPTIONS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            i18n::language_name(ui.language),
            Style::default().fg(p.text),
        ),
        Span::styled(
            match (ui.language, ui.runtime_connection.is_remote_observer()) {
                (Language::Zh, false) => "  （L 切换语言并保存到 config.toml）",
                (Language::En, false) => "  (L changes language and saves it to config.toml)",
                (Language::Zh, true) => "  （L 仅切换当前 TUI 会话）",
                (Language::En, true) => "  (L changes this TUI session only)",
            },
            Style::default().fg(p.muted),
        ),
    ]));

    if ui.allows_local_codex_switch() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "Codex 本地 Switch",
                Language::En => "Codex Local Switch",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        match codex_switch::inspect() {
            Ok(status) => lines.extend(codex_switch_status_lines(p, ui.language, &status)),
            Err(error) => lines.push(Line::from(Span::styled(
                match ui.language {
                    Language::Zh => format!("  读取状态失败：{error}"),
                    Language::En => format!("  read status failed: {error}"),
                },
                Style::default().fg(p.warn),
            ))),
        }
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "  n/o 按配置开启/关闭；B/I/F/V/D 切换 ChatGPT/Imagegen/Official/Official Imagegen/Default preset。",
                Language::En => {
                    "  n/o apply configured patch/off; B/I/F/V/D select ChatGPT/Imagegen/Official/Official Imagegen/Default presets."
                }
            },
            Style::default().fg(p.muted),
        )));
    }

    let show_relay_diagnostics = ui.can_inspect_relay_capabilities()
        || ui.codex_relay_diagnostics.loading
        || ui.codex_relay_diagnostics.last_error.is_some()
        || ui.codex_relay_diagnostics.last_result.is_some();
    let show_relay_smoke = ui.can_run_relay_live_smoke()
        || ui.codex_relay_live_smoke.loading
        || ui.codex_relay_live_smoke.last_error.is_some()
        || ui.codex_relay_live_smoke.passed_counts().is_some();
    if show_relay_diagnostics || show_relay_smoke {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "Relay 兼容诊断",
                Language::En => "Relay compatibility diagnostics",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        if show_relay_diagnostics {
            let diagnostic = if ui.codex_relay_diagnostics.loading {
                "running".to_string()
            } else if let Some(error) = ui.codex_relay_diagnostics.last_error.as_deref() {
                format!("failed: {}", shorten(error, 90))
            } else if let Some(result) = ui.codex_relay_diagnostics.last_result.as_ref() {
                format!(
                    "{}/{} mismatches={}",
                    result.provider_id,
                    result.endpoint_id,
                    result.mismatches.len()
                )
            } else {
                "not run".to_string()
            };
            let key = if ui.can_inspect_relay_capabilities() {
                "C "
            } else {
                ""
            };
            lines.push(Line::from(format!("  {key}capabilities: {diagnostic}")));
        }
        if show_relay_smoke {
            let smoke = if ui.codex_relay_live_smoke.loading {
                "running".to_string()
            } else if let Some(error) = ui.codex_relay_live_smoke.last_error.as_deref() {
                format!("failed: {}", shorten(error, 90))
            } else if let Some((passed, total)) = ui.codex_relay_live_smoke.passed_counts() {
                format!("{passed}/{total} passed")
            } else {
                "not run".to_string()
            };
            let keys = if ui.can_run_relay_live_smoke() {
                "X compact / Y compact+image"
            } else {
                "compact / compact+image"
            };
            lines.push(Line::from(format!("  {keys}: {smoke}")));
        }
    }

    if !ui.runtime_connection.is_remote_observer() {
        let home = crate::config::proxy_home_dir();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "本机路径",
                Language::En => "Local paths",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(format!(
            "  config: {}",
            crate::config::config_file_path().display()
        )));
        lines.push(Line::from(format!("  home: {}", home.display())));
        lines.push(Line::from(format!(
            "  request log: {}",
            crate::logging::request_log_path().display()
        )));
        lines.push(Line::from(format!(
            "  database: {}",
            home.join("state/state.sqlite").display()
        )));
        lines.push(Line::from(format!(
            "  logs: {}",
            home.join("logs").display()
        )));
        lines.push(Line::from(format!(
            "  reports: {}",
            home.join("reports").display()
        )));
    }

    let block = Block::default()
        .title(Span::styled(
            format!("{}  PgUp/PgDn", i18n::text(ui.language, msg::PAGE_SETTINGS)),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let inner = block.inner(area);
    let max_scroll = max_wrapped_vertical_scroll(&lines, inner.width, inner.height);
    ui.settings_scroll = ui.settings_scroll.min(max_scroll);
    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .scroll((ui.settings_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
    if max_scroll > 0 {
        let mut scrollbar = ScrollbarState::new(usize::from(max_scroll) + 1)
            .position(usize::from(ui.settings_scroll));
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(widget, area, &mut scrollbar);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        codex_switch_status_lines, read_status_label, render_settings_page, retry_lines,
        retry_profile_name, telemetry_lines,
    };
    use crate::config::RetryProfileName;
    use crate::dashboard_core::{OperatorReadStatus, OperatorRetrySummary};
    use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot};
    use crate::tui::Language;
    use crate::tui::model::{Palette, Snapshot};
    use crate::tui::state::{RuntimeConnectionKind, UiState};
    use crate::tui::types::Page;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn buffer_text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in buffer.area.y..buffer.area.y.saturating_add(buffer.area.height) {
            for x in buffer.area.x..buffer.area.x.saturating_add(buffer.area.width) {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn lines_text(lines: &[ratatui::prelude::Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn settings_labels_preserve_explicit_read_states() {
        assert_eq!(
            read_status_label(OperatorReadStatus::Ready, Language::En),
            "ready"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::Stale, Language::En),
            "stale"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::Disconnected, Language::En),
            "offline"
        );
        assert_eq!(
            read_status_label(OperatorReadStatus::AuthRequired, Language::En),
            "auth required"
        );
    }

    #[test]
    fn remote_observer_settings_do_not_advertise_local_codex_switch() {
        let backend = TestBackend::new(120, 48);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut ui = UiState {
            page: Page::Settings,
            language: Language::En,
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };
        let snapshot = Snapshot::default();

        let frame = terminal
            .draw(|frame| {
                render_settings_page(frame, Palette::default(), &mut ui, &snapshot, frame.area());
            })
            .expect("draw attached Settings");
        let text = buffer_text(frame.buffer);

        assert!(text.contains("connection: remote observer"), "{text}");
        assert!(!text.contains("Codex Local Switch"), "{text}");
        assert!(!text.contains("n/o"), "{text}");
        assert!(!text.contains("C capabilities"), "{text}");
        assert!(!text.contains("X compact"), "{text}");
        assert!(!text.contains("Local paths"), "{text}");
        assert!(text.contains("changes this TUI session only"), "{text}");
    }

    #[test]
    fn integrated_settings_restore_operational_summary_and_local_paths() {
        let backend = TestBackend::new(140, 64);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut ui = UiState {
            page: Page::Settings,
            language: Language::En,
            runtime_connection: RuntimeConnectionKind::Integrated,
            ..UiState::default()
        };
        let mut snapshot = Snapshot::default();
        snapshot.stats_5m.total = 10;
        snapshot.stats_5m.ok_2xx = 9;
        snapshot.stats_5m.p95_ms = Some(250);
        snapshot.pricing_catalog.source = "bundled+remote".to_string();
        snapshot.pricing_catalog.model_count = 42;

        let frame = terminal
            .draw(|frame| {
                render_settings_page(frame, Palette::default(), &mut ui, &snapshot, frame.area());
            })
            .expect("draw integrated Settings");
        let text = buffer_text(frame.buffer);

        assert!(text.contains("Runtime activity"), "{text}");
        assert!(text.contains("5m ok=90% p95=250ms"), "{text}");
        assert!(text.contains("Balances and metering"), "{text}");
        assert!(text.contains("source=bundled+remote models=42"), "{text}");
        assert!(text.contains("saves it to config.toml"), "{text}");
        assert!(text.contains("Local paths"), "{text}");
        assert!(text.contains("state.sqlite"), "{text}");
    }

    #[test]
    fn local_attached_settings_keep_host_local_paths() {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut ui = UiState {
            page: Page::Settings,
            language: Language::En,
            runtime_connection: RuntimeConnectionKind::LocalAttached,
            settings_scroll: u16::MAX,
            ..UiState::default()
        };
        let snapshot = Snapshot::default();

        let frame = terminal
            .draw(|frame| {
                render_settings_page(frame, Palette::default(), &mut ui, &snapshot, frame.area());
            })
            .expect("draw local attached Settings");
        let text = buffer_text(frame.buffer);

        assert!(text.contains("Local paths"), "{text}");
        assert!(text.contains("request log:"), "{text}");
        assert!(text.contains("database:"), "{text}");
        assert!(text.contains("logs:"), "{text}");
        assert!(text.contains("reports:"), "{text}");
    }

    #[test]
    fn telemetry_lists_every_provider_and_prioritizes_attention_states() {
        let mut snapshot = Snapshot::default();
        for provider in ["alpha", "bravo", "charlie", "delta", "echo"] {
            snapshot.provider_balances.insert(
                provider.to_string(),
                vec![ProviderBalanceSnapshot {
                    observation_provider_id: provider.to_string(),
                    status: BalanceSnapshotStatus::Ok,
                    ..ProviderBalanceSnapshot::default()
                }],
            );
        }
        snapshot.provider_balances.insert(
            "z-error".to_string(),
            vec![ProviderBalanceSnapshot {
                observation_provider_id: "z-error".to_string(),
                status: BalanceSnapshotStatus::Error,
                error: Some("probe_error".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        );

        let text = lines_text(&telemetry_lines(
            Palette::default(),
            &snapshot,
            Language::En,
        ));

        for provider in ["alpha", "bravo", "charlie", "delta", "echo", "z-error"] {
            assert!(text.contains(provider), "missing {provider}:\n{text}");
        }
        assert!(
            text.find("z-error").expect("error provider")
                < text.find("alpha").expect("alphabetical provider"),
            "{text}"
        );
    }

    #[test]
    fn settings_scroll_to_end_reaches_local_paths_and_clamps() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut ui = UiState {
            page: Page::Settings,
            language: Language::En,
            runtime_connection: RuntimeConnectionKind::Integrated,
            settings_scroll: u16::MAX,
            ..UiState::default()
        };
        let snapshot = Snapshot::default();

        let frame = terminal
            .draw(|frame| {
                render_settings_page(frame, Palette::default(), &mut ui, &snapshot, frame.area());
            })
            .expect("draw scrolled Settings");
        let text = buffer_text(frame.buffer);

        assert!(ui.settings_scroll > 0);
        assert!(ui.settings_scroll < u16::MAX);
        assert!(text.contains("Local paths"), "{text}");
        assert!(text.contains("database:"), "{text}");
    }

    #[test]
    fn settings_retry_summary_uses_the_typed_profile() {
        assert_eq!(
            retry_profile_name(Some(RetryProfileName::AggressiveFailover)),
            "aggressive-failover"
        );
    }

    #[test]
    fn settings_retry_summary_exposes_the_resolved_policy() {
        let retry: OperatorRetrySummary = serde_json::from_value(serde_json::json!({
            "configured_profile": "same-upstream",
            "upstream_max_attempts": 3,
            "provider_max_attempts": 2,
            "policy": {
                "upstream": {
                    "max_attempts": 3,
                    "backoff_ms": 100,
                    "backoff_max_ms": 1000,
                    "jitter_ms": 25,
                    "on_status": "408,429,5xx",
                    "on_class": ["upstream_transport_error"],
                    "strategy": "same_upstream"
                },
                "provider": {
                    "max_attempts": 2,
                    "backoff_ms": 200,
                    "backoff_max_ms": 2000,
                    "jitter_ms": 50,
                    "on_status": "429,5xx",
                    "on_class": ["cloudflare_challenge"],
                    "strategy": "failover"
                },
                "never_on_status": "400,401,403",
                "never_on_class": ["invalid_request"],
                "cloudflare_challenge_cooldown_secs": 45,
                "cloudflare_timeout_cooldown_secs": 30,
                "transport_cooldown_secs": 20,
                "cooldown_backoff_factor": 2,
                "cooldown_backoff_max_secs": 300,
                "reasoning_guard": {
                    "enabled": true,
                    "reasoning_equals": [0, 1],
                    "boundary_sequence_max_n": 4,
                    "paths": ["/v1/responses"],
                    "action": "retry",
                    "stream_mode": "strict-buffer",
                    "max_guard_retries": 2,
                    "on_retry_exhausted": "block",
                    "log_matches": true
                }
            }
        }))
        .expect("retry summary fixture");

        let text = lines_text(&retry_lines(&retry, Language::En));
        for expected in [
            "upstream: strategy=same-upstream attempts=3 backoff=100..1000ms jitter=25ms",
            "retry_on: status=[408,429,5xx] class=[upstream_transport_error]",
            "provider: strategy=failover attempts=2 backoff=200..2000ms jitter=50ms",
            "never: status=[400,401,403] class=[invalid_request]",
            "cooldown: transport=20s cf_challenge=45s cf_timeout=30s factor=2 max=300s",
            "reasoning_guard: on tokens=[0,1] boundary=4 action=retry stream=strict-buffer retries=2 exhausted=block paths=[/v1/responses] log=on",
        ] {
            assert!(text.contains(expected), "missing {expected:?}:\n{text}");
        }
    }

    #[test]
    fn codex_switch_status_shows_the_complete_client_patch() {
        let status = codex_helper_core::codex_switch::CodexSwitchStatus {
            phase: codex_helper_core::codex_switch::CodexSwitchPhase::Applied,
            enabled: true,
            managed: true,
            base_url: Some("http://127.0.0.1:3211/v1".to_string()),
            client_patch: Some(crate::config::CodexClientPatchConfig {
                preset: crate::config::CodexClientPreset::OfficialImagegen,
                responses_websocket: true,
                compaction: crate::config::CodexCompactionStrategy::RemoteV2,
                translate_models: true,
                hosted_image_generation: crate::config::CodexHostedImageGenerationMode::Disabled,
            }),
            recovery_reason: None,
            config_path: "/tmp/codex/config.toml".into(),
            state_path: "/tmp/helper/codex-switch.json".into(),
        };

        let text = codex_switch_status_lines(Palette::default(), Language::En, &status)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("preset: official-imagegen"), "{text}");
        assert!(text.contains("compaction: remote-v2"), "{text}");
        assert!(text.contains("websocket: yes"), "{text}");
        assert!(text.contains("translate_models: yes"), "{text}");
        assert!(text.contains("hosted_image: disabled"), "{text}");
    }
}

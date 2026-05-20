use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::codex_capability_profile::{
    CodexCapabilityDecision, CodexCapabilitySupport, CodexPatchModeRecommendationConfidence,
};
use crate::config::{ResolvedRetryConfig, ResolvedRetryLayerConfig, RetryStrategy};
use crate::healthcheck::{
    HEALTHCHECK_MAX_INFLIGHT_ENV, HEALTHCHECK_TIMEOUT_MS_ENV, HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV,
};
use crate::proxy::{
    CodexRelayCapabilitiesResponse, CodexRelayLiveSmokeCase, CodexRelayLiveSmokeConfidence,
    CodexRelayLiveSmokeOutcome, CodexRelayLiveSmokeResponse, CodexRelayLiveSmokeResult,
    CodexRelayProbeConfidence, CodexRelayProbeResult, CodexRelayProbeSupport,
};
use crate::tui::Language;
use crate::tui::codex_relay_live_smoke::CodexRelayLiveSmokeMode;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_amount_brief_lang, balance_snapshot_status_label_lang, now_ms,
    shorten, shorten_middle,
};
use crate::tui::state::UiState;

fn retry_strategy_name(strategy: RetryStrategy) -> &'static str {
    match strategy {
        RetryStrategy::Failover => "failover",
        RetryStrategy::SameUpstream => "same_upstream",
    }
}

fn retry_trigger_summary(layer: &ResolvedRetryLayerConfig) -> String {
    let statuses = if layer.on_status.trim().is_empty() {
        "-".to_string()
    } else {
        layer.on_status.clone()
    };
    let classes = if layer.on_class.is_empty() {
        "-".to_string()
    } else {
        layer.on_class.join(",")
    };
    format!("status=[{statuses}] class=[{classes}]")
}

fn capability_support_label(support: CodexCapabilitySupport) -> &'static str {
    match support {
        CodexCapabilitySupport::Unknown => "unknown",
        CodexCapabilitySupport::Supported => "supported",
        CodexCapabilitySupport::Unsupported => "unsupported",
    }
}

fn probe_support_label(support: CodexRelayProbeSupport) -> &'static str {
    match support {
        CodexRelayProbeSupport::Supported => "supported",
        CodexRelayProbeSupport::Unsupported => "unsupported",
        CodexRelayProbeSupport::Unknown => "unknown",
    }
}

fn probe_confidence_label(confidence: CodexRelayProbeConfidence) -> &'static str {
    match confidence {
        CodexRelayProbeConfidence::SuccessStatus => "success_status",
        CodexRelayProbeConfidence::EndpointValidation => "endpoint_validation",
        CodexRelayProbeConfidence::ErrorClassification => "error_classification",
        CodexRelayProbeConfidence::Transport => "transport",
        CodexRelayProbeConfidence::Malformed => "malformed",
    }
}

fn live_smoke_case_label(case: CodexRelayLiveSmokeCase) -> &'static str {
    match case {
        CodexRelayLiveSmokeCase::ResponsesCompact => "responses_compact",
        CodexRelayLiveSmokeCase::HostedImageGeneration => "hosted_image_generation",
        CodexRelayLiveSmokeCase::ResponsesWebSocket => "responses_websocket",
    }
}

fn live_smoke_outcome_label(outcome: CodexRelayLiveSmokeOutcome) -> &'static str {
    match outcome {
        CodexRelayLiveSmokeOutcome::Passed => "passed",
        CodexRelayLiveSmokeOutcome::Failed => "failed",
        CodexRelayLiveSmokeOutcome::Unknown => "unknown",
    }
}

fn live_smoke_confidence_label(confidence: CodexRelayLiveSmokeConfidence) -> &'static str {
    match confidence {
        CodexRelayLiveSmokeConfidence::LiveOutputShape => "live_output_shape",
        CodexRelayLiveSmokeConfidence::LiveAccepted => "live_accepted",
        CodexRelayLiveSmokeConfidence::LiveError => "live_error",
        CodexRelayLiveSmokeConfidence::Transport => "transport",
        CodexRelayLiveSmokeConfidence::Malformed => "malformed",
    }
}

fn recommendation_confidence_label(
    confidence: CodexPatchModeRecommendationConfidence,
) -> &'static str {
    match confidence {
        CodexPatchModeRecommendationConfidence::High => "high",
        CodexPatchModeRecommendationConfidence::Medium => "medium",
        CodexPatchModeRecommendationConfidence::Low => "low",
    }
}

fn live_smoke_result_brief(result: &CodexRelayLiveSmokeResult) -> String {
    let mut parts = vec![
        live_smoke_outcome_label(result.outcome).to_string(),
        format!("via {}", live_smoke_confidence_label(result.confidence)),
    ];
    if let Some(status_code) = result.status_code {
        parts.push(format!("status={status_code}"));
    }
    if let Some(shape) = result.response_shape.as_deref() {
        parts.push(format!("shape={shape}"));
    }
    if result.output_items_seen > 0 {
        parts.push(format!("items={}", result.output_items_seen));
    }
    if result.image_generation_call_seen {
        parts.push("image_call=true".to_string());
    }
    if result.image_result_present {
        parts.push("image_result=true".to_string());
    }
    if let Some(error_class) = result.error_class.as_deref() {
        parts.push(format!("class={error_class}"));
    }
    parts.push(shorten(&result.reason, 72));
    parts.join("  ")
}

fn decision_brief(decision: &CodexCapabilityDecision) -> String {
    format!(
        "{} ({})",
        capability_support_label(decision.support),
        shorten(&decision.reason, 72)
    )
}

fn probe_brief(result: &CodexRelayProbeResult) -> String {
    let mut parts = vec![
        probe_support_label(result.support).to_string(),
        format!("via {}", probe_confidence_label(result.confidence)),
    ];
    if let Some(status_code) = result.status_code {
        parts.push(format!("status={status_code}"));
    }
    if let Some(shape) = result.response_shape.as_deref() {
        parts.push(format!("shape={shape}"));
    }
    if result.translation_required {
        parts.push("translation_required=true".to_string());
    }
    parts.push(shorten(&result.reason, 72));
    parts.join("  ")
}

fn codex_relay_diagnostics_lines(p: Palette, ui: &UiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => "Codex Relay 能力诊断",
            Language::En => "Codex Relay Diagnostics",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => {
                "  C 运行一次有界诊断：/models 只读，/responses 与 /responses/compact 只发 {} 校验请求；不会自动切换 preset。"
            }
            Language::En => {
                "  C runs one bounded diagnostic: /models read-only, /responses and /responses/compact send {} validation probes; preset is not changed automatically."
            }
        },
        Style::default().fg(p.muted),
    )]));

    let state = &ui.codex_relay_diagnostics;
    if state.loading {
        lines.push(Line::from(vec![Span::styled(
            match ui.language {
                Language::Zh => "  status: 诊断中...",
                Language::En => "  status: running...",
            },
            Style::default().fg(p.accent),
        )]));
    }
    if let Some(error) = state.last_error.as_deref() {
        lines.push(Line::from(vec![Span::styled(
            format!("  error: {}", shorten(error, 110)),
            Style::default().fg(p.warn),
        )]));
    }

    if let Some(response) = state.last_result.as_ref() {
        push_codex_relay_diagnostics_result_lines(&mut lines, p, response);
    } else if !state.loading && state.last_error.is_none() {
        lines.push(Line::from(vec![Span::styled(
            match ui.language {
                Language::Zh => "  status: 尚未运行",
                Language::En => "  status: not run",
            },
            Style::default().fg(p.muted),
        )]));
    }
    lines
}

fn codex_relay_live_smoke_lines(p: Palette, ui: &UiState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => "Codex Relay Live Smoke",
            Language::En => "Codex Relay Live Smoke",
        },
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![Span::styled(
        match ui.language {
            Language::Zh => {
                "  X 二次确认后真实请求 /responses/compact；Y 二次确认后真实请求 compact+hosted image_generation。会消耗上游 tokens/余额，不会更新路由健康状态。"
            }
            Language::En => {
                "  X double-confirms a real /responses/compact request; Y double-confirms compact+hosted image_generation. This may consume upstream tokens/credits and does not update routing health."
            }
        },
        Style::default().fg(p.warn),
    )]));

    let state = &ui.codex_relay_live_smoke;
    if let Some(mode) = state.pending_confirm {
        lines.push(Line::from(vec![Span::styled(
            format!(
                "  confirm: press {} again within 3s",
                match mode {
                    CodexRelayLiveSmokeMode::CompactOnly => "X",
                    CodexRelayLiveSmokeMode::CompactAndImage => "Y",
                }
            ),
            Style::default().fg(p.warn),
        )]));
    }
    if state.loading {
        lines.push(Line::from(vec![Span::styled(
            match (ui.language, state.mode) {
                (Language::Zh, Some(CodexRelayLiveSmokeMode::CompactOnly)) => {
                    "  status: remote compaction live smoke 运行中..."
                }
                (Language::Zh, Some(CodexRelayLiveSmokeMode::CompactAndImage)) => {
                    "  status: compact+image live smoke 运行中..."
                }
                (Language::Zh, None) => "  status: live smoke 运行中...",
                (Language::En, Some(CodexRelayLiveSmokeMode::CompactOnly)) => {
                    "  status: remote compaction live smoke running..."
                }
                (Language::En, Some(CodexRelayLiveSmokeMode::CompactAndImage)) => {
                    "  status: compact+image live smoke running..."
                }
                (Language::En, None) => "  status: live smoke running...",
            },
            Style::default().fg(p.accent),
        )]));
    }
    if let Some(error) = state.last_error.as_deref() {
        lines.push(Line::from(vec![Span::styled(
            format!("  error: {}", shorten(error, 110)),
            Style::default().fg(p.warn),
        )]));
    }

    if let Some(response) = state.last_result.as_ref() {
        push_codex_relay_live_smoke_result_lines(&mut lines, p, response);
    } else if !state.loading && state.last_error.is_none() {
        lines.push(Line::from(vec![Span::styled(
            match ui.language {
                Language::Zh => "  status: 尚未运行",
                Language::En => "  status: not run",
            },
            Style::default().fg(p.muted),
        )]));
    }
    lines
}

fn push_codex_relay_live_smoke_result_lines(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    response: &CodexRelayLiveSmokeResponse,
) {
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  target: {} #{}  {}",
            response.station_name,
            response.upstream_index,
            shorten_middle(&response.upstream_base_url, 70)
        ),
        Style::default().fg(p.text),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  model: requested={}  upstream={}",
            response.requested_model, response.upstream_model
        ),
        Style::default().fg(p.muted),
    )]));
    for result in &response.results {
        let color = match result.outcome {
            CodexRelayLiveSmokeOutcome::Passed => p.good,
            CodexRelayLiveSmokeOutcome::Failed => p.bad,
            CodexRelayLiveSmokeOutcome::Unknown => p.warn,
        };
        lines.push(Line::from(vec![Span::styled(
            format!(
                "  live {}: {}",
                live_smoke_case_label(result.case),
                live_smoke_result_brief(result)
            ),
            Style::default().fg(color),
        )]));
    }
    for warning in response.warnings.iter().take(3) {
        lines.push(Line::from(vec![Span::styled(
            format!("    warning: {}", shorten(warning, 96)),
            Style::default().fg(p.warn),
        )]));
    }
}

fn push_codex_relay_diagnostics_result_lines(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    response: &CodexRelayCapabilitiesResponse,
) {
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  target: {} #{}  {}",
            response.station_name,
            response.upstream_index,
            shorten_middle(&response.upstream_base_url, 70)
        ),
        Style::default().fg(p.text),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  preset={}  model={}  catalog_shape={:?}  selected={:?}",
            response.patch_mode.as_preset_str(),
            response.model.as_deref().unwrap_or("-"),
            response.expected.model_catalog.shape,
            response.expected.model_catalog.selection
        ),
        Style::default().fg(p.muted),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  expected: compact={}  image_generation={}  web_search={}  apply_patch={}",
            decision_brief(&response.expected.remote_compaction_v1),
            decision_brief(&response.expected.hosted_image_generation),
            decision_brief(&response.expected.web_search),
            decision_brief(&response.expected.apply_patch)
        ),
        Style::default().fg(p.muted),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  observed /models: {}",
            probe_brief(&response.observed.models)
        ),
        Style::default().fg(p.muted),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  observed /responses: {}",
            probe_brief(&response.observed.responses)
        ),
        Style::default().fg(p.muted),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  observed /responses/compact: {}",
            probe_brief(&response.observed.responses_compact)
        ),
        Style::default().fg(p.muted),
    )]));

    if response.mismatches.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "  mismatches: none",
            Style::default().fg(p.good),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            format!("  mismatches: {}", response.mismatches.len()),
            Style::default().fg(p.warn),
        )]));
        for mismatch in response.mismatches.iter().take(4) {
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "    - {} expected={} observed={}  {}",
                    mismatch.capability,
                    mismatch.expected,
                    mismatch.observed,
                    shorten(&mismatch.reason, 86)
                ),
                Style::default().fg(p.warn),
            )]));
        }
    }

    let recommendation = &response.recommendation;
    lines.push(Line::from(vec![Span::styled(
        format!(
            "  recommendation: {} -> {}  confidence={}{}",
            recommendation.current_patch_mode.as_preset_str(),
            recommendation.recommended_patch_mode.as_preset_str(),
            recommendation_confidence_label(recommendation.confidence),
            if recommendation.changes_current_mode {
                "  change"
            } else {
                ""
            }
        ),
        Style::default().fg(if recommendation.changes_current_mode {
            p.accent
        } else {
            p.good
        }),
    )]));
    for reason in recommendation.reasons.iter().take(3) {
        lines.push(Line::from(vec![Span::styled(
            format!("    reason: {}", shorten(reason, 96)),
            Style::default().fg(p.muted),
        )]));
    }
    for warning in recommendation.warnings.iter().take(3) {
        lines.push(Line::from(vec![Span::styled(
            format!("    warning: {}", shorten(warning, 96)),
            Style::default().fg(p.warn),
        )]));
    }
}

fn retry_layer_preview_lang(
    label: &str,
    layer: &ResolvedRetryLayerConfig,
    lang: Language,
) -> String {
    format!(
        "{label}: {}={} {}={} backoff={}..{}ms jitter={}ms retry_on={}",
        i18n::label(lang, "strategy"),
        retry_strategy_name(layer.strategy),
        i18n::label(lang, "attempts"),
        layer.max_attempts,
        layer.backoff_ms,
        layer.backoff_max_ms,
        layer.jitter_ms,
        retry_trigger_summary(layer)
    )
}

#[cfg(test)]
fn retry_policy_preview_lines(retry: &ResolvedRetryConfig) -> Vec<String> {
    retry_policy_preview_lines_lang(retry, Language::En)
}

fn retry_policy_preview_lines_lang(retry: &ResolvedRetryConfig, lang: Language) -> Vec<String> {
    let mut lines = vec![
        retry_layer_preview_lang("upstream", &retry.upstream, lang),
        retry_layer_preview_lang("route", &retry.route, lang),
    ];
    let boundary = match (lang, retry.allow_cross_station_before_first_output) {
        (Language::Zh, true) => "边界：首个输出前允许跨站点 failover；输出后保持已提交路由",
        (Language::Zh, false) => "边界：首个输出前禁止跨站点 failover；仅执行同站点/上游策略",
        (Language::En, true) => {
            "boundary: cross-station failover allowed before first output; after output stays on committed route"
        }
        (Language::En, false) => {
            "boundary: cross-station failover blocked before first output; same-station/upstream policy only"
        }
    };
    lines.push(boundary.to_string());
    let never_class = if retry.never_on_class.is_empty() {
        "-".to_string()
    } else {
        retry.never_on_class.join(",")
    };
    lines.push(match lang {
        Language::Zh => format!(
            "护栏：never_status=[{}] never_class=[{}]",
            retry.never_on_status, never_class
        ),
        Language::En => format!(
            "guardrails: never_status=[{}] never_class=[{}]",
            retry.never_on_status, never_class
        ),
    });
    lines.push(match lang {
        Language::Zh => format!(
            "冷却：transport={}s cf_challenge={}s cf_timeout={}s backoff_factor={} max={}s",
            retry.transport_cooldown_secs,
            retry.cloudflare_challenge_cooldown_secs,
            retry.cloudflare_timeout_cooldown_secs,
            retry.cooldown_backoff_factor,
            retry.cooldown_backoff_max_secs
        ),
        Language::En => format!(
            "cooldown: transport={}s cf_challenge={}s cf_timeout={}s backoff_factor={} max={}s",
            retry.transport_cooldown_secs,
            retry.cloudflare_challenge_cooldown_secs,
            retry.cloudflare_timeout_cooldown_secs,
            retry.cooldown_backoff_factor,
            retry.cooldown_backoff_max_secs
        ),
    });
    lines
}

fn pricing_catalog_preview_lines_lang(
    snapshot: &Snapshot,
    limit: usize,
    lang: Language,
) -> Vec<String> {
    let catalog = &snapshot.pricing_catalog;
    let mut lines = vec![format!(
        "source={}  models={}",
        catalog.source, catalog.model_count
    )];
    if catalog.models.is_empty() {
        lines.push(i18n::label(lang, "no price rows").to_string());
        return lines;
    }

    for row in prioritized_price_rows(snapshot, limit) {
        lines.push(format!(
            "{}  in={} out={} cr={} cc={}  {}/{}",
            shorten_middle(&price_model_label(row), 24),
            format_price(&row.input_per_1m_usd),
            format_price(&row.output_per_1m_usd),
            format_optional_price(row.cache_read_input_per_1m_usd.as_deref()),
            format_optional_price(row.cache_creation_input_per_1m_usd.as_deref()),
            confidence_label_lang(row.confidence, lang),
            row.source
        ));
    }

    lines
}

fn balance_overview_lines_lang(snapshot: &Snapshot, limit: usize, lang: Language) -> Vec<String> {
    let mut stations = summarize_station_balances(&snapshot.provider_balances);
    if stations.is_empty() {
        return vec![i18n::label(lang, "no balance/quota data").to_string()];
    }

    stations.sort_by(|left, right| {
        station_priority(left)
            .cmp(&station_priority(right))
            .then_with(|| left.station_name.cmp(&right.station_name))
    });

    let total_rows = stations
        .iter()
        .map(|station| station.total_rows)
        .sum::<usize>();
    let exhausted_rows = stations
        .iter()
        .map(|station| station.exhausted_rows)
        .sum::<usize>();
    let lazy_rows = stations
        .iter()
        .map(|station| station.lazy_rows)
        .sum::<usize>();
    let stale_rows = stations
        .iter()
        .map(|station| station.stale_rows)
        .sum::<usize>();
    let unknown_rows = stations
        .iter()
        .map(|station| station.unknown_rows + station.error_rows)
        .sum::<usize>();

    let mut lines = vec![format!(
        "{}={}  rows={}  {}={}  {}={}  {}={}  {}={}",
        i18n::label(lang, "stations"),
        stations.len(),
        total_rows,
        i18n::label(lang, "exhausted"),
        exhausted_rows,
        i18n::label(lang, "lazy"),
        lazy_rows,
        i18n::label(lang, "stale"),
        stale_rows,
        i18n::label(lang, "unknown"),
        unknown_rows
    )];
    for station in stations.into_iter().take(limit) {
        lines.push(format!(
            "{}  rows={} {}={} {}={} {}={} {}={} {}={}  {}",
            shorten_middle(&station.station_name, 20),
            station.total_rows,
            i18n::label(lang, "ok"),
            station.ok_rows,
            i18n::label(lang, "stale"),
            station.stale_rows,
            i18n::label(lang, "exhausted"),
            station.exhausted_rows,
            i18n::label(lang, "lazy"),
            station.lazy_rows,
            i18n::label(lang, "unknown"),
            station.unknown_rows + station.error_rows,
            station
                .primary
                .as_ref()
                .map(|snapshot| format_primary_balance_lang(snapshot, lang))
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    lines
}

fn prioritized_price_rows(
    snapshot: &Snapshot,
    limit: usize,
) -> Vec<&crate::pricing::ModelPriceView> {
    snapshot
        .pricing_catalog
        .prioritized_models(recent_model_order(snapshot), limit)
}

fn recent_model_order(snapshot: &Snapshot) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for request in &snapshot.recent {
        if let Some(model) = request
            .model
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }
    for row in &snapshot.rows {
        if let Some(model) = row
            .effective_model
            .as_ref()
            .map(|value| value.value.as_str())
            .or(row.last_model.as_deref())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            *counts.entry(model.to_string()).or_default() += 1;
        }
    }

    let mut models = counts.into_iter().collect::<Vec<_>>();
    models.sort_by(|(left_model, left_count), (right_model, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_model.cmp(right_model))
    });
    models.into_iter().map(|(model, _)| model).collect()
}

fn price_model_label(row: &crate::pricing::ModelPriceView) -> String {
    match row.display_name.as_deref() {
        Some(display) if display != row.model_id => format!("{display} ({})", row.model_id),
        Some(display) => display.to_string(),
        None => row.model_id.clone(),
    }
}

fn format_price(value: &str) -> String {
    format!("${value}")
}

fn format_optional_price(value: Option<&str>) -> String {
    value.map(format_price).unwrap_or_else(|| "-".to_string())
}

fn confidence_label_lang(
    confidence: crate::pricing::CostConfidence,
    lang: Language,
) -> &'static str {
    match confidence {
        crate::pricing::CostConfidence::Unknown => i18n::label(lang, "unknown"),
        crate::pricing::CostConfidence::Partial => i18n::label(lang, "partial"),
        crate::pricing::CostConfidence::Estimated => i18n::label(lang, "estimated"),
        crate::pricing::CostConfidence::Exact => i18n::label(lang, "exact"),
    }
}

#[derive(Debug, Clone)]
struct StationBalanceSummary {
    station_name: String,
    total_rows: usize,
    ok_rows: usize,
    stale_rows: usize,
    exhausted_rows: usize,
    lazy_rows: usize,
    error_rows: usize,
    unknown_rows: usize,
    primary: Option<crate::state::ProviderBalanceSnapshot>,
}

fn summarize_station_balances(
    provider_balances: &HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
) -> Vec<StationBalanceSummary> {
    provider_balances
        .iter()
        .map(|(station_name, balances)| StationBalanceSummary {
            station_name: station_name.clone(),
            total_rows: balances.len(),
            ok_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Ok)
                .count(),
            stale_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Stale)
                .count(),
            exhausted_rows: balances
                .iter()
                .filter(|balance| {
                    balance.status == crate::state::BalanceSnapshotStatus::Exhausted
                        && !balance.routing_ignored_exhaustion()
                })
                .count(),
            lazy_rows: balances
                .iter()
                .filter(|balance| balance.routing_ignored_exhaustion())
                .count(),
            error_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Error)
                .count(),
            unknown_rows: balances
                .iter()
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Unknown)
                .count(),
            primary: balances.iter().cloned().min_by(balance_priority),
        })
        .collect()
}

fn balance_priority(
    left: &crate::state::ProviderBalanceSnapshot,
    right: &crate::state::ProviderBalanceSnapshot,
) -> std::cmp::Ordering {
    balance_snapshot_rank(left)
        .cmp(&balance_snapshot_rank(right))
        .then_with(|| left.upstream_index.cmp(&right.upstream_index))
        .then_with(|| left.provider_id.cmp(&right.provider_id))
        .then_with(|| left.fetched_at_ms.cmp(&right.fetched_at_ms))
}

fn balance_snapshot_rank(snapshot: &crate::state::ProviderBalanceSnapshot) -> u8 {
    if snapshot.status == crate::state::BalanceSnapshotStatus::Exhausted
        && !snapshot.routing_ignored_exhaustion()
    {
        return 0;
    }

    match snapshot.status {
        crate::state::BalanceSnapshotStatus::Stale
        | crate::state::BalanceSnapshotStatus::Exhausted => 1,
        crate::state::BalanceSnapshotStatus::Error
        | crate::state::BalanceSnapshotStatus::Unknown => 2,
        crate::state::BalanceSnapshotStatus::Ok => 3,
    }
}

fn station_priority(summary: &StationBalanceSummary) -> u8 {
    summary
        .primary
        .as_ref()
        .map(balance_snapshot_rank)
        .unwrap_or(5)
}

fn format_primary_balance_lang(
    snapshot: &crate::state::ProviderBalanceSnapshot,
    lang: Language,
) -> String {
    let provider_id = if snapshot.provider_id.trim().is_empty() {
        "-".to_string()
    } else {
        shorten_middle(&snapshot.provider_id, 20)
    };
    let amount =
        balance_amount_brief_lang(snapshot, lang).unwrap_or_else(|| snapshot.amount_summary());
    let mut line = format!(
        "{}  {}  #{}  {}",
        amount,
        provider_id,
        snapshot
            .upstream_index
            .map(|idx| idx.to_string())
            .unwrap_or_else(|| "-".to_string()),
        balance_snapshot_status_label_lang(snapshot, lang)
    );
    if let Some(err) = snapshot.error.as_deref()
        && !err.trim().is_empty()
    {
        line.push_str(&format!(
            "  {}={}",
            i18n::label(lang, "lookup_failed"),
            shorten(err, 48)
        ));
    }
    line
}

pub(super) fn render_settings_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let now_epoch_ms = now_ms();
    let block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::SETTINGS_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();

    let lang_name = i18n::language_name(ui.language);
    let refresh_env = std::env::var("CODEX_HELPER_TUI_REFRESH_MS").ok();
    let recent_max_env = std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX").ok();
    let health_timeout_env = std::env::var(HEALTHCHECK_TIMEOUT_MS_ENV).ok();
    let health_inflight_env = std::env::var(HEALTHCHECK_MAX_INFLIGHT_ENV).ok();
    let health_upstream_conc_env = std::env::var(HEALTHCHECK_UPSTREAM_CONCURRENCY_ENV).ok();

    let effective_recent_max = recent_max_env
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_000)
        .clamp(200, 20_000);

    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;
    let ok_pct = |ok: usize, total: usize| -> String {
        if total == 0 {
            "-".to_string()
        } else {
            format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
        }
    };

    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::RUNTIME_OVERVIEW_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            match ui.language {
                Language::Zh => "连接：",
                Language::En => "connection: ",
            },
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_connection.label(ui.language),
            Style::default().fg(if ui.runtime_connection.is_attached() {
                p.accent
            } else {
                p.text
            }),
        ),
        Span::styled(
            format!(
                "  shutdown: {}",
                match ui.runtime_shutdown_available {
                    Some(true) => match ui.language {
                        Language::Zh => "可用",
                        Language::En => "available",
                    },
                    Some(false) => match ui.language {
                        Language::Zh => "不可用",
                        Language::En => "unavailable",
                    },
                    None => "-",
                }
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    if ui.runtime_connection.is_attached() {
        lines.push(Line::from(vec![Span::styled(
            match ui.language {
                Language::Zh => {
                    "  附着 TUI 是只读观察路径：q 只退出控制台，不会停止 resident proxy；停止代理请用 `codex-helper daemon stop`。"
                }
                Language::En => {
                    "  Attached TUI is read-only: q exits only the console and keeps the resident proxy running; stop it with `codex-helper daemon stop`."
                }
            },
            Style::default().fg(p.muted),
        )]));
    }
    if let Some(error) = ui.runtime_status_error.as_deref()
        && !error.trim().is_empty()
    {
        lines.push(Line::from(vec![
            Span::styled(
                match ui.language {
                    Language::Zh => "  状态刷新失败：",
                    Language::En => "  status refresh failed: ",
                },
                Style::default().fg(p.warn),
            ),
            Span::styled(shorten(error, 96), Style::default().fg(p.warn)),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("5m ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s5.ok_2xx, s5.total),
                s5.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.err_429,
                s5.err_5xx,
                s5.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("1h ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s1.ok_2xx, s1.total),
                s1.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.err_429,
                s1.err_5xx,
                s1.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some((pid, n)) = s5.top_provider.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top provider: ", Style::default().fg(p.muted)),
            Span::styled(pid.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }
    if let Some((cfg, n)) = s5.top_config.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top station: ", Style::default().fg(p.muted)),
            Span::styled(cfg.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::BALANCE_OVERVIEW_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    for line in balance_overview_lines_lang(snapshot, 6, ui.language) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(p.muted)),
            Span::styled(line, Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PRICING_CATALOG_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    for line in pricing_catalog_preview_lines_lang(snapshot, 6, ui.language) {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(p.muted)),
            Span::styled(line, Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::TUI_OPTIONS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(lang_name, Style::default().fg(p.text)),
        Span::styled(
            i18n::text(ui.language, msg::LANGUAGE_TOGGLE_HINT),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::REFRESH_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{}ms", ui.refresh_ms), Style::default().fg(p.text)),
        Span::styled(
            format!(
                "  env CODEX_HELPER_TUI_REFRESH_MS={}",
                refresh_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::WINDOW_SAMPLES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("recent_finished_max={effective_recent_max}"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            format!(
                "  env CODEX_HELPER_RECENT_FINISHED_MAX={}",
                recent_max_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PROFILE_CONTROL_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
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
            i18n::text(ui.language, msg::PRESS_P_MANAGE),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::RUNTIME_OVERRIDE_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.runtime_default_profile_override
                .as_deref()
                .unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            i18n::text(ui.language, msg::PRESS_CAPITAL_P_MANAGE),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::EFFECTIVE_DEFAULT_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            ui.effective_default_profile.as_deref().unwrap_or("<none>"),
            Style::default().fg(p.text),
        ),
    ]));
    let profile_list = if ui.profile_options.is_empty() {
        i18n::text(ui.language, msg::NO_PROFILES).to_string()
    } else {
        shorten_middle(
            ui.profile_options
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
                .as_str(),
            110,
        )
    };
    lines.push(Line::from(vec![
        Span::styled(
            i18n::text(ui.language, msg::AVAILABLE_PROFILES_LABEL),
            Style::default().fg(p.muted),
        ),
        Span::styled(profile_list, Style::default().fg(p.text)),
    ]));

    if ui.service_name == "codex" {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            match ui.language {
                Language::Zh => "Codex 客户端 Patch",
                Language::En => "Codex Client Patch",
            },
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        match crate::codex_integration::codex_switch_status() {
            Ok(status) => {
                lines.push(Line::from(vec![
                    Span::styled("  preset: ", Style::default().fg(p.muted)),
                    Span::styled(
                        status
                            .patch_mode
                            .map(|mode| mode.as_preset_str().to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        Style::default().fg(p.text),
                    ),
                    Span::styled("  base_url: ", Style::default().fg(p.muted)),
                    Span::styled(
                        status.base_url.as_deref().unwrap_or("-").to_string(),
                        Style::default().fg(p.text),
                    ),
                ]));
                lines.push(Line::from(vec![Span::styled(
                    match ui.language {
                        Language::Zh => {
                            "  B/I/F/V/D 启用 ChatGPT / Imagegen / Official relay / Official imagegen / 默认 preset；C 诊断 relay 能力；X/Y 确认后运行 live smoke。修改 ~/.codex/config.toml 后已有 Codex app 需要重启。"
                        }
                        Language::En => {
                            "  B/I/F/V/D enable ChatGPT / Imagegen / Official relay / Official imagegen / default preset; C diagnoses relay capabilities; X/Y run live smoke after confirmation. Restart existing Codex apps after ~/.codex/config.toml changes."
                        }
                    },
                    Style::default().fg(p.muted),
                )]));
                lines.extend(codex_relay_diagnostics_lines(p, ui));
                lines.extend(codex_relay_live_smoke_lines(p, ui));
            }
            Err(err) => {
                lines.push(Line::from(vec![Span::styled(
                    match ui.language {
                        Language::Zh => format!("  读取 Codex switch 状态失败：{err}"),
                        Language::En => format!("  read Codex switch status failed: {err}"),
                    },
                    Style::default().fg(p.warn),
                )]));
                lines.extend(codex_relay_diagnostics_lines(p, ui));
                lines.extend(codex_relay_live_smoke_lines(p, ui));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::HEALTH_CHECK_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "timeout_ms={}  max_inflight={}  upstream_concurrency={}",
            health_timeout_env.as_deref().unwrap_or("-"),
            health_inflight_env.as_deref().unwrap_or("-"),
            health_upstream_conc_env.as_deref().unwrap_or("-"),
        ),
        Style::default().fg(p.muted),
    )]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::PATHS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "config file")),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            crate::config::config_file_path().display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    let home = crate::config::proxy_home_dir();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "home"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(home.display().to_string(), Style::default().fg(p.text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "logs"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            home.join("logs").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<7}", format!("{}:", i18n::label(ui.language, "reports"))),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            home.join("reports").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::RUNTIME_CONFIG_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    let loaded = ui
        .last_runtime_config_loaded_at_ms
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    let mtime = ui
        .last_runtime_config_source_mtime_ms
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());
    lines.push(Line::from(vec![
        Span::styled("loaded_at_ms: ", Style::default().fg(p.muted)),
        Span::styled(loaded, Style::default().fg(p.text)),
        Span::styled("  mtime_ms: ", Style::default().fg(p.muted)),
        Span::styled(mtime, Style::default().fg(p.text)),
        Span::styled(
            i18n::text(ui.language, msg::PRESS_R_RELOAD),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some(retry) = ui.last_runtime_retry.as_ref() {
        lines.push(Line::from(vec![Span::styled(
            i18n::label(ui.language, "retry policy"),
            Style::default().fg(p.text),
        )]));
        for line in retry_policy_preview_lines_lang(retry, ui.language) {
            lines.push(Line::from(vec![
                Span::styled("  - ", Style::default().fg(p.muted)),
                Span::styled(line, Style::default().fg(p.muted)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        i18n::text(ui.language, msg::COMMON_KEYS_TITLE),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(i18n::text(
        ui.language,
        if ui.service_name == "codex" {
            msg::COMMON_KEYS_CODEX
        } else {
            msg::COMMON_KEYS_OTHER
        },
    )));

    lines.push(Line::from(""));
    let updated_ms = snapshot.refreshed_at.elapsed().as_millis();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "updated")),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{updated_ms}ms"), Style::default().fg(p.muted)),
        Span::raw("  "),
        Span::styled(
            format!("{}: ", i18n::label(ui.language, "now")),
            Style::default().fg(p.muted),
        ),
        Span::styled(now_epoch_ms.to_string(), Style::default().fg(p.muted)),
    ]));

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn lines_text(lines: &[Line<'_>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    fn probe_result(
        kind: crate::proxy::CodexRelayProbeKind,
        support: CodexRelayProbeSupport,
        confidence: CodexRelayProbeConfidence,
        status_code: Option<u16>,
        reason: &str,
    ) -> CodexRelayProbeResult {
        CodexRelayProbeResult {
            kind,
            support,
            confidence,
            status_code,
            response_shape: None,
            translation_required: false,
            error_class: None,
            reason: reason.to_string(),
        }
    }

    fn diagnostic_response() -> CodexRelayCapabilitiesResponse {
        let expected =
            crate::codex_capability_profile::CodexCapabilityProfile::for_models_response_json(
                crate::codex_integration::CodexPatchMode::OfficialImagegenBridge,
                &json!({
                    "models": [{
                        "slug": "gpt-5.5",
                        "input_modalities": ["text", "image"],
                        "supports_search_tool": true,
                        "apply_patch_tool_type": "freeform",
                        "supports_reasoning_summaries": true
                    }]
                }),
                Some("gpt-5.5"),
            );
        let observed = crate::proxy::CodexRelayCapabilitiesObserved {
            models: {
                let mut result = probe_result(
                    crate::proxy::CodexRelayProbeKind::Models,
                    CodexRelayProbeSupport::Supported,
                    CodexRelayProbeConfidence::SuccessStatus,
                    Some(200),
                    "relay returned a Codex models catalog",
                );
                result.response_shape = Some("codex_models".to_string());
                result
            },
            responses: probe_result(
                crate::proxy::CodexRelayProbeKind::Responses,
                CodexRelayProbeSupport::Supported,
                CodexRelayProbeConfidence::EndpointValidation,
                Some(400),
                "endpoint exists",
            ),
            responses_compact: probe_result(
                crate::proxy::CodexRelayProbeKind::ResponsesCompact,
                CodexRelayProbeSupport::Unsupported,
                CodexRelayProbeConfidence::ErrorClassification,
                Some(404),
                "endpoint is missing",
            ),
        };
        let recommendation =
            crate::codex_capability_profile::CodexPatchModeRecommendation::for_input(
                crate::codex_capability_profile::CodexPatchModeRecommendationInput {
                    current_patch_mode:
                        crate::codex_integration::CodexPatchMode::OfficialImagegenBridge,
                    model_catalog: expected.model_catalog.clone(),
                    responses: CodexCapabilitySupport::Supported,
                    responses_compact: CodexCapabilitySupport::Unsupported,
                },
            );
        CodexRelayCapabilitiesResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            station_name: "input".to_string(),
            upstream_index: 0,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: "https://relay.example/v1".to_string(),
            patch_mode: crate::codex_integration::CodexPatchMode::OfficialImagegenBridge,
            responses_websocket: false,
            model: Some("gpt-5.5".to_string()),
            expected,
            observed,
            recommendation,
            mismatches: vec![crate::proxy::CodexRelayCapabilityMismatch {
                capability: "remote_compaction_v1".to_string(),
                expected: "supported".to_string(),
                observed: "unsupported via error_classification".to_string(),
                reason: "endpoint is missing".to_string(),
            }],
        }
    }

    fn live_smoke_response() -> CodexRelayLiveSmokeResponse {
        CodexRelayLiveSmokeResponse {
            api_version: 1,
            service_name: "codex".to_string(),
            station_name: "input".to_string(),
            upstream_index: 0,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: "https://relay.example/v1".to_string(),
            requested_model: "gpt-5.5".to_string(),
            upstream_model: "openai/gpt-5.5".to_string(),
            cases: vec![
                CodexRelayLiveSmokeCase::ResponsesCompact,
                CodexRelayLiveSmokeCase::HostedImageGeneration,
            ],
            results: vec![
                CodexRelayLiveSmokeResult {
                    case: CodexRelayLiveSmokeCase::ResponsesCompact,
                    outcome: CodexRelayLiveSmokeOutcome::Passed,
                    confidence: CodexRelayLiveSmokeConfidence::LiveOutputShape,
                    side_effect: crate::proxy::CodexRelayLiveSmokeSideEffect::LiveRequest,
                    status_code: Some(200),
                    response_shape: Some("compact_output_compaction_item".to_string()),
                    output_items_seen: 1,
                    image_generation_call_seen: false,
                    image_result_present: false,
                    accepted_by_responses: false,
                    error_class: None,
                    reason: "compact endpoint returned a live output array".to_string(),
                },
                CodexRelayLiveSmokeResult {
                    case: CodexRelayLiveSmokeCase::HostedImageGeneration,
                    outcome: CodexRelayLiveSmokeOutcome::Passed,
                    confidence: CodexRelayLiveSmokeConfidence::LiveOutputShape,
                    side_effect: crate::proxy::CodexRelayLiveSmokeSideEffect::LiveRequest,
                    status_code: Some(200),
                    response_shape: Some("image_generation_call".to_string()),
                    output_items_seen: 1,
                    image_generation_call_seen: true,
                    image_result_present: true,
                    accepted_by_responses: true,
                    error_class: None,
                    reason: "responses endpoint returned a hosted image_generation_call"
                        .to_string(),
                },
            ],
            warnings: vec![
                "live smoke sends real upstream requests and may consume tokens or credits"
                    .to_string(),
            ],
        }
    }

    fn retry_layer(strategy: RetryStrategy, attempts: u32) -> ResolvedRetryLayerConfig {
        ResolvedRetryLayerConfig {
            max_attempts: attempts,
            backoff_ms: 100,
            backoff_max_ms: 1_000,
            jitter_ms: 25,
            on_status: "429,500-599".to_string(),
            on_class: vec!["upstream_transport_error".to_string()],
            strategy,
        }
    }

    #[test]
    fn retry_policy_preview_lines_explain_layers_and_boundary() {
        let retry = ResolvedRetryConfig {
            upstream: retry_layer(RetryStrategy::SameUpstream, 2),
            route: retry_layer(RetryStrategy::Failover, 3),
            allow_cross_station_before_first_output: true,
            never_on_status: "400,401,403".to_string(),
            never_on_class: vec!["client_error_non_retryable".to_string()],
            cloudflare_challenge_cooldown_secs: 60,
            cloudflare_timeout_cooldown_secs: 30,
            transport_cooldown_secs: 45,
            cooldown_backoff_factor: 2,
            cooldown_backoff_max_secs: 900,
        };

        let lines = retry_policy_preview_lines(&retry);

        assert!(lines[0].contains("upstream: strategy=same_upstream attempts=2"));
        assert!(lines[1].contains("route: strategy=failover attempts=3"));
        assert!(lines[2].contains("cross-station failover allowed before first output"));
        assert!(lines[3].contains("never_status=[400,401,403]"));
        assert!(lines[4].contains("transport=45s"));
    }

    #[test]
    fn primary_balance_summary_puts_amount_before_provider_identity() {
        let snapshot = crate::state::ProviderBalanceSnapshot {
            provider_id: "input".to_string(),
            upstream_index: Some(1),
            status: crate::state::BalanceSnapshotStatus::Ok,
            plan_name: Some("CodeX Pro Annual".to_string()),
            subscription_balance_usd: Some("165.08".to_string()),
            ..crate::state::ProviderBalanceSnapshot::default()
        };

        let line = format_primary_balance_lang(&snapshot, Language::En);
        let amount_pos = line.find("$165.08").expect(&line);
        let provider_pos = line.find("input").expect(&line);

        assert!(amount_pos < provider_pos, "{line}");
    }

    #[test]
    fn codex_relay_diagnostics_lines_show_observed_mismatch_and_recommendation() {
        let ui = UiState {
            codex_relay_diagnostics: crate::tui::state::CodexRelayDiagnosticsState {
                last_result: Some(diagnostic_response()),
                ..Default::default()
            },
            ..UiState::default()
        };

        let text = lines_text(&codex_relay_diagnostics_lines(Palette::default(), &ui));

        assert!(text.contains("Codex Relay Diagnostics"), "{text}");
        assert!(text.contains("observed /responses/compact"), "{text}");
        assert!(text.contains("mismatches: 1"), "{text}");
        assert!(
            text.contains("official-imagegen -> imagegen-bridge"),
            "{text}"
        );
        assert!(text.contains("warning:"), "{text}");
    }

    #[test]
    fn codex_relay_live_smoke_lines_show_confirmation_and_results() {
        let mut ui = UiState {
            codex_relay_live_smoke: crate::tui::state::CodexRelayLiveSmokeState {
                pending_confirm: Some(CodexRelayLiveSmokeMode::CompactAndImage),
                pending_confirm_at: Some(std::time::Instant::now()),
                last_result: Some(live_smoke_response()),
                ..Default::default()
            },
            ..UiState::default()
        };

        let text = lines_text(&codex_relay_live_smoke_lines(Palette::default(), &ui));

        assert!(text.contains("Codex Relay Live Smoke"), "{text}");
        assert!(text.contains("press Y again within 3s"), "{text}");
        assert!(text.contains("live responses_compact: passed"), "{text}");
        assert!(
            text.contains("live hosted_image_generation: passed"),
            "{text}"
        );
        assert!(text.contains("image_result=true"), "{text}");
        assert!(text.contains("warning:"), "{text}");

        ui.codex_relay_live_smoke.pending_confirm = Some(CodexRelayLiveSmokeMode::CompactOnly);
        let text = lines_text(&codex_relay_live_smoke_lines(Palette::default(), &ui));
        assert!(text.contains("press X again within 3s"), "{text}");
    }
}

use eframe::egui;

use super::super::super::i18n::{Language, pick};
use super::super::{
    EffectiveRouteField, effective_route_field_label, format_age, format_duration_ms,
    format_duration_ms_opt, non_empty_trimmed, now_ms, route_decision_field_value,
    route_value_source_label, session_binding_mode_label, shorten_middle, usage_line,
};
use super::console_layout::{ConsoleTone, console_kv_grid, console_note, console_section};
use super::route_explanation::format_service_tier_display;
use crate::state::{FinishedRequest, RouteDecisionProvenance, RouteValueSource};

pub(in super::super) fn render_request_detail_cards(
    ui: &mut egui::Ui,
    lang: Language,
    request: &FinishedRequest,
) {
    render_request_summary_card(ui, lang, request);
    ui.add_space(8.0);
    render_request_usage_speed_cost_card(ui, lang, request);
    ui.add_space(8.0);
    render_request_control_trace_card(ui, lang, request);
    ui.add_space(8.0);
    render_request_retry_chain_card(ui, lang, request);
}

pub(in super::super) fn request_service_tier_display(
    value: Option<&str>,
    lang: Language,
) -> String {
    format_service_tier_display(value, lang, "-")
}

pub(in super::super) fn request_route_decision_reason(
    request: &FinishedRequest,
    decision: &RouteDecisionProvenance,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    let Some(resolved) = route_decision_field_value(decision, field) else {
        return pick(
            lang,
            "这个字段没有对应的路由决策信息。",
            "No route-decision provenance was captured for this field.",
        )
        .to_string();
    };
    let field_label = effective_route_field_label(field, lang);

    match resolved.source {
        RouteValueSource::RequestPayload => format!(
            "{} {}={}.",
            pick(
                lang,
                "这个字段直接来自请求体",
                "This field came directly from the request payload"
            ),
            field_label,
            resolved.value
        ),
        RouteValueSource::SessionOverride => format!(
            "{} {}={}.",
            pick(
                lang,
                "路由时命中了 session override，因此它覆盖了其他来源并固定为",
                "Routing hit a session override, so it replaced every lower-priority source with",
            ),
            field_label,
            resolved.value
        ),
        RouteValueSource::GlobalOverride => format!(
            "{} {}.",
            pick(
                lang,
                "当前没有会话级站点覆盖，因此命中了全局 pin",
                "There was no session-level station override, so routing followed the global pin to",
            ),
            resolved.value
        ),
        RouteValueSource::ProfileDefault => format!(
            "{} {}，{} {}={}.",
            pick(lang, "这个字段来自", "This field came from"),
            request_binding_reference(decision, lang),
            pick(lang, "其默认", "whose default"),
            field_label,
            resolved.value
        ),
        RouteValueSource::StationMapping => {
            let requested_model = request.model.as_deref().unwrap_or("-");
            let station = decision
                .effective_station
                .as_ref()
                .map(|value| value.value.as_str())
                .or(request.station_name.as_deref())
                .unwrap_or("-");
            let upstream = decision
                .effective_upstream_base_url
                .as_ref()
                .map(|value| shorten_middle(&value.value, 56))
                .or_else(|| {
                    request
                        .upstream_base_url
                        .as_deref()
                        .map(|value| shorten_middle(value, 56))
                })
                .unwrap_or_else(|| "-".to_string());
            format!(
                "{} {}，{} {} / upstream {} {} {}.",
                pick(lang, "请求提交的模型是", "The request submitted model"),
                requested_model,
                pick(lang, "但站点", "but station"),
                station,
                upstream,
                pick(
                    lang,
                    "的 model mapping 将实际模型改写为",
                    "rewrote the effective model through model mapping to",
                ),
                resolved.value
            )
        }
        RouteValueSource::RuntimeFallback => match field {
            EffectiveRouteField::Station => format!(
                "{} {}.",
                pick(
                    lang,
                    "没有更高优先级的 session / global / profile 来源，运行时最终选中了站点",
                    "No higher-priority session/global/profile source applied, so runtime finally selected station",
                ),
                resolved.value
            ),
            EffectiveRouteField::Upstream => {
                let station = decision
                    .effective_station
                    .as_ref()
                    .map(|value| value.value.as_str())
                    .or(request.station_name.as_deref())
                    .unwrap_or("-");
                let provider = decision
                    .provider_id
                    .as_deref()
                    .or(request.provider_id.as_deref())
                    .unwrap_or("-");
                format!(
                    "{} {} / provider {}，{} {}.",
                    pick(lang, "站点", "After station"),
                    station,
                    provider,
                    pick(lang, "运行时命中的 upstream 是", "runtime hit upstream"),
                    shorten_middle(&resolved.value, 56)
                )
            }
            _ => format!(
                "{} {}={}.",
                pick(
                    lang,
                    "没有更高优先级来源，运行时沿用了",
                    "No higher-priority source applied, so runtime kept",
                ),
                field_label,
                resolved.value
            ),
        },
    }
}

fn render_request_summary_card(ui: &mut egui::Ui, lang: Language, request: &FinishedRequest) {
    let request_rows = vec![
        ("service".to_string(), request.service.clone()),
        (
            "request".to_string(),
            format!("{} {}", request.method, request.path),
        ),
        ("status".to_string(), request.status_code.to_string()),
        ("total".to_string(), format_duration_ms(request.duration_ms)),
        (
            "first token".to_string(),
            format_duration_ms_opt(request.ttfb_ms),
        ),
        (
            "session".to_string(),
            request
                .session_id
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "trace_id".to_string(),
            request
                .trace_id
                .as_deref()
                .map(|value| shorten_middle(value, 48))
                .unwrap_or_else(|| format!("request-{}", request.id)),
        ),
    ];
    let mut route_rows = vec![
        (
            "model".to_string(),
            request.model.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "effort".to_string(),
            request
                .reasoning_effort
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "service_tier".to_string(),
            request_service_tier_display(request.service_tier.as_deref(), lang),
        ),
        (
            "station".to_string(),
            request
                .station_name
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "provider".to_string(),
            request
                .provider_id
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "upstream host".to_string(),
            request_upstream_host(request).unwrap_or_else(|| "-".to_string()),
        ),
        (
            "upstream".to_string(),
            request
                .upstream_base_url
                .as_deref()
                .map(|value| shorten_middle(value, 84))
                .unwrap_or_else(|| "-".to_string()),
        ),
    ];
    if let Some(usage) = request
        .usage
        .as_ref()
        .filter(|usage| usage.total_tokens > 0)
    {
        route_rows.push(("usage".to_string(), usage_line(usage)));
        route_rows.push((
            "cost".to_string(),
            request.cost.display_total_with_confidence(),
        ));
    }
    if let Some(rate) = request_output_tok_per_sec(request) {
        route_rows.push(("out_tok/s".to_string(), format!("{rate:.1}")));
    }

    let tone = if request.status_code >= 400 {
        ConsoleTone::Warning
    } else {
        ConsoleTone::Neutral
    };
    console_section(
        ui,
        pick(lang, "请求快照", "Request snapshot"),
        tone,
        |ui| {
            ui.columns(2, |cols| {
                cols[0].label(pick(lang, "基本", "Request"));
                console_kv_grid(
                    &mut cols[0],
                    ("requests_snapshot_left", request.id),
                    &request_rows,
                );

                cols[1].label(pick(lang, "路由结果", "Route result"));
                console_kv_grid(
                    &mut cols[1],
                    ("requests_snapshot_right", request.id),
                    &route_rows,
                );
            });

            if request.route_decision.is_none()
                && let Some(note) = request_fast_mode_note(request, None, lang)
            {
                ui.add_space(6.0);
                console_note(ui, note);
            }
        },
    );
}

fn render_request_usage_speed_cost_card(
    ui: &mut egui::Ui,
    lang: Language,
    request: &FinishedRequest,
) {
    let has_usage = request.usage.is_some();
    let has_cost = !request.cost.is_unknown();
    let tone = if has_usage || has_cost {
        ConsoleTone::Positive
    } else {
        ConsoleTone::Neutral
    };

    console_section(
        ui,
        pick(lang, "用量 / 速度 / 成本", "Usage / speed / cost"),
        tone,
        |ui| {
            if !has_usage && !has_cost {
                console_note(
                    ui,
                    pick(
                        lang,
                        "这个请求没有可用的 usage 或成本数据；可能是上游没有返回 usage，或旧日志没有这些字段。",
                        "This request has no usage or cost data; the upstream may not have returned usage, or the record may come from an older log.",
                    ),
                );
                return;
            }

            ui.columns(3, |cols| {
                cols[0].label(pick(lang, "速度", "Speed"));
                console_kv_grid(
                    &mut cols[0],
                    ("requests_usage_speed", request.id),
                    &request_speed_rows(request),
                );

                cols[1].label(pick(lang, "Tokens", "Tokens"));
                let token_rows = request
                    .usage
                    .as_ref()
                    .map(|usage| request_token_rows(usage, request.cache_input_accounting()))
                    .unwrap_or_else(|| {
                        vec![(
                            "usage".to_string(),
                            pick(lang, "unknown", "unknown").to_string(),
                        )]
                    });
                console_kv_grid(
                    &mut cols[1],
                    ("requests_usage_tokens", request.id),
                    &token_rows,
                );

                cols[2].label(pick(lang, "成本", "Cost"));
                console_kv_grid(
                    &mut cols[2],
                    ("requests_usage_cost", request.id),
                    &request_cost_rows(request),
                );
            });
        },
    );
}

fn render_request_control_trace_card(ui: &mut egui::Ui, lang: Language, request: &FinishedRequest) {
    let tone = if request.route_decision.is_some() {
        ConsoleTone::Accent
    } else {
        ConsoleTone::Neutral
    };
    console_section(ui, pick(lang, "控制链", "Control trace"), tone, |ui| {
        let Some(decision) = request.route_decision.as_ref() else {
            console_note(
                ui,
                pick(
                    lang,
                    "当前请求还没有 route_decision 快照；可以看到最终观测结果，但无法准确解释它来自 request payload / session override / profile 默认还是全局 pin。",
                    "This request has no route_decision snapshot yet. The final observed result is still visible, but the exact source chain across request payload, session override, profile default, and global pin cannot be reconstructed precisely.",
                ),
            );
            if let Some(note) = request_fast_mode_note(request, None, lang) {
                ui.add_space(6.0);
                console_note(ui, note);
            }
            return;
        };

        ui.small(format!(
            "{}: {}",
            pick(lang, "决策时间", "Decided"),
            format_age(now_ms(), Some(decision.decided_at_ms))
        ));
        if let Some(profile_name) = decision.binding_profile_name.as_deref() {
            ui.small(format!(
                "{}: {profile_name}",
                pick(lang, "binding(profile)", "Binding (profile)")
            ));
        }
        if decision.binding_continuity_mode.is_some() {
            ui.small(format!(
                "{}: {}",
                pick(lang, "continuity", "Continuity"),
                session_binding_mode_label(decision.binding_continuity_mode, lang)
            ));
        }
        if let Some(provider_endpoint) =
            super::route_explanation::format_route_decision_provider_endpoint(
                decision
                    .provider_id
                    .as_deref()
                    .or(request.provider_id.as_deref()),
                decision.endpoint_id.as_deref(),
            )
        {
            ui.small(format!(
                "{}: {provider_endpoint}",
                pick(lang, "provider/endpoint(决策)", "provider/endpoint")
            ));
        }

        ui.add_space(6.0);
        egui::Grid::new(("requests_control_trace_grid", request.id))
            .num_columns(4)
            .spacing([12.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong(pick(lang, "字段", "Field"));
                ui.strong(pick(lang, "观测值", "Observed"));
                ui.strong(pick(lang, "决策值 / 来源", "Decision / source"));
                ui.strong(pick(lang, "为什么", "Why"));
                ui.end_row();

                for field in EffectiveRouteField::ALL {
                    ui.label(effective_route_field_label(field, lang));
                    ui.monospace(request_observed_route_value(request, field, lang));
                    ui.monospace(
                        super::route_explanation::format_resolved_route_value_for_field(
                            route_decision_field_value(decision, field),
                            field,
                            lang,
                        ),
                    );
                    ui.small(request_route_decision_reason(
                        request, decision, field, lang,
                    ));
                    ui.end_row();
                }
            });

        let changed = request_route_decision_changed_fields(request, decision, lang);
        ui.add_space(6.0);
        if changed.is_empty() {
            console_note(
                ui,
                pick(
                    lang,
                    "最终观测结果与路由决策快照一致。",
                    "The final observed result matches the route decision snapshot.",
                ),
            );
        } else {
            console_note(
                ui,
                format!(
                    "{}: {}",
                    pick(
                        lang,
                        "下列字段的观测值与决策快照不同，解释时以控制链为准",
                        "These observed fields differ from the route decision snapshot; prefer the control-trace explanation for source provenance",
                    ),
                    changed.join(", ")
                ),
            );
        }

        if let Some(note) = request_fast_mode_note(request, Some(decision), lang) {
            ui.add_space(4.0);
            console_note(ui, note);
        }
    });
}

fn render_request_retry_chain_card(ui: &mut egui::Ui, lang: Language, request: &FinishedRequest) {
    let observability = request.observability_view();
    let tone = if observability.retried {
        ConsoleTone::Warning
    } else {
        ConsoleTone::Neutral
    };
    console_section(
        ui,
        pick(lang, "重试 / 熔断链", "Retry / failover chain"),
        tone,
        |ui| {
            if let Some(retry) = request.retry.as_ref() {
                ui.small(format!("attempts: {}", observability.attempt_count));
                if observability.retried {
                    console_note(
                        ui,
                        pick(
                            lang,
                            "这次请求发生了重试或 provider / upstream 切换。",
                            "This request retried or switched provider/upstream during execution.",
                        ),
                    );
                    ui.add_space(4.0);
                }
                let max = 12usize;
                let attempts = retry.route_attempts_or_derived();
                if !attempts.is_empty() {
                    for (idx, attempt) in attempts.iter().take(max).enumerate() {
                        ui.monospace(format!(
                            "{:>2}. {}",
                            idx + 1,
                            request_route_attempt_line(attempt)
                        ));
                    }
                    if attempts.len() > max {
                        ui.small(format!("... +{} more", attempts.len() - max));
                    }
                    return;
                }

                for (idx, entry) in retry.upstream_chain.iter().take(max).enumerate() {
                    ui.monospace(format!("{:>2}. {}", idx + 1, shorten_middle(entry, 120)));
                }
                if retry.upstream_chain.len() > max {
                    ui.small(format!("... +{} more", retry.upstream_chain.len() - max));
                }
                return;
            }

            console_note(
                ui,
                pick(
                    lang,
                    "这次请求没有可见的重试或熔断切换链。",
                    "No visible retry or failover chain was recorded for this request.",
                ),
            );
        },
    );
}

fn request_route_attempt_line(attempt: &crate::logging::RouteAttemptLog) -> String {
    let target = match (
        attempt.station_name.as_deref(),
        attempt.upstream_base_url.as_deref(),
    ) {
        (Some(station), Some(upstream)) => format!("{station}:{}", shorten_middle(upstream, 64)),
        (Some(station), None) => station.to_string(),
        (None, Some(upstream)) => shorten_middle(upstream, 72),
        (None, None) => "-".to_string(),
    };
    let mut parts = vec![attempt.decision.clone()];
    if let Some(provider_id) = attempt.provider_id.as_deref() {
        parts.push(format!("provider={}", shorten_middle(provider_id, 28)));
    }
    let mut attempt_parts = Vec::new();
    if let Some(provider_attempt) = attempt.provider_attempt {
        if let Some(max) = attempt.provider_max_attempts {
            attempt_parts.push(format!("p={provider_attempt}/{max}"));
        } else {
            attempt_parts.push(format!("p={provider_attempt}"));
        }
    }
    if let Some(upstream_attempt) = attempt.upstream_attempt {
        if let Some(max) = attempt.upstream_max_attempts {
            attempt_parts.push(format!("u={upstream_attempt}/{max}"));
        } else {
            attempt_parts.push(format!("u={upstream_attempt}"));
        }
    }
    if !attempt_parts.is_empty() {
        parts.push(attempt_parts.join(" "));
    }
    if attempt.skipped {
        parts.push("skipped".to_string());
    }
    if let Some(status_code) = attempt.status_code {
        parts.push(format!("status={status_code}"));
    }
    if let Some(error_class) = attempt.error_class.as_deref() {
        parts.push(format!("class={error_class}"));
    }
    if let Some(model) = attempt.model.as_deref() {
        parts.push(format!("model={model}"));
    }
    if let Some(ttfb_ms) = attempt.upstream_headers_ms {
        parts.push(format!("ttfb={}", format_duration_ms(ttfb_ms)));
    }
    if let Some(duration_ms) = attempt.duration_ms {
        parts.push(format!("dur={}", format_duration_ms(duration_ms)));
    }
    if let Some(cooldown_secs) = attempt.cooldown_secs {
        parts.push(format!("cooldown={cooldown_secs}s"));
    }
    if !attempt.avoid_for_station.is_empty() {
        let avoid = attempt
            .avoid_for_station
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("avoid=[{avoid}]"));
    } else if let Some(avoided_total) = attempt.avoided_total.filter(|value| *value > 0) {
        if let Some(total) = attempt.total_upstreams {
            parts.push(format!("avoided={avoided_total}/{total}"));
        } else {
            parts.push(format!("avoided={avoided_total}"));
        }
    }
    if let Some(reason) = attempt.reason.as_deref() {
        parts.push(format!("reason={}", shorten_middle(reason, 56)));
    }
    format!("{target}  {}", parts.join(" "))
}

fn request_upstream_host(request: &FinishedRequest) -> Option<String> {
    let raw = request.upstream_base_url.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    let after_scheme = raw.split_once("://").map(|(_, rest)| rest).unwrap_or(raw);
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn request_speed_rows(request: &FinishedRequest) -> Vec<(String, String)> {
    let observability = request.observability_view();
    let mut rows = vec![
        (
            "total".to_string(),
            observability
                .duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "first token".to_string(),
            observability
                .ttfb_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "generation".to_string(),
            observability
                .generation_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "streaming".to_string(),
            if observability.streaming { "yes" } else { "no" }.to_string(),
        ),
    ];
    if let Some(rate) = observability.output_tokens_per_second {
        rows.push(("out_tok/s".to_string(), format!("{rate:.1}")));
    }
    rows
}

fn request_token_rows(
    usage: &crate::usage::UsageMetrics,
    accounting: crate::usage::CacheInputAccounting,
) -> Vec<(String, String)> {
    let cache = usage.cache_usage_breakdown(accounting);
    let mut rows = vec![
        ("input".to_string(), usage.input_tokens.to_string()),
        ("output".to_string(), usage.output_tokens.to_string()),
        (
            "reasoning".to_string(),
            usage.reasoning_output_tokens_total().to_string(),
        ),
        ("total".to_string(), usage.total_tokens.to_string()),
    ];
    if usage.has_cache_tokens() {
        rows.push((
            "effective_input".to_string(),
            cache.effective_input_tokens.to_string(),
        ));
        rows.push((
            "cached_input".to_string(),
            usage.cached_input_tokens.to_string(),
        ));
        rows.push((
            "cache_read".to_string(),
            usage.cache_read_input_tokens.to_string(),
        ));
        rows.push((
            "cache_create".to_string(),
            usage.cache_creation_tokens_total().to_string(),
        ));
        if usage.cache_creation_5m_input_tokens > 0 || usage.cache_creation_1h_input_tokens > 0 {
            rows.push((
                "cache_create_5m".to_string(),
                usage.cache_creation_5m_input_tokens.to_string(),
            ));
            rows.push((
                "cache_create_1h".to_string(),
                usage.cache_creation_1h_input_tokens.to_string(),
            ));
        }
    }
    if let Some(rate) = usage.cache_hit_rate_with_accounting(accounting) {
        rows.push((
            "cache_hit_rate".to_string(),
            format!("{:.1}%", rate * 100.0),
        ));
    }
    rows
}

fn request_cost_rows(request: &FinishedRequest) -> Vec<(String, String)> {
    let cost = &request.cost;
    let mut rows = vec![("total".to_string(), cost.display_total_with_confidence())];
    rows.push((
        "input".to_string(),
        format_usd_option(cost.input_cost_usd.as_deref()),
    ));
    rows.push((
        "output".to_string(),
        format_usd_option(cost.output_cost_usd.as_deref()),
    ));
    rows.push((
        "cache_read".to_string(),
        format_usd_option(cost.cache_read_cost_usd.as_deref()),
    ));
    rows.push((
        "cache_create".to_string(),
        format_usd_option(cost.cache_creation_cost_usd.as_deref()),
    ));
    if let Some(multiplier) = cost.service_tier_multiplier.as_deref() {
        rows.push(("tier_mult".to_string(), multiplier.to_string()));
    }
    if let Some(multiplier) = cost.provider_cost_multiplier.as_deref() {
        rows.push(("provider_mult".to_string(), multiplier.to_string()));
    }
    rows.push((
        "source".to_string(),
        cost.pricing_source
            .clone()
            .unwrap_or_else(|| "-".to_string()),
    ));
    rows
}

fn format_usd_option(value: Option<&str>) -> String {
    value
        .map(|value| format!("${value}"))
        .unwrap_or_else(|| "-".to_string())
}

pub(in super::super) fn request_output_tok_per_sec(request: &FinishedRequest) -> Option<f64> {
    request.output_tokens_per_second()
}

fn request_observed_route_value(
    request: &FinishedRequest,
    field: EffectiveRouteField,
    lang: Language,
) -> String {
    match field {
        EffectiveRouteField::Model => request.model.clone().unwrap_or_else(|| "-".to_string()),
        EffectiveRouteField::Station => request
            .station_name
            .clone()
            .unwrap_or_else(|| "-".to_string()),
        EffectiveRouteField::Upstream => request
            .upstream_base_url
            .as_deref()
            .map(|value| shorten_middle(value, 72))
            .unwrap_or_else(|| "-".to_string()),
        EffectiveRouteField::Effort => request
            .reasoning_effort
            .clone()
            .unwrap_or_else(|| "-".to_string()),
        EffectiveRouteField::ServiceTier => {
            request_service_tier_display(request.service_tier.as_deref(), lang)
        }
    }
}

fn request_observed_route_raw(
    request: &FinishedRequest,
    field: EffectiveRouteField,
) -> Option<String> {
    match field {
        EffectiveRouteField::Model => non_empty_trimmed(request.model.as_deref()),
        EffectiveRouteField::Station => non_empty_trimmed(request.station_name.as_deref()),
        EffectiveRouteField::Upstream => non_empty_trimmed(request.upstream_base_url.as_deref()),
        EffectiveRouteField::Effort => non_empty_trimmed(request.reasoning_effort.as_deref()),
        EffectiveRouteField::ServiceTier => non_empty_trimmed(request.service_tier.as_deref()),
    }
}

fn request_binding_reference(decision: &RouteDecisionProvenance, lang: Language) -> String {
    match decision.binding_profile_name.as_deref() {
        Some(name) => format!("profile {name}"),
        None => pick(lang, "当前会话绑定", "the current session binding").to_string(),
    }
}

fn request_route_decision_changed_fields(
    request: &FinishedRequest,
    decision: &RouteDecisionProvenance,
    lang: Language,
) -> Vec<String> {
    EffectiveRouteField::ALL
        .into_iter()
        .filter(|field| {
            let decided =
                route_decision_field_value(decision, *field).map(|value| value.value.as_str());
            let observed = request_observed_route_raw(request, *field);
            match (decided, observed.as_deref()) {
                (Some(decided), Some(observed)) => decided != observed,
                _ => false,
            }
        })
        .map(|field| effective_route_field_label(field, lang).to_string())
        .collect()
}

fn request_fast_mode_note(
    request: &FinishedRequest,
    decision: Option<&RouteDecisionProvenance>,
    lang: Language,
) -> Option<String> {
    let decided = decision.and_then(|decision| decision.effective_service_tier.as_ref());
    if let Some(value) = decided
        && value.value.eq_ignore_ascii_case("priority")
    {
        return Some(format!(
            "{}: service_tier=priority，{} [{}].",
            pick(lang, "fast mode", "Fast mode"),
            pick(
                lang,
                "这次请求是按快速模式路由的",
                "this request was routed in fast mode"
            ),
            route_value_source_label(value.source, lang)
        ));
    }
    request
        .service_tier
        .as_deref()
        .filter(|value| value.trim().eq_ignore_ascii_case("priority"))
        .map(|_| {
            format!(
                "{}: service_tier=priority，{}。",
                pick(lang, "fast mode", "Fast mode"),
                pick(
                    lang,
                    "当前观测到快速模式，但缺少来源快照",
                    "fast mode is visible in the observed request, but source provenance is missing",
                )
            )
        })
}

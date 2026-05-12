use crate::dashboard_core::{
    StationRetryBoundary, StationRoutingCandidate, StationRoutingMode, StationRoutingPosture,
    StationRoutingPostureInput, StationRoutingSkipReason, StationRoutingSource,
    build_station_routing_posture,
};

use super::*;

pub(super) fn render_stations_routing_preview(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    configured_active_station: Option<&str>,
) {
    let posture =
        build_gui_station_routing_posture(snapshot, runtime_maps, configured_active_station);

    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "自动切换解释",
            "Auto-switch explanation",
        ));
        ui.small(pick(
            ctx.lang,
            "按当前运行态解释新请求会如何选择 station、哪些目标被排除，以及 retry 何时允许跨站。具体会话仍会先应用 session pin / profile binding。",
            "Explain how new requests choose stations under the current runtime, which targets are excluded, and when retry may cross station boundaries. Concrete sessions still apply session pins and profile bindings first.",
        ));
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "来源", "Source"),
                format_routing_source(ctx.lang, &posture.source)
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "模式", "Mode"),
                format_routing_mode(ctx.lang, posture.mode)
            ));
        });
        ui.small(format_retry_boundary(ctx.lang, posture.retry_boundary));
        if let Some(summary) = format_recent_switch_observations(ctx.lang, snapshot) {
            ui.small(summary);
        }
        if let Some(note) = session_pin_note(ctx.lang, posture.session_pin_count) {
            ui.colored_label(egui::Color32::from_rgb(200, 120, 40), note);
        }

        ui.add_space(4.0);
        ui.label(pick(ctx.lang, "候选顺序", "Candidate order"));
        ui.small(format_routing_order_hint(ctx.lang, posture.mode));
        if posture.eligible_candidates.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(ctx.lang, "<无可用候选>", "<no eligible candidates>"),
            );
        } else {
            for (index, candidate) in posture.eligible_candidates.iter().enumerate() {
                ui.small(format!(
                    "{}. {}",
                    index + 1,
                    format_routing_candidate(ctx.lang, candidate)
                ));
            }
            if posture.mode == StationRoutingMode::PinnedStation {
                ui.small(pick(
                    ctx.lang,
                    "pin 模式下 disabled/draining/half-open 仍可被固定路由使用；breaker_open 和无上游会阻断。",
                    "In pin mode, disabled/draining/half-open stations remain usable for pinned routing; breaker_open and empty upstream pools block it.",
                ));
            }
        }

        if !posture.skipped.is_empty() {
            ui.add_space(4.0);
            ui.label(pick(ctx.lang, "跳过原因", "Skipped"));
            for item in &posture.skipped {
                ui.small(format!(
                    "{}: {}",
                    item.station_name,
                    item.reasons
                        .iter()
                        .map(|reason| format_skip_reason(ctx.lang, reason))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        ui.add_space(6.0);
        if let Some(explain) = snapshot.routing_explain.as_ref() {
            ui.label(pick(ctx.lang, "运行态选路", "Runtime route"));
            ui.small(format_runtime_selected_route(ctx.lang, explain));
            for candidate in &explain.candidates {
                ui.small(format_runtime_candidate(candidate));
            }
        } else if snapshot.supports_routing_explain_api {
            ui.small(pick(
                ctx.lang,
                "运行态 explain API 暂无可用快照。",
                "Runtime explain API has no available snapshot yet.",
            ));
        }
    });
}

fn build_gui_station_routing_posture(
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    configured_active_station: Option<&str>,
) -> StationRoutingPosture {
    let stations = snapshot
        .stations
        .iter()
        .map(|station| {
            StationRoutingCandidate::from_station_option(
                station,
                configured_active_station,
                runtime_maps.lb_view.get(station.name.as_str()),
                runtime_maps
                    .provider_balances
                    .get(station.name.as_str())
                    .map(Vec::as_slice),
            )
        })
        .collect::<Vec<_>>();

    build_station_routing_posture(StationRoutingPostureInput {
        stations: &stations,
        session_station_override: None,
        global_station_override: snapshot.global_station_override.as_deref(),
        configured_active_station,
        session_pin_count: snapshot.session_station_overrides.len(),
        retry: snapshot.resolved_retry.as_ref(),
    })
}

fn format_routing_source(lang: Language, source: &StationRoutingSource) -> String {
    match source {
        StationRoutingSource::SessionPin(station) => format!("session pin={station}"),
        StationRoutingSource::GlobalPin(station) => format!("global pin={station}"),
        StationRoutingSource::ConfiguredActiveStation(station) => {
            format!("configured active_station={station}")
        }
        StationRoutingSource::Auto => pick(
            lang,
            "auto / no configured active",
            "auto / no configured active",
        )
        .to_string(),
    }
}

fn format_routing_mode(lang: Language, mode: StationRoutingMode) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => pick(lang, "pinned station", "pinned station"),
        StationRoutingMode::AutoLevelFallback => {
            pick(lang, "auto / level fallback", "auto / level fallback")
        }
        StationRoutingMode::AutoSingleLevelFallback => pick(
            lang,
            "auto / single-level fallback",
            "auto / single-level fallback",
        ),
    }
}

fn format_retry_boundary(lang: Language, boundary: StationRetryBoundary) -> String {
    match boundary {
        StationRetryBoundary::Unknown => pick(
            lang,
            "retry: resolved policy 暂不可见；跨站边界未知。",
            "retry: resolved policy is not visible yet; cross-station boundaries are unknown.",
        )
        .to_string(),
        StationRetryBoundary::CrossStationBeforeFirstOutput {
            provider_max_attempts,
        } => match lang {
            Language::Zh => format!(
                "retry: provider failover x{provider_max_attempts}；首包前可按候选顺序跨 station，首包后固定在当前 station。"
            ),
            Language::En => format!(
                "retry: provider failover x{provider_max_attempts}; may cross stations in candidate order before first output, then stays on the current station."
            ),
        },
        StationRetryBoundary::CurrentStationFirst {
            provider_strategy,
            provider_max_attempts,
        } => match lang {
            Language::Zh => format!(
                "retry: provider {} x{provider_max_attempts}；当前策略不允许首包前跨 station，失败会先留在已选 station 内。",
                retry_strategy_label(provider_strategy)
            ),
            Language::En => format!(
                "retry: provider {} x{provider_max_attempts}; cross-station failover before first output is disabled, so failures stay inside the selected station first.",
                retry_strategy_label(provider_strategy)
            ),
        },
        StationRetryBoundary::NextRequestOnly => pick(
            lang,
            "retry: provider 只有一次尝试；自动切换主要依赖下一次请求重新选路。",
            "retry: provider has one attempt; automatic switching mainly happens when the next request is routed.",
        )
        .to_string(),
    }
}

fn format_recent_switch_observations(
    lang: Language,
    snapshot: &GuiRuntimeSnapshot,
) -> Option<String> {
    snapshot.operator_retry_summary.as_ref().map(|summary| {
        format!(
            "{}: retry={}  {}={}  {}={}  fast={}",
            pick(lang, "最近观测", "Recent observations"),
            summary.recent_retried_requests,
            pick(lang, "同站", "same_station"),
            summary.recent_same_station_retries,
            pick(lang, "跨站", "cross_station"),
            summary.recent_cross_station_failovers,
            summary.recent_fast_mode_requests,
        )
    })
}

fn format_routing_order_hint(lang: Language, mode: StationRoutingMode) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => pick(
            lang,
            "排序规则：pin 模式只看固定目标；level / active / 余额不会重新排序，breaker_open 和无上游会阻断。",
            "Order rule: pinned mode only uses the pinned target; level / active / balance do not reorder it, while breaker_open and empty upstream pools block it.",
        ),
        StationRoutingMode::AutoLevelFallback => pick(
            lang,
            "排序规则：默认会把已知全耗尽的 station 降级；可配置的 provider 例外会只显示余额不参与路由。其余先按 level，小 level 优先；同级再优先 active。",
            "Order rule: known fully exhausted stations are demoted by default, while provider-level exceptions only show balance but do not affect routing. The rest prefer lower level, then active within the same level.",
        ),
        StationRoutingMode::AutoSingleLevelFallback => pick(
            lang,
            "排序规则：所有候选同级时，默认会把已知全耗尽的 station 降级；provider 可关闭这条信任。其余优先 active，再按名称稳定排序。",
            "Order rule: with one level, known fully exhausted stations are demoted by default unless a provider opts out of routing trust. The rest prefer active, then stable name order.",
        ),
    }
}

fn format_routing_candidate(lang: Language, candidate: &StationRoutingCandidate) -> String {
    let mut parts = vec![format!("L{}", candidate.level.clamp(1, 10))];
    if candidate.active {
        parts.push("active".to_string());
    }
    match candidate.upstreams {
        Some(upstreams) => parts.push(format!("upstreams={upstreams}")),
        None => parts.push("upstreams=?".to_string()),
    }
    if candidate.runtime_state != RuntimeConfigState::Normal {
        parts.push(format!(
            "state={}",
            runtime_config_state_label(lang, candidate.runtime_state)
        ));
    }
    if candidate.has_cooldown {
        parts.push("cooldown".to_string());
    }
    if candidate.all_usage_exhausted {
        parts.push("usage=exhausted(all)".to_string());
    } else if candidate.any_usage_exhausted {
        parts.push("usage=exhausted(partial)".to_string());
    }
    if !candidate.balance.is_empty() {
        parts.push(format_routing_balance(candidate));
    }

    match candidate
        .alias
        .as_deref()
        .filter(|alias| !alias.trim().is_empty())
    {
        Some(alias) => format!("{} ({alias}) [{}]", candidate.name, parts.join(", ")),
        None => format!("{} [{}]", candidate.name, parts.join(", ")),
    }
}

fn format_routing_balance(candidate: &StationRoutingCandidate) -> String {
    let balance = &candidate.balance;
    let mut parts = Vec::new();
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            parts.push("exhausted(untrusted)".to_string());
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        parts.push("exhausted(all)".to_string());
    } else if balance.routing_exhausted > 0 {
        parts.push(format!(
            "exhausted={}/{}",
            balance.routing_exhausted, balance.routing_snapshots
        ));
    }
    if balance.routing_ignored_exhausted > 0 {
        parts.push(format!(
            "ignored_for_routing={}",
            balance.routing_ignored_exhausted
        ));
    }
    if balance.stale > 0 {
        parts.push(format!("stale={}", balance.stale));
    }
    let unknown = balance.unknown + balance.error;
    if unknown > 0 {
        parts.push(format!("unknown={unknown}"));
    }
    if parts.is_empty() {
        format!("balance=ok({})", balance.snapshots)
    } else {
        format!("balance={}", parts.join("/"))
    }
}

fn format_skip_reason(lang: Language, reason: &StationRoutingSkipReason) -> String {
    match reason {
        StationRoutingSkipReason::Disabled => pick(lang, "disabled", "disabled").to_string(),
        StationRoutingSkipReason::RuntimeState(state) => match lang {
            Language::Zh => format!(
                "state={} 不参与自动路由",
                runtime_config_state_label(lang, *state)
            ),
            Language::En => format!(
                "state={} is not eligible for automatic routing",
                runtime_config_state_label(lang, *state)
            ),
        },
        StationRoutingSkipReason::NoRoutableUpstreams => {
            pick(lang, "no routable upstreams", "no routable upstreams").to_string()
        }
        StationRoutingSkipReason::MissingPinnedTarget => pick(
            lang,
            "pinned target is not in the current station list",
            "pinned target is not in the current station list",
        )
        .to_string(),
        StationRoutingSkipReason::BreakerOpenBlocksPinned => pick(
            lang,
            "breaker_open blocks pinned routing",
            "breaker_open blocks pinned routing",
        )
        .to_string(),
    }
}

fn format_runtime_selected_route(
    lang: Language,
    explain: &crate::routing_explain::RoutingExplainResponse,
) -> String {
    match explain.selected_route.as_ref() {
        Some(selected) => match lang {
            Language::Zh => format!(
                "selected={} endpoint={} station={} upstream#{} path={}",
                selected.provider_id,
                selected.endpoint_id,
                selected.station_name,
                selected.upstream_index,
                selected.route_path.join(" > ")
            ),
            Language::En => format!(
                "selected={} endpoint={} station={} upstream#{} path={}",
                selected.provider_id,
                selected.endpoint_id,
                selected.station_name,
                selected.upstream_index,
                selected.route_path.join(" > ")
            ),
        },
        None => pick(lang, "selected=<none>", "selected=<none>").to_string(),
    }
}

fn format_runtime_candidate(candidate: &crate::routing_explain::RoutingExplainCandidate) -> String {
    let marker = if candidate.selected { "*" } else { " " };
    format!(
        "{} {} endpoint={} station={} upstream#{} skip={} path={}",
        marker,
        candidate.provider_id,
        candidate.endpoint_id,
        candidate.station_name,
        candidate.upstream_index,
        format_runtime_skip_reasons(&candidate.skip_reasons),
        candidate.route_path.join(" > ")
    )
}

fn format_runtime_skip_reasons(
    reasons: &[crate::routing_explain::RoutingExplainSkipReason],
) -> String {
    if reasons.is_empty() {
        return "-".to_string();
    }
    reasons
        .iter()
        .map(crate::routing_explain::RoutingExplainSkipReason::code)
        .collect::<Vec<_>>()
        .join(",")
}

fn session_pin_note(lang: Language, session_pin_count: usize) -> Option<String> {
    (session_pin_count > 0).then(|| match lang {
        Language::Zh => format!(
            "{session_pin_count} 个会话有 station pin；这些会话会先使用自己的 pin，再看 global/auto 策略。"
        ),
        Language::En => format!(
            "{session_pin_count} sessions have station pins; those sessions use their own pins before global/auto policy."
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_boundary_text_explains_cross_station_window() {
        let text = format_retry_boundary(
            Language::En,
            StationRetryBoundary::CrossStationBeforeFirstOutput {
                provider_max_attempts: 3,
            },
        );

        assert!(text.contains("before first output"));
        assert!(text.contains("stays on the current station"));
    }

    #[test]
    fn routing_order_hint_explains_level_priority() {
        let text = format_routing_order_hint(Language::En, StationRoutingMode::AutoLevelFallback);

        assert!(text.contains("demoted by default"));
        assert!(text.contains("provider-level exceptions"));
        assert!(text.contains("prefer lower level"));
    }

    #[test]
    fn routing_order_hint_explains_same_level_balance_and_active_priority() {
        let text =
            format_routing_order_hint(Language::En, StationRoutingMode::AutoSingleLevelFallback);

        assert!(text.contains("demoted by default"));
        assert!(text.contains("provider opts out of routing trust"));
        assert!(text.contains("prefer active"));
    }

    #[test]
    fn routing_order_hint_explains_pinned_balance_boundary() {
        let text = format_routing_order_hint(Language::En, StationRoutingMode::PinnedStation);

        assert!(text.contains("pinned target"));
        assert!(text.contains("balance do not reorder"));
    }

    #[test]
    fn candidate_label_marks_runtime_warnings() {
        let label = format_routing_candidate(
            Language::En,
            &StationRoutingCandidate {
                name: "alpha".to_string(),
                alias: Some("Alpha".to_string()),
                level: 1,
                enabled: true,
                active: true,
                upstreams: Some(2),
                runtime_state: RuntimeConfigState::HalfOpen,
                has_cooldown: true,
                any_usage_exhausted: true,
                all_usage_exhausted: false,
                balance: crate::dashboard_core::StationRoutingBalanceSummary::default(),
            },
        );

        assert!(label.contains("alpha (Alpha)"));
        assert!(label.contains("state=half_open"));
        assert!(label.contains("cooldown"));
        assert!(label.contains("usage=exhausted(partial)"));
    }

    #[test]
    fn candidate_label_marks_balance_warnings() {
        let label = format_routing_candidate(
            Language::En,
            &StationRoutingCandidate {
                name: "alpha".to_string(),
                alias: None,
                level: 1,
                enabled: true,
                active: false,
                upstreams: Some(2),
                runtime_state: RuntimeConfigState::Normal,
                has_cooldown: false,
                any_usage_exhausted: false,
                all_usage_exhausted: false,
                balance: crate::dashboard_core::StationRoutingBalanceSummary {
                    snapshots: 2,
                    ok: 0,
                    exhausted: 1,
                    stale: 1,
                    error: 0,
                    unknown: 0,
                    routing_snapshots: 2,
                    routing_exhausted: 1,
                    routing_ignored_exhausted: 0,
                },
            },
        );

        assert!(label.contains("balance=exhausted=1/2/stale=1"));
    }

    #[test]
    fn candidate_label_does_not_treat_unknown_balance_as_ok() {
        let label = format_routing_candidate(
            Language::En,
            &StationRoutingCandidate {
                name: "alpha".to_string(),
                alias: None,
                level: 1,
                enabled: true,
                active: false,
                upstreams: Some(1),
                runtime_state: RuntimeConfigState::Normal,
                has_cooldown: false,
                any_usage_exhausted: false,
                all_usage_exhausted: false,
                balance: crate::dashboard_core::StationRoutingBalanceSummary {
                    snapshots: 1,
                    ok: 0,
                    exhausted: 0,
                    stale: 0,
                    error: 0,
                    unknown: 1,
                    routing_snapshots: 1,
                    routing_exhausted: 0,
                    routing_ignored_exhausted: 0,
                },
            },
        );

        assert!(label.contains("balance=unknown=1"));
        assert!(!label.contains("balance=ok"));
    }

    #[test]
    fn candidate_label_marks_ignored_routing_exhaustion() {
        let label = format_routing_candidate(
            Language::En,
            &StationRoutingCandidate {
                name: "alpha".to_string(),
                alias: None,
                level: 1,
                enabled: true,
                active: false,
                upstreams: Some(1),
                runtime_state: RuntimeConfigState::Normal,
                has_cooldown: false,
                any_usage_exhausted: false,
                all_usage_exhausted: false,
                balance: crate::dashboard_core::StationRoutingBalanceSummary {
                    snapshots: 1,
                    ok: 0,
                    exhausted: 1,
                    stale: 0,
                    error: 0,
                    unknown: 0,
                    routing_snapshots: 0,
                    routing_exhausted: 0,
                    routing_ignored_exhausted: 1,
                },
            },
        );

        assert!(label.contains("exhausted(untrusted)"));
        assert!(label.contains("ignored_for_routing=1"));
    }
}

use super::*;

#[derive(Debug, Clone, PartialEq)]
struct StationRelatedRequestView<'a> {
    request: &'a FinishedRequest,
    touched_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct StationProviderHitSummary {
    provider: String,
    requests: usize,
    errors: usize,
    retry_requests: usize,
    attempts_total: u64,
}

impl StationProviderHitSummary {
    fn avg_attempts(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.attempts_total as f64 / self.requests as f64
        }
    }
}

fn retry_chain_mentions_station(retry: &crate::logging::RetryInfo, station_name: &str) -> bool {
    retry.touches_station(station_name)
}

fn station_related_recent_requests<'a>(
    recent: &'a [FinishedRequest],
    station_name: &str,
) -> Vec<StationRelatedRequestView<'a>> {
    recent
        .iter()
        .filter_map(|request| {
            if request.station_name.as_deref() == Some(station_name) {
                return Some(StationRelatedRequestView {
                    request,
                    touched_only: false,
                });
            }

            request
                .retry
                .as_ref()
                .filter(|retry| retry_chain_mentions_station(retry, station_name))
                .map(|_| StationRelatedRequestView {
                    request,
                    touched_only: true,
                })
        })
        .collect()
}

fn station_provider_hit_summaries(
    related_requests: &[StationRelatedRequestView<'_>],
) -> Vec<StationProviderHitSummary> {
    let mut grouped = BTreeMap::<String, StationProviderHitSummary>::new();

    for related in related_requests
        .iter()
        .filter(|related| !related.touched_only)
    {
        let request = related.request;
        let key = request.provider_id.as_deref().unwrap_or("-").to_string();
        let entry = grouped
            .entry(key.clone())
            .or_insert_with(|| StationProviderHitSummary {
                provider: key,
                requests: 0,
                errors: 0,
                retry_requests: 0,
                attempts_total: 0,
            });
        entry.requests += 1;
        if request.status_code >= 400 {
            entry.errors += 1;
        }
        let attempts = request.attempt_count();
        if attempts > 1 {
            entry.retry_requests += 1;
        }
        entry.attempts_total = entry.attempts_total.saturating_add(attempts as u64);
    }

    let mut rows = grouped.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.requests
            .cmp(&a.requests)
            .then_with(|| a.provider.cmp(&b.provider))
    });
    rows
}

fn related_request_outcome_label(
    request: &FinishedRequest,
    station_name: &str,
    touched_only: bool,
    lang: Language,
) -> String {
    if touched_only {
        format!(
            "{} {}",
            pick(
                lang,
                "曾触碰该站点，最终切到",
                "Touched this station, then failed over to",
            ),
            request.station_name.as_deref().unwrap_or("-"),
        )
    } else if request
        .retry
        .as_ref()
        .is_some_and(|retry| retry_chain_mentions_station(retry, station_name))
        && request.attempt_count() > 1
    {
        pick(
            lang,
            "最终命中该站点（经历重试/切换）",
            "Final hit on this station after retry/failover",
        )
        .to_string()
    } else {
        pick(lang, "最终命中该站点", "Final hit on this station").to_string()
    }
}

fn retry_chain_brief(retry: &crate::logging::RetryInfo) -> String {
    let attempts = retry.route_attempts_or_derived();
    if attempts.is_empty() {
        return "-".to_string();
    }
    let max = 2usize;
    if attempts.len() <= max {
        return attempts
            .iter()
            .map(route_attempt_brief)
            .collect::<Vec<_>>()
            .join(" -> ");
    }
    format!(
        "{} -> ... -> {}",
        route_attempt_brief(&attempts[0]),
        attempts
            .last()
            .map(route_attempt_brief)
            .unwrap_or_else(|| "-".to_string())
    )
}

fn route_attempt_brief(attempt: &crate::logging::RouteAttemptLog) -> String {
    let target = match (
        attempt.station_name.as_deref(),
        attempt.upstream_base_url.as_deref(),
    ) {
        (Some(station), Some(upstream)) => {
            format!("{station}:{}", summarize_upstream_target(upstream, 44))
        }
        (Some(station), None) => station.to_string(),
        (None, Some(upstream)) => summarize_upstream_target(upstream, 48),
        (None, None) => "-".to_string(),
    };
    let mut parts = vec![target, attempt.decision.clone()];
    if let Some(provider_id) = attempt.provider_id.as_deref() {
        parts.push(format!("provider={}", shorten_middle(provider_id, 24)));
    }
    if let Some(provider_attempt) = attempt.provider_attempt {
        if let Some(max) = attempt.provider_max_attempts {
            parts.push(format!("p={provider_attempt}/{max}"));
        } else {
            parts.push(format!("p={provider_attempt}"));
        }
    }
    if let Some(upstream_attempt) = attempt.upstream_attempt {
        if let Some(max) = attempt.upstream_max_attempts {
            parts.push(format!("u={upstream_attempt}/{max}"));
        } else {
            parts.push(format!("u={upstream_attempt}"));
        }
    }
    if let Some(status_code) = attempt.status_code {
        parts.push(format!("st={status_code}"));
    }
    if let Some(duration_ms) = attempt.duration_ms {
        parts.push(format!("dur={duration_ms}ms"));
    }
    if let Some(cooldown_secs) = attempt.cooldown_secs {
        parts.push(format!("cd={cooldown_secs}s"));
    }
    if let Some(error_class) = attempt.error_class.as_deref() {
        parts.push(format!("class={error_class}"));
    } else if let Some(reason) = attempt.reason.as_deref() {
        parts.push(shorten_middle(reason, 48));
    }
    parts.join(" ")
}

pub(super) fn render_station_recent_hits_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    cfg: &StationOption,
    snapshot: &GuiRuntimeSnapshot,
) {
    ui.label(pick(
        ctx.lang,
        "最近命中 / 失败切换",
        "Recent hits / failover",
    ));
    ui.small(pick(
        ctx.lang,
        "这里把最近请求与 retry/failover 链连起来看：既展示最终命中该站点的请求，也展示曾触碰该站点但最终切到别处的请求。",
        "This section links recent requests with retry/failover chains: it shows both requests that finally hit this station and requests that touched it before failing over elsewhere.",
    ));

    let related_requests = station_related_recent_requests(&snapshot.recent, cfg.name.as_str());
    if related_requests.is_empty() {
        ui.small(pick(
            ctx.lang,
            "最近没有与该站点相关的请求命中记录。",
            "No recent request hits are related to this station.",
        ));
        return;
    }

    let provider_summaries = station_provider_hit_summaries(&related_requests);
    if !provider_summaries.is_empty() {
        ui.small(pick(
            ctx.lang,
            "最终命中该站点的 provider 摘要：",
            "Provider summary for final hits on this station:",
        ));
        for summary in provider_summaries.iter().take(6) {
            ui.small(format!(
                "{}  n={}  err={}  retry={}  avg_attempts={:.1}",
                summary.provider,
                summary.requests,
                summary.errors,
                summary.retry_requests,
                summary.avg_attempts()
            ));
        }
    }

    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .id_salt(("stations_recent_hits_scroll", cfg.name.as_str()))
        .max_height(200.0)
        .show(ui, |ui| {
            let now = now_ms();
            for related in related_requests.iter().take(8) {
                let request = related.request;
                let attempts = request.attempt_count();
                let upstream = request
                    .upstream_base_url
                    .as_deref()
                    .map(|value| summarize_upstream_target(value, 56))
                    .unwrap_or_else(|| "-".to_string());
                let tier =
                    format_service_tier_display(request.service_tier.as_deref(), ctx.lang, "-");
                let outcome = related_request_outcome_label(
                    request,
                    cfg.name.as_str(),
                    related.touched_only,
                    ctx.lang,
                );
                let color = if request.status_code >= 400 {
                    egui::Color32::from_rgb(200, 120, 40)
                } else {
                    egui::Color32::from_rgb(120, 120, 120)
                };

                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(
                            color,
                            format!(
                                "{}  st={}  {}ms  att={}  {}",
                                format_age(now, Some(request.ended_at_ms)),
                                request.status_code,
                                request.duration_ms,
                                attempts,
                                outcome
                            ),
                        );
                    });
                    ui.small(format!(
                        "provider={}  final_station={}  upstream={}  service_tier={}",
                        request.provider_id.as_deref().unwrap_or("-"),
                        request.station_name.as_deref().unwrap_or("-"),
                        upstream,
                        tier
                    ));
                    if let Some(retry) = request.retry.as_ref()
                        && request.attempt_count() > 1
                    {
                        ui.small(format!(
                            "{}: {}",
                            pick(ctx.lang, "retry chain", "Retry chain"),
                            retry_chain_brief(retry)
                        ));
                    }
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(
        station_name: &str,
        provider_id: &str,
        retry: Option<crate::logging::RetryInfo>,
    ) -> FinishedRequest {
        FinishedRequest {
            id: 1,
            trace_id: Some("codex-1".to_string()),
            session_id: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            station_name: Some(station_name.to_string()),
            provider_id: Some(provider_id.to_string()),
            upstream_base_url: Some("https://api.example.com/v1".to_string()),
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry,
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 500,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1_000,
        }
    }

    #[test]
    fn retry_chain_mentions_station_matches_station_prefix_only() {
        let retry = crate::logging::RetryInfo {
            attempts: 2,
            upstream_chain: vec![
                "right:https://api.right.example/v1 (idx=0) transport_error=timeout".to_string(),
                "https://api.vibe.example/v1 (idx=1) status=200 class=-".to_string(),
            ],
            route_attempts: Vec::new(),
        };

        assert!(retry_chain_mentions_station(&retry, "right"));
        assert!(!retry_chain_mentions_station(&retry, "vibe"));
    }

    #[test]
    fn station_related_recent_requests_includes_touched_failover_records() {
        let retry = crate::logging::RetryInfo {
            attempts: 2,
            upstream_chain: vec![
                "right:https://api.right.example/v1 (idx=0) transport_error=timeout".to_string(),
                "https://api.vibe.example/v1 (idx=1) status=200 class=-".to_string(),
            ],
            route_attempts: Vec::new(),
        };
        let recent = vec![
            sample_request("right", "right", None),
            sample_request("vibe", "vibe", Some(retry)),
        ];

        let related = station_related_recent_requests(&recent, "right");

        assert_eq!(related.len(), 2);
        assert!(!related[0].touched_only);
        assert!(related[1].touched_only);
    }

    #[test]
    fn station_provider_hit_summaries_only_count_final_hits() {
        let retry = crate::logging::RetryInfo {
            attempts: 2,
            upstream_chain: vec![
                "right:https://api.right.example/v1 (idx=0) transport_error=timeout".to_string(),
                "https://api.vibe.example/v1 (idx=1) status=200 class=-".to_string(),
            ],
            route_attempts: Vec::new(),
        };
        let right = sample_request("right", "right", None);
        let vibe = sample_request("vibe", "vibe", Some(retry));
        let recent = vec![
            StationRelatedRequestView {
                request: &right,
                touched_only: false,
            },
            StationRelatedRequestView {
                request: &vibe,
                touched_only: true,
            },
        ];

        let summaries = station_provider_hit_summaries(&recent);

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].provider, "right");
        assert_eq!(summaries[0].requests, 1);
    }
}

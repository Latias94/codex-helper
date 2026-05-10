use super::*;

pub(super) fn format_runtime_station_health_status(
    health: Option<&StationHealth>,
    status: Option<&HealthCheckStatus>,
) -> String {
    if let Some(status) = status {
        if !status.done {
            return if status.cancel_requested {
                format!("cancel {}/{}", status.completed, status.total.max(1))
            } else {
                format!("run {}/{}", status.completed, status.total.max(1))
            };
        }
        if status.canceled {
            return "canceled".to_string();
        }
    }

    let Some(health) = health else {
        return "-".to_string();
    };
    if health.upstreams.is_empty() {
        return format!("0/0 @{}", health.checked_at_ms);
    }
    let ok = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .count();
    let best_ms = health
        .upstreams
        .iter()
        .filter(|upstream| upstream.ok == Some(true))
        .filter_map(|upstream| upstream.latency_ms)
        .min();
    if ok > 0 {
        if let Some(latency_ms) = best_ms {
            format!(
                "{ok}/{} {}",
                health.upstreams.len(),
                format_duration_ms(latency_ms)
            )
        } else {
            format!("{ok}/{} ok", health.upstreams.len())
        }
    } else {
        let code = health
            .upstreams
            .iter()
            .filter_map(|upstream| upstream.status_code)
            .next();
        match code {
            Some(code) => format!("err {code}"),
            None => "err".to_string(),
        }
    }
}

pub(super) fn format_runtime_lb_summary(lb: Option<&LbConfigView>) -> String {
    let Some(lb) = lb else {
        return "-".to_string();
    };
    let cooldowns = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.cooldown_remaining_secs.is_some())
        .count();
    let exhausted = lb
        .upstreams
        .iter()
        .filter(|upstream| upstream.usage_exhausted)
        .count();
    let failures: u32 = lb
        .upstreams
        .iter()
        .map(|upstream| upstream.failure_count)
        .sum();

    if cooldowns == 0 && exhausted == 0 && failures == 0 {
        return "-".to_string();
    }

    format!("cd={cooldowns} fail={failures} quota={exhausted}")
}

pub(super) fn runtime_config_state_label(
    lang: Language,
    state: RuntimeConfigState,
) -> &'static str {
    match (lang, state) {
        (Language::Zh, RuntimeConfigState::Normal) => "normal",
        (Language::Zh, RuntimeConfigState::Draining) => "draining",
        (Language::Zh, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (Language::Zh, RuntimeConfigState::HalfOpen) => "half_open",
        (_, RuntimeConfigState::Normal) => "normal",
        (_, RuntimeConfigState::Draining) => "draining",
        (_, RuntimeConfigState::BreakerOpen) => "breaker_open",
        (_, RuntimeConfigState::HalfOpen) => "half_open",
    }
}

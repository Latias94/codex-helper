#[cfg(test)]
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Instant;

use ratatui::prelude::{Color, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::dashboard_core::{
    OperatorProviderBalanceSummary, OperatorProviderEndpointSummary, OperatorProviderSummary,
    OperatorReadData, OperatorRequestSummary, OperatorSessionSummary, WindowStats,
};
use crate::pricing::UsdAmount;
use crate::runtime_identity::ProviderEndpointKey;
#[cfg(test)]
use crate::state::SessionIdentityCard;
use crate::state::{
    BalanceSnapshotStatus, ProviderBalanceSnapshot, ResolvedRouteValue, RouteDecisionProvenance,
    SessionObservationScope, UsageDayView, UsageRollupView,
};
use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::state::RequestControlFilter;
use crate::usage::UsageMetrics;

pub type UpstreamSummary = OperatorProviderEndpointSummary;
pub type ProviderOption = OperatorProviderSummary;

#[derive(Debug, Clone, PartialEq)]
pub(in crate::tui) struct SessionRow {
    pub(in crate::tui) session_id: Option<String>,
    pub(in crate::tui) local_session_id: Option<String>,
    pub(in crate::tui) observation_scope: SessionObservationScope,
    pub(in crate::tui) host_local_transcript_path: Option<String>,
    pub(in crate::tui) last_client_name: Option<String>,
    pub(in crate::tui) last_client_addr: Option<String>,
    pub(in crate::tui) cwd: Option<String>,
    pub(in crate::tui) active_count: usize,
    pub(in crate::tui) active_started_at_ms_min: Option<u64>,
    pub(in crate::tui) active_last_method: Option<String>,
    pub(in crate::tui) active_last_path: Option<String>,
    pub(in crate::tui) last_status: Option<u16>,
    pub(in crate::tui) last_duration_ms: Option<u64>,
    pub(in crate::tui) last_ended_at_ms: Option<u64>,
    pub(in crate::tui) last_model: Option<String>,
    pub(in crate::tui) last_reasoning_effort: Option<String>,
    pub(in crate::tui) last_service_tier: Option<String>,
    pub(in crate::tui) last_provider_id: Option<String>,
    pub(in crate::tui) last_usage: Option<UsageMetrics>,
    pub(in crate::tui) total_usage: Option<UsageMetrics>,
    pub(in crate::tui) turns_total: Option<u64>,
    pub(in crate::tui) turns_with_usage: Option<u64>,
    pub(in crate::tui) last_output_tokens_per_second: Option<f64>,
    pub(in crate::tui) avg_output_tokens_per_second: Option<f64>,
    pub(in crate::tui) binding_profile_name: Option<String>,
    pub(in crate::tui) binding_continuity_mode: Option<crate::state::SessionContinuityMode>,
    pub(in crate::tui) last_route_decision: Option<RouteDecisionProvenance>,
    pub(in crate::tui) route_affinity: Option<SessionRouteAffinityView>,
    pub(in crate::tui) effective_model: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_reasoning_effort: Option<ResolvedRouteValue>,
    pub(in crate::tui) effective_service_tier: Option<ResolvedRouteValue>,
}

impl SessionRow {
    pub(in crate::tui) fn local_command_session_id(&self) -> Option<&str> {
        self.local_session_id.as_deref()
    }

    pub(in crate::tui) fn observed_provider_id(&self) -> Option<&str> {
        self.last_route_decision
            .as_ref()
            .and_then(|decision| non_empty(decision.provider_id.as_deref()?))
            .or_else(|| non_empty(self.last_provider_id.as_deref()?))
    }

    pub(in crate::tui) fn observed_endpoint_id(&self) -> Option<&str> {
        self.last_route_decision
            .as_ref()
            .and_then(|decision| non_empty(decision.endpoint_id.as_deref()?))
    }

    pub(in crate::tui) fn observed_upstream_origin(&self) -> Option<String> {
        self.last_route_decision
            .as_ref()
            .and_then(|decision| decision.effective_upstream_base_url.as_ref())
            .and_then(|value| sanitize_upstream_origin(&value.value))
            .or_else(|| {
                self.route_affinity
                    .as_ref()
                    .and_then(|affinity| sanitize_upstream_origin(&affinity.upstream_origin))
            })
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

pub(in crate::tui) fn sanitize_upstream_origin(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

fn sanitize_route_decision(decision: &RouteDecisionProvenance) -> RouteDecisionProvenance {
    let mut decision = decision.clone();
    decision.effective_upstream_base_url =
        decision
            .effective_upstream_base_url
            .as_ref()
            .and_then(|value| {
                Some(ResolvedRouteValue {
                    value: sanitize_upstream_origin(&value.value)?,
                    source: value.source,
                })
            });
    decision
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct SessionRouteAffinityView {
    pub(in crate::tui) provider_id: String,
    pub(in crate::tui) endpoint_id: String,
    pub(in crate::tui) upstream_origin: String,
    pub(in crate::tui) route_path: Vec<String>,
    pub(in crate::tui) last_selected_at_ms: u64,
    pub(in crate::tui) last_changed_at_ms: u64,
    pub(in crate::tui) change_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::tui) struct RequestAttemptControlEvidence {
    pub(in crate::tui) provider_signal_codes: Vec<String>,
    pub(in crate::tui) policy_action_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::tui) struct RequestControlEvidence {
    pub(in crate::tui) provider_signal_codes: Vec<String>,
    pub(in crate::tui) policy_action_codes: Vec<String>,
    pub(in crate::tui) route_attempts: HashMap<u32, RequestAttemptControlEvidence>,
}

impl RequestControlEvidence {
    fn has_provider_signals(&self) -> bool {
        !self.provider_signal_codes.is_empty()
            || self
                .route_attempts
                .values()
                .any(|attempt| !attempt.provider_signal_codes.is_empty())
    }

    fn has_policy_actions(&self) -> bool {
        !self.policy_action_codes.is_empty()
            || self
                .route_attempts
                .values()
                .any(|attempt| !attempt.policy_action_codes.is_empty())
    }
}

#[derive(Debug, Clone)]
pub(in crate::tui) struct Snapshot {
    pub(in crate::tui) rows: Vec<SessionRow>,
    pub(in crate::tui) recent: Vec<OperatorRequestSummary>,
    pub(in crate::tui) request_control_evidence: HashMap<u64, RequestControlEvidence>,
    pub(in crate::tui) usage_day: UsageDayView,
    #[allow(dead_code)]
    pub(in crate::tui) usage_rollup: UsageRollupView,
    pub(in crate::tui) provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub(in crate::tui) stats_5m: WindowStats,
    pub(in crate::tui) stats_1h: WindowStats,
    pub(in crate::tui) service_status: Option<crate::service_status::ServiceStatusSnapshot>,
    pub(in crate::tui) refreshed_at: Instant,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        }
    }
}

pub(in crate::tui) fn request_attempt_count(request: &OperatorRequestSummary) -> u32 {
    let retry_attempts = request
        .retry
        .as_ref()
        .map(|retry| retry.attempts)
        .unwrap_or(0);
    retry_attempts
        .max(request.observability.attempt_count)
        .max(1)
}

pub(in crate::tui) fn request_provider_endpoint(
    request: &OperatorRequestSummary,
) -> Option<ProviderEndpointKey> {
    Some(ProviderEndpointKey::new(
        non_empty(&request.service)?,
        non_empty(request.provider_id.as_deref()?)?,
        non_empty(request.endpoint_id.as_deref()?)?,
    ))
}

#[derive(Debug, Clone, Copy)]
pub(in crate::tui) struct Palette {
    pub(in crate::tui) bg: Color,
    pub(in crate::tui) panel: Color,
    pub(in crate::tui) border: Color,
    pub(in crate::tui) text: Color,
    pub(in crate::tui) muted: Color,
    pub(in crate::tui) accent: Color,
    pub(in crate::tui) focus: Color,
    pub(in crate::tui) good: Color,
    pub(in crate::tui) warn: Color,
    pub(in crate::tui) bad: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            bg: Color::Rgb(14, 17, 22),
            panel: Color::Rgb(18, 22, 28),
            border: Color::Rgb(54, 62, 74),
            text: Color::Rgb(224, 228, 234),
            muted: Color::Rgb(144, 154, 164),
            accent: Color::Rgb(88, 166, 255),
            focus: Color::Rgb(121, 192, 255),
            good: Color::Rgb(63, 185, 80),
            warn: Color::Rgb(210, 153, 34),
            bad: Color::Rgb(248, 81, 73),
        }
    }
}

pub(in crate::tui) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(in crate::tui) const CODEX_RECENT_WINDOWS: [(u64, &str); 6] = [
    (30 * 60, "30m"),
    (60 * 60, "1h"),
    (3 * 60 * 60, "3h"),
    (8 * 60 * 60, "8h"),
    (12 * 60 * 60, "12h"),
    (24 * 60 * 60, "24h"),
];

pub(in crate::tui) fn codex_recent_window_label(idx: usize) -> &'static str {
    CODEX_RECENT_WINDOWS[idx.min(CODEX_RECENT_WINDOWS.len() - 1)].1
}

pub(in crate::tui) fn codex_recent_window_threshold_ms(now_ms: u64, idx: usize) -> u64 {
    let secs = CODEX_RECENT_WINDOWS[idx.min(CODEX_RECENT_WINDOWS.len() - 1)].0;
    now_ms.saturating_sub(secs.saturating_mul(1000))
}

pub(in crate::tui) fn basename(path: &str) -> &str {
    let path = path.trim_end_matches(['/', '\\']);
    let slash = path.rfind('/');
    let backslash = path.rfind('\\');
    let idx = match (slash, backslash) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    if let Some(i) = idx {
        &path[i.saturating_add(1)..]
    } else {
        path
    }
}

pub(in crate::tui) fn shorten(s: &str, max: usize) -> String {
    shorten_head(s, max)
}

pub(in crate::tui) fn shorten_middle(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let remaining = max.saturating_sub(1);
    let head_w = remaining / 2;
    let tail_w = remaining.saturating_sub(head_w);
    let head = prefix_by_width(s, head_w);
    let tail = suffix_by_width(s, tail_w);
    format!("{head}…{tail}")
}

fn shorten_head(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "…".to_string();
    }
    let head = prefix_by_width(s, max.saturating_sub(1));
    format!("{head}…")
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn prefix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut width = 0usize;
    let mut end = 0usize;
    for (i, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(w) > max_width {
            break;
        }
        width = width.saturating_add(w);
        end = i.saturating_add(ch.len_utf8());
    }
    &s[..end]
}

fn suffix_by_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut width = 0usize;
    let mut start = s.len();
    for (i, ch) in s.char_indices().rev() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(w) > max_width {
            break;
        }
        width = width.saturating_add(w);
        start = i;
    }
    &s[start..]
}

pub(in crate::tui) fn short_sid(sid: &str, max: usize) -> String {
    // Prefer head truncation (end ellipsis) over middle truncation so the string stays readable
    // and copy/paste friendly in terminals.
    shorten_head(sid, max)
}

pub(in crate::tui) fn operator_provider_policy_action_count(provider: &ProviderOption) -> usize {
    provider
        .endpoints
        .iter()
        .map(|endpoint| endpoint.policy_actions.len())
        .sum()
}

pub(in crate::tui) fn balance_status_style(p: Palette, status: BalanceSnapshotStatus) -> Style {
    match status {
        BalanceSnapshotStatus::Ok => Style::default().fg(p.good),
        BalanceSnapshotStatus::Exhausted => Style::default().fg(p.bad),
        BalanceSnapshotStatus::Error => Style::default().fg(p.warn),
        BalanceSnapshotStatus::Stale => Style::default().fg(p.warn),
        BalanceSnapshotStatus::Unknown => Style::default().fg(p.muted),
    }
}

pub(in crate::tui) fn balance_snapshot_status_style(
    p: Palette,
    snapshot: &ProviderBalanceSnapshot,
) -> Style {
    if snapshot.routing_ignored_exhaustion() {
        Style::default().fg(p.warn)
    } else {
        balance_status_style(p, snapshot.status)
    }
}

pub(in crate::tui) fn balance_amount_brief_lang(
    snapshot: &ProviderBalanceSnapshot,
    lang: Language,
) -> Option<String> {
    if snapshot.unlimited_quota == Some(true) {
        return Some(i18n::label(lang, "unlimited").to_string());
    }

    if let Some(total) = snapshot
        .total_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(quota) = quota_amount_brief(snapshot, lang) {
            return Some(format!(
                "{} | {} {}",
                quota,
                i18n::label(lang, "left"),
                usd_brief(total)
            ));
        }
        return Some(format!(
            "{} {}",
            i18n::label(lang, "left"),
            usd_brief(total)
        ));
    }

    if let Some(amount) = quota_amount_brief(snapshot, lang) {
        return Some(amount);
    }

    let subscription = snapshot
        .subscription_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let paygo = snapshot
        .paygo_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let (Some(sub), Some(paygo)) = (subscription, paygo) {
        return Some(format!(
            "{} {} {} + {} {}",
            i18n::label(lang, "left"),
            i18n::label(lang, "subscription"),
            usd_brief(sub),
            i18n::label(lang, "paygo"),
            usd_brief(paygo)
        ));
    }
    if let Some(sub) = subscription {
        return Some(format!(
            "{} {} {}",
            i18n::label(lang, "subscription"),
            i18n::label(lang, "left"),
            usd_brief(sub)
        ));
    }
    if let Some(paygo) = paygo {
        return Some(format!(
            "{} {} {}",
            i18n::label(lang, "paygo"),
            i18n::label(lang, "left"),
            usd_brief(paygo)
        ));
    }

    match (
        snapshot
            .monthly_spent_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        snapshot
            .monthly_budget_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(spent), Some(budget)) => {
            if let Some(left) = left_brief_from_budget_and_spent(budget, spent) {
                Some(format!(
                    "{} {} / {} {}",
                    i18n::label(lang, "left"),
                    left,
                    i18n::label(lang, "budget"),
                    usd_brief(budget)
                ))
            } else {
                Some(format!(
                    "{} {} / {} {}",
                    i18n::label(lang, "budget"),
                    usd_brief(budget),
                    i18n::label(lang, "used"),
                    usd_brief(spent)
                ))
            }
        }
        (Some(spent), None) => {
            if snapshot.plan_name.is_none()
                && snapshot.quota_period.is_none()
                && snapshot.quota_remaining_usd.is_none()
                && snapshot.quota_limit_usd.is_none()
                && snapshot.quota_used_usd.is_none()
            {
                Some(format!(
                    "{} {}",
                    i18n::label(lang, "used"),
                    usd_brief(spent)
                ))
            } else {
                None
            }
        }
        (None, Some(budget)) => Some(format!(
            "{} {}",
            i18n::label(lang, "budget"),
            usd_brief(budget)
        )),
        (None, None) => snapshot
            .total_used_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("{} {}", i18n::label(lang, "used"), usd_brief(value)))
            .or_else(|| {
                snapshot
                    .today_used_usd
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        format!("{} {}", i18n::label(lang, "today used"), usd_brief(value))
                    })
            }),
    }
}

fn quota_amount_brief(snapshot: &ProviderBalanceSnapshot, lang: Language) -> Option<String> {
    let period = snapshot
        .quota_period
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let remaining = snapshot
        .quota_remaining_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let limit = snapshot
        .quota_limit_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let used = snapshot
        .quota_used_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if remaining.is_none() && limit.is_none() && used.is_none() {
        return None;
    }

    let quota_label = match period {
        Some("quota") | None => i18n::label(lang, "quota").to_string(),
        Some(period) => period.to_string(),
    };

    let amount = match (remaining, limit, used) {
        (Some(remaining), Some(limit), _) => {
            format!(
                "{} {} / {}",
                i18n::label(lang, "left"),
                usd_brief(remaining),
                usd_brief(limit)
            )
        }
        (Some(remaining), None, Some(used)) => {
            format!(
                "{} {} / {} {}",
                i18n::label(lang, "left"),
                usd_brief(remaining),
                i18n::label(lang, "used"),
                usd_brief(used)
            )
        }
        (Some(remaining), None, None) => {
            format!("{} {}", i18n::label(lang, "left"), usd_brief(remaining))
        }
        (None, Some(limit), Some(used)) => {
            format!(
                "{} {} / {}",
                i18n::label(lang, "used"),
                usd_brief(used),
                usd_brief(limit)
            )
        }
        (None, Some(limit), None) => {
            format!("{} {}", i18n::label(lang, "limit"), usd_brief(limit))
        }
        (None, None, Some(used)) => {
            format!("{} {}", i18n::label(lang, "used"), usd_brief(used))
        }
        (None, None, None) => return None,
    };

    Some(format!("{quota_label} {amount}"))
}

fn balance_amount_terse_lang(snapshot: &ProviderBalanceSnapshot, lang: Language) -> Option<String> {
    if snapshot.unlimited_quota == Some(true) {
        return Some(i18n::label(lang, "unlimited").to_string());
    }

    let subscription = snapshot
        .subscription_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let paygo = snapshot
        .paygo_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let total = snapshot
        .total_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(amount) = quota_amount_terse(snapshot, lang) {
        return Some(amount);
    }

    if let Some(total) = total {
        return Some(usd_brief(total));
    }

    match (subscription, paygo) {
        (Some(sub), Some(paygo)) => Some(format!(
            "{} {} + {} {}",
            i18n::label(lang, "subscription"),
            usd_brief(sub),
            i18n::label(lang, "paygo"),
            usd_brief(paygo)
        )),
        (Some(sub), None) => Some(format!(
            "{} {}",
            i18n::label(lang, "subscription"),
            usd_brief(sub)
        )),
        (None, Some(paygo)) => Some(format!(
            "{} {}",
            i18n::label(lang, "paygo"),
            usd_brief(paygo)
        )),
        (None, None) => None,
    }
}

fn balance_amount_tiny(snapshot: &ProviderBalanceSnapshot) -> Option<String> {
    let total = snapshot
        .total_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(amount) = quota_amount_tiny(snapshot) {
        return Some(amount);
    }

    if let Some(total) = total {
        return Some(usd_brief(total));
    }

    let subscription = snapshot
        .subscription_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let paygo = snapshot
        .paygo_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (subscription, paygo) {
        (Some(sub), None) => Some(usd_brief(sub)),
        (None, Some(paygo)) => Some(usd_brief(paygo)),
        _ => None,
    }
}

fn quota_amount_terse(snapshot: &ProviderBalanceSnapshot, lang: Language) -> Option<String> {
    let period = snapshot
        .quota_period
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let remaining = snapshot
        .quota_remaining_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let limit = snapshot
        .quota_limit_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (remaining, limit) {
        (Some(remaining), Some(limit)) => {
            let quota_label = match period {
                Some("quota") | None => i18n::label(lang, "quota").to_string(),
                Some(period) => period.to_string(),
            };
            Some(format!(
                "{quota_label} {}/{}",
                usd_brief(remaining),
                usd_brief(limit)
            ))
        }
        _ => None,
    }
}

fn quota_amount_tiny(snapshot: &ProviderBalanceSnapshot) -> Option<String> {
    let remaining = snapshot
        .quota_remaining_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let limit = snapshot
        .quota_limit_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (remaining, limit) {
        (Some(remaining), Some(limit)) => {
            Some(format!("{}/{}", usd_brief(remaining), usd_brief(limit)))
        }
        _ => None,
    }
}

fn usd_brief(raw: &str) -> String {
    format!("${}", decimal_brief(raw))
}

fn left_brief_from_budget_and_spent(budget: &str, spent: &str) -> Option<String> {
    let budget = UsdAmount::from_decimal_str(budget)?;
    let spent = UsdAmount::from_decimal_str(spent)?;
    Some(format!("${}", budget.saturating_sub(spent).format_usd()))
}

fn decimal_brief(raw: &str) -> String {
    let raw = raw.trim();
    let Ok(value) = raw.parse::<f64>() else {
        return raw.to_string();
    };
    if !value.is_finite() {
        return raw.to_string();
    }
    let decimals = if value.abs() >= 1.0 { 2 } else { 4 };
    let mut out = format!("{value:.decimals$}");
    if value.abs() < 1.0
        && let Some(dot) = out.find('.')
    {
        while out.ends_with('0') {
            out.pop();
        }
        if out.len() == dot + 1 {
            out.pop();
        }
    }
    if out == "-0" { "0".to_string() } else { out }
}

fn balance_status_brief_lang(status: BalanceSnapshotStatus, lang: Language) -> &'static str {
    match status {
        BalanceSnapshotStatus::Ok => i18n::label(lang, "ok"),
        BalanceSnapshotStatus::Exhausted => i18n::label(lang, "exh"),
        BalanceSnapshotStatus::Stale => i18n::label(lang, "stale"),
        BalanceSnapshotStatus::Error | BalanceSnapshotStatus::Unknown => {
            i18n::label(lang, "unknown")
        }
    }
}

fn balance_snapshot_status_brief_lang(
    snapshot: &ProviderBalanceSnapshot,
    lang: Language,
) -> &'static str {
    if snapshot.routing_ignored_exhaustion() {
        i18n::label(lang, "lazy")
    } else {
        balance_status_brief_lang(snapshot.status, lang)
    }
}

pub(in crate::tui) fn balance_status_label_lang(
    status: BalanceSnapshotStatus,
    lang: Language,
) -> &'static str {
    match status {
        BalanceSnapshotStatus::Ok => i18n::label(lang, "ok"),
        BalanceSnapshotStatus::Exhausted => i18n::label(lang, "exhausted"),
        BalanceSnapshotStatus::Stale => i18n::label(lang, "stale"),
        BalanceSnapshotStatus::Error | BalanceSnapshotStatus::Unknown => {
            i18n::label(lang, "unknown")
        }
    }
}

#[cfg(test)]
pub(in crate::tui) fn balance_snapshot_status_label(
    snapshot: &ProviderBalanceSnapshot,
) -> &'static str {
    balance_snapshot_status_label_lang(snapshot, Language::En)
}

pub(in crate::tui) fn balance_snapshot_status_label_lang(
    snapshot: &ProviderBalanceSnapshot,
    lang: Language,
) -> &'static str {
    if snapshot.routing_ignored_exhaustion() {
        i18n::label(lang, "lazy reset")
    } else {
        balance_status_label_lang(snapshot.status, lang)
    }
}

#[cfg(test)]
pub(in crate::tui) fn provider_balance_compact(
    snapshot: &ProviderBalanceSnapshot,
    max_width: usize,
) -> String {
    provider_balance_compact_lang(snapshot, max_width, Language::En)
}

pub(in crate::tui) fn provider_balance_compact_lang(
    snapshot: &ProviderBalanceSnapshot,
    max_width: usize,
    lang: Language,
) -> String {
    let status = (snapshot.status != BalanceSnapshotStatus::Ok)
        .then(|| balance_snapshot_status_brief_lang(snapshot, lang).to_string());
    let plan = snapshot
        .plan_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let amount = balance_amount_brief_lang(snapshot, lang);
    let terse_amount = balance_amount_terse_lang(snapshot, lang);
    let tiny_amount = balance_amount_tiny(snapshot);

    let mut candidates = Vec::new();
    let full = [status.as_deref(), plan.as_deref(), amount.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if !full.is_empty() {
        push_balance_candidate(&mut candidates, full);
    }
    if amount.is_some() {
        let without_plan = [status.as_deref(), amount.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !without_plan.is_empty() {
            push_balance_candidate(&mut candidates, without_plan);
        }
    }
    if let (Some(status), Some(terse_amount)) = (status.as_deref(), terse_amount.as_deref()) {
        push_balance_candidate(&mut candidates, format!("{status} {terse_amount}"));
    }
    if let Some(amount) = amount.clone() {
        push_balance_candidate(&mut candidates, amount);
    }
    if let Some(terse_amount) = terse_amount.clone() {
        push_balance_candidate(&mut candidates, terse_amount);
    }
    if let Some(tiny_amount) = tiny_amount.clone() {
        push_balance_candidate(&mut candidates, tiny_amount);
    }
    if let Some(status) = status.clone() {
        push_balance_candidate(&mut candidates, status);
    }
    if candidates.is_empty() {
        push_balance_candidate(
            &mut candidates,
            balance_snapshot_status_brief_lang(snapshot, lang).to_string(),
        );
    }

    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| display_width(candidate) <= max_width)
    {
        return candidate.clone();
    }

    balance_atomic_fallback(snapshot, max_width, lang)
}

fn push_balance_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

pub(in crate::tui) fn balance_snapshot_rank(snapshot: &ProviderBalanceSnapshot) -> u8 {
    match snapshot.status {
        BalanceSnapshotStatus::Ok => 0,
        BalanceSnapshotStatus::Stale => 1,
        BalanceSnapshotStatus::Exhausted if snapshot.routing_ignored_exhaustion() => 1,
        BalanceSnapshotStatus::Unknown | BalanceSnapshotStatus::Error => 2,
        BalanceSnapshotStatus::Exhausted => 3,
    }
}

fn primary_balance_snapshot(
    balances: &[ProviderBalanceSnapshot],
) -> Option<&ProviderBalanceSnapshot> {
    balances.iter().min_by(|left, right| {
        balance_snapshot_rank(left)
            .cmp(&balance_snapshot_rank(right))
            .then_with(|| {
                left.provider_endpoint
                    .endpoint_id
                    .cmp(&right.provider_endpoint.endpoint_id)
            })
            .then_with(|| {
                left.observation_provider_id
                    .cmp(&right.observation_provider_id)
            })
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    })
}

#[cfg(test)]
pub(in crate::tui) fn provider_balance_brief(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    provider_id: &str,
    max_width: usize,
) -> String {
    provider_balance_brief_lang(provider_balances, provider_id, max_width, Language::En)
}

pub(in crate::tui) fn provider_balance_brief_lang(
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    provider_id: &str,
    max_width: usize,
    lang: Language,
) -> String {
    let Some(balances) = provider_balances.get(provider_id) else {
        return "-".to_string();
    };
    if balances.is_empty() {
        return "-".to_string();
    }

    if balances.len() == 1 {
        return provider_balance_compact_lang(&balances[0], max_width, lang);
    }

    let total = balances.len();
    let ok = balances
        .iter()
        .filter(|snapshot| snapshot.status == BalanceSnapshotStatus::Ok)
        .count();
    let stale = balances
        .iter()
        .filter(|snapshot| snapshot.status == BalanceSnapshotStatus::Stale)
        .count();
    let exhausted = balances
        .iter()
        .filter(|snapshot| {
            snapshot.status == BalanceSnapshotStatus::Exhausted
                && !snapshot.routing_ignored_exhaustion()
        })
        .count();
    let lazy_exhausted = balances
        .iter()
        .filter(|snapshot| snapshot.routing_ignored_exhaustion())
        .count();
    let error = balances
        .iter()
        .filter(|snapshot| snapshot.status == BalanceSnapshotStatus::Error)
        .count();
    let unknown = balances
        .iter()
        .filter(|snapshot| snapshot.status == BalanceSnapshotStatus::Unknown)
        .count();
    let displayed_unknown = unknown + error;

    let primary = primary_balance_snapshot(balances);
    let primary_amount = primary.and_then(|snapshot| balance_amount_brief_lang(snapshot, lang));
    let primary_terse_amount =
        primary.and_then(|snapshot| balance_amount_terse_lang(snapshot, lang));
    let primary_tiny_amount = primary.and_then(balance_amount_tiny);

    let mut parts = Vec::new();
    let mut amount_index = None;
    if primary_amount.is_none() || ok == 0 {
        if ok > 0 {
            parts.push(format!("{} {ok}/{total}", i18n::label(lang, "ok")));
        } else if stale > 0 {
            parts.push(format!("{} {stale}/{total}", i18n::label(lang, "stale")));
        } else if displayed_unknown > 0 {
            parts.push(format!(
                "{} {displayed_unknown}/{total}",
                i18n::label(lang, "unknown")
            ));
        } else if lazy_exhausted > 0 {
            parts.push(format!(
                "{} {lazy_exhausted}/{total}",
                i18n::label(lang, "lazy")
            ));
        } else if exhausted > 0 {
            parts.push(format!("{} {exhausted}/{total}", i18n::label(lang, "exh")));
        }
    }

    if let Some(amount) = primary_amount {
        amount_index = Some(parts.len());
        parts.push(amount);
    }
    if exhausted > 0 && ok > 0 {
        parts.push(format!("{} {exhausted}", i18n::label(lang, "exh")));
    }
    if lazy_exhausted > 0 && ok > 0 {
        parts.push(format!("{} {lazy_exhausted}", i18n::label(lang, "lazy")));
    }
    if error > 0 && (ok > 0 || stale > 0 || unknown > 0 || exhausted > 0 || lazy_exhausted > 0) {
        parts.push(format!("{} {error}", i18n::label(lang, "unknown")));
    }

    if parts.is_empty() {
        "-".to_string()
    } else {
        let fallback_label = if ok > 0 {
            i18n::label(lang, "ok").to_string()
        } else if stale > 0 {
            format!("{} {stale}/{total}", i18n::label(lang, "stale"))
        } else if displayed_unknown > 0 {
            format!(
                "{} {displayed_unknown}/{total}",
                i18n::label(lang, "unknown")
            )
        } else if lazy_exhausted > 0 {
            format!("{} {lazy_exhausted}/{total}", i18n::label(lang, "lazy"))
        } else if exhausted > 0 {
            format!("{} {exhausted}/{total}", i18n::label(lang, "exh"))
        } else {
            "-".to_string()
        };
        compact_balance_parts(
            &parts,
            amount_index,
            primary_terse_amount.as_deref(),
            primary_tiny_amount.as_deref(),
            max_width,
            &fallback_label,
        )
    }
}

fn balance_atomic_fallback(
    snapshot: &ProviderBalanceSnapshot,
    max_width: usize,
    lang: Language,
) -> String {
    let label = balance_snapshot_status_brief_lang(snapshot, lang);
    if display_width(label) <= max_width {
        return label.to_string();
    }
    shorten_middle(label, max_width)
}

fn compact_balance_parts(
    parts: &[String],
    amount_index: Option<usize>,
    terse_amount: Option<&str>,
    tiny_amount: Option<&str>,
    max_width: usize,
    fallback_label: &str,
) -> String {
    let full = parts.join(" ");
    if display_width(&full) <= max_width {
        return full;
    }

    if let Some(amount_index) = amount_index {
        let mut candidate = parts[amount_index].clone();
        for part in parts.iter().skip(amount_index.saturating_add(1)) {
            let next = format!("{candidate} {part}");
            if display_width(&next) <= max_width {
                candidate = next;
            }
        }

        for part in parts[..amount_index].iter().rev() {
            let next = format!("{part} {candidate}");
            if display_width(&next) <= max_width {
                candidate = next;
            }
        }

        if display_width(&candidate) <= max_width {
            return candidate;
        }

        if let Some(terse_amount) = terse_amount
            && display_width(terse_amount) <= max_width
        {
            return terse_amount.to_string();
        }

        if let Some(tiny_amount) = tiny_amount
            && display_width(tiny_amount) <= max_width
        {
            return tiny_amount.to_string();
        }
    }

    let non_amount = parts
        .iter()
        .enumerate()
        .filter(|(idx, _)| Some(*idx) != amount_index)
        .map(|(_, part)| part.as_str())
        .find(|part| display_width(part) <= max_width);
    if let Some(part) = non_amount {
        return part.to_string();
    }

    if display_width(fallback_label) <= max_width {
        return fallback_label.to_string();
    }

    shorten_middle(fallback_label, max_width)
}

fn balance_by_provider_id<'a>(
    provider_balances: &'a HashMap<String, Vec<ProviderBalanceSnapshot>>,
    provider_id: &str,
) -> Option<&'a ProviderBalanceSnapshot> {
    let balances = provider_balances.get(provider_id)?;
    let mut matches = balances
        .iter()
        .filter(|snapshot| snapshot.provider_endpoint.provider_id == provider_id)
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return None;
    }
    matches.sort_by(|left, right| {
        balance_snapshot_rank(left)
            .cmp(&balance_snapshot_rank(right))
            .then_with(|| {
                left.provider_endpoint
                    .endpoint_id
                    .cmp(&right.provider_endpoint.endpoint_id)
            })
            .then_with(|| {
                left.observation_provider_id
                    .cmp(&right.observation_provider_id)
            })
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    });
    matches.into_iter().next()
}

pub(in crate::tui) fn session_observed_provider_balance_brief_lang(
    row: &SessionRow,
    provider_balances: &HashMap<String, Vec<ProviderBalanceSnapshot>>,
    max_width: usize,
    lang: Language,
) -> Option<String> {
    session_observed_provider_balance_snapshot(row, provider_balances)
        .map(|snapshot| provider_balance_compact_lang(snapshot, max_width, lang))
}

pub(in crate::tui) fn session_observed_provider_balance_snapshot<'a>(
    row: &SessionRow,
    provider_balances: &'a HashMap<String, Vec<ProviderBalanceSnapshot>>,
) -> Option<&'a ProviderBalanceSnapshot> {
    let provider_id = row.observed_provider_id()?;
    if let Some(endpoint_id) = row.observed_endpoint_id()
        && let Some(snapshot) = provider_balances.get(provider_id).and_then(|snapshots| {
            snapshots.iter().find(|snapshot| {
                snapshot.provider_endpoint.provider_id == provider_id
                    && snapshot.provider_endpoint.endpoint_id == endpoint_id
            })
        })
    {
        return Some(snapshot);
    }
    balance_by_provider_id(provider_balances, provider_id)
}

#[cfg(test)]
fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

pub(in crate::tui) fn format_age(now_ms: u64, ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return "-".to_string();
    };
    if now_ms <= ts {
        return "0s".to_string();
    }
    let mut secs = (now_ms - ts) / 1000;
    let days = secs / 86400;
    secs %= 86400;
    let hours = secs / 3600;
    secs %= 3600;
    let mins = secs / 60;
    secs %= 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

pub(in crate::tui) fn duration_short(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 10_000 {
        let mut out = format!("{:.1}s", ms as f64 / 1_000.0);
        if out.ends_with(".0s") {
            out.replace_range(out.len() - 3..out.len() - 1, "");
        }
        out
    } else if ms < 60_000 {
        format!("{}s", ms / 1_000)
    } else if ms < 3_600_000 {
        let secs = ms / 1_000;
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        let secs = ms / 1_000;
        let mins = (secs % 3_600) / 60;
        format!("{}h{}m", secs / 3_600, mins)
    }
}

pub(in crate::tui) fn tokens_short(n: i64) -> String {
    crate::usage_format::tokens_short(n)
}

pub(in crate::tui) fn format_tok_per_second(value: Option<f64>) -> String {
    crate::usage_format::tokens_per_second(value)
}

pub(in crate::tui) fn usage_line_lang(usage: &UsageMetrics, lang: Language) -> String {
    format!(
        "{}: {}/{}/{}/{}",
        i18n::label(lang, "tok in/out/rsn/ttl"),
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_output_tokens),
        tokens_short(usage.total_tokens),
    )
}

pub(in crate::tui) fn status_style(p: Palette, status: Option<u16>) -> Style {
    match status {
        Some(s) if (200..300).contains(&s) => Style::default().fg(p.good),
        Some(s) if (300..400).contains(&s) => Style::default().fg(p.accent),
        Some(s) if (400..500).contains(&s) => Style::default().fg(p.warn),
        Some(_) => Style::default().fg(p.bad),
        None => Style::default().fg(p.muted),
    }
}

#[cfg(test)]
fn build_session_rows_from_cards(cards: &[SessionIdentityCard]) -> Vec<SessionRow> {
    let mut rows = cards
        .iter()
        .filter_map(|card| {
            let session_id = card.session_id.clone()?;
            Some(SessionRow {
                session_id: Some(session_id),
                local_session_id: card.session_id.clone(),
                observation_scope: card.observation_scope,
                host_local_transcript_path: card.host_local_transcript_path.clone(),
                last_client_name: card.last_client_name.clone(),
                last_client_addr: card.last_client_addr.clone(),
                cwd: card.cwd.clone(),
                active_count: card.active_count as usize,
                active_started_at_ms_min: card.active_started_at_ms_min,
                active_last_method: None,
                active_last_path: None,
                last_status: card.last_status,
                last_duration_ms: card.last_duration_ms,
                last_ended_at_ms: card.last_ended_at_ms,
                last_model: card.last_model.clone(),
                last_reasoning_effort: card.last_reasoning_effort.clone(),
                last_service_tier: card.last_service_tier.clone(),
                last_provider_id: card.last_provider_id.clone(),
                last_usage: card.last_usage.clone(),
                total_usage: card.total_usage.clone(),
                turns_total: card.turns_total,
                turns_with_usage: card.turns_with_usage,
                last_output_tokens_per_second: card.last_output_tokens_per_second,
                avg_output_tokens_per_second: card.avg_output_tokens_per_second,
                binding_profile_name: card.binding_profile_name.clone(),
                binding_continuity_mode: card.binding_continuity_mode,
                last_route_decision: card
                    .last_route_decision
                    .as_ref()
                    .map(sanitize_route_decision),
                route_affinity: card.route_affinity.as_ref().and_then(|affinity| {
                    Some(SessionRouteAffinityView {
                        provider_id: affinity.provider_endpoint.provider_id.clone(),
                        endpoint_id: affinity.provider_endpoint.endpoint_id.clone(),
                        upstream_origin: sanitize_upstream_origin(&affinity.upstream_base_url)?,
                        route_path: affinity.route_path.clone(),
                        last_selected_at_ms: affinity.last_selected_at_ms,
                        last_changed_at_ms: affinity.last_changed_at_ms,
                        change_reason: affinity.change_reason.clone(),
                    })
                }),
                effective_model: card.effective_model.clone(),
                effective_reasoning_effort: card.effective_reasoning_effort.clone(),
                effective_service_tier: card.effective_service_tier.clone(),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(session_sort_key(r)));
    rows
}

pub(in crate::tui) fn format_observed_client_identity(
    client_name: Option<&str>,
    client_addr: Option<&str>,
) -> Option<String> {
    match (
        client_name.map(str::trim).filter(|value| !value.is_empty()),
        client_addr.map(str::trim).filter(|value| !value.is_empty()),
    ) {
        (Some(name), Some(addr)) => Some(format!("{name} @ {addr}")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(addr)) => Some(addr.to_string()),
        (None, None) => None,
    }
}

pub(in crate::tui) fn session_observation_scope_label_lang(
    scope: SessionObservationScope,
    lang: Language,
) -> &'static str {
    match scope {
        SessionObservationScope::ObservedOnly => i18n::label(lang, "observed only"),
        SessionObservationScope::HostLocalEnriched => i18n::label(lang, "host-local enriched"),
    }
}

pub(in crate::tui) fn session_transcript_host_status_lang(
    row: &SessionRow,
    lang: Language,
) -> String {
    if row.host_local_transcript_path.is_some() {
        i18n::label(lang, "linked under ~/.codex/sessions").to_string()
    } else {
        i18n::label(lang, "no host-local transcript detected").to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::tui) struct SessionControlPosture {
    pub(in crate::tui) headline: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) color: Color,
}

#[cfg(test)]
pub(in crate::tui) fn session_control_posture(
    row: &SessionRow,
    route_graph_routing: bool,
) -> SessionControlPosture {
    session_control_posture_lang(row, route_graph_routing, Language::En)
}

pub(in crate::tui) fn session_control_posture_lang(
    row: &SessionRow,
    route_graph_routing: bool,
    lang: Language,
) -> SessionControlPosture {
    if let Some(profile_name) = row.binding_profile_name.as_deref() {
        let mode = row
            .binding_continuity_mode
            .map(|mode| format!("{mode:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "default_profile".to_string());
        return SessionControlPosture {
            headline: match lang {
                Language::Zh => format!("绑定到 profile {profile_name}（{mode}）"),
                Language::En => format!("bound to profile {profile_name} ({mode})"),
            },
            detail: i18n::label(
                lang,
                "This session keeps its stored profile binding while runtime observations explain the effective route.",
            )
            .to_string(),
            color: Color::Rgb(63, 185, 80),
        };
    }

    SessionControlPosture {
        headline: i18n::label(lang, "no stored profile binding").to_string(),
        detail: i18n::label(
            lang,
            if route_graph_routing {
                "Effective route comes from request payloads, route graph defaults, and runtime fallback."
            } else {
                "Effective route comes from request payloads, profile defaults, and runtime fallback."
            },
        )
        .to_string(),
        color: Color::Rgb(144, 154, 164),
    }
}

pub(in crate::tui) fn snapshot_from_operator_data(
    data: &OperatorReadData,
    local_session_ids: &HashMap<String, String>,
) -> Snapshot {
    let sessions = &data.summary.sessions;
    Snapshot {
        rows: sessions
            .iter()
            .map(|session| session_row_from_operator(session, local_session_ids))
            .collect(),
        recent: data.recent_requests.clone(),
        request_control_evidence: data
            .recent_requests
            .iter()
            .map(|request| (request.id, request_control_evidence_from_operator(request)))
            .collect(),
        usage_day: data.usage_day.clone(),
        usage_rollup: data.usage_rollup.clone(),
        provider_balances: operator_provider_balances(
            &data.summary.service_name,
            &data.provider_balances,
        ),
        stats_5m: data.stats_5m.clone(),
        stats_1h: data.stats_1h.clone(),
        service_status: None,
        refreshed_at: Instant::now(),
    }
}

pub(in crate::tui) fn provider_options_from_operator_data(
    data: &OperatorReadData,
) -> Vec<ProviderOption> {
    data.summary.providers.clone()
}

fn session_row_from_operator(
    session: &OperatorSessionSummary,
    local_session_ids: &HashMap<String, String>,
) -> SessionRow {
    SessionRow {
        session_id: Some(session.session_key.clone()),
        local_session_id: local_session_ids.get(&session.session_key).cloned(),
        observation_scope: SessionObservationScope::ObservedOnly,
        host_local_transcript_path: None,
        last_client_name: None,
        last_client_addr: None,
        cwd: None,
        active_count: usize::try_from(session.active_count).unwrap_or(usize::MAX),
        active_started_at_ms_min: session.active_started_at_ms_min,
        active_last_method: None,
        active_last_path: None,
        last_status: session.last_status,
        last_duration_ms: session.last_duration_ms,
        last_ended_at_ms: session.last_ended_at_ms,
        last_model: session.last_model.clone(),
        last_reasoning_effort: session.last_reasoning_effort.clone(),
        last_service_tier: session.last_service_tier.clone(),
        last_provider_id: session.last_provider_id.clone(),
        last_usage: session.last_usage.clone(),
        total_usage: session.total_usage.clone(),
        turns_total: session.turns_total,
        turns_with_usage: session.turns_with_usage,
        last_output_tokens_per_second: session.last_output_tokens_per_second,
        avg_output_tokens_per_second: session.avg_output_tokens_per_second,
        binding_profile_name: session.binding_profile_name.clone(),
        binding_continuity_mode: session.binding_continuity_mode,
        last_route_decision: session
            .last_route_decision
            .as_ref()
            .map(sanitize_route_decision),
        route_affinity: session.route_affinity.as_ref().and_then(|affinity| {
            Some(SessionRouteAffinityView {
                provider_id: affinity.provider_id.clone(),
                endpoint_id: affinity.endpoint_id.clone(),
                upstream_origin: sanitize_upstream_origin(&affinity.upstream_origin)?,
                route_path: affinity.route_path.clone(),
                last_selected_at_ms: affinity.last_selected_at_ms,
                last_changed_at_ms: affinity.last_changed_at_ms,
                change_reason: affinity.change_reason.clone(),
            })
        }),
        effective_model: session.effective_model.clone(),
        effective_reasoning_effort: session.effective_reasoning_effort.clone(),
        effective_service_tier: session.effective_service_tier.clone(),
    }
}

fn request_control_evidence_from_operator(
    request: &OperatorRequestSummary,
) -> RequestControlEvidence {
    RequestControlEvidence {
        provider_signal_codes: request.provider_signal_codes.clone(),
        policy_action_codes: request.policy_action_codes.clone(),
        route_attempts: request
            .retry
            .iter()
            .flat_map(|retry| retry.route_attempts.iter())
            .map(|attempt| {
                (
                    attempt.attempt_index,
                    RequestAttemptControlEvidence {
                        provider_signal_codes: attempt.provider_signal_codes.clone(),
                        policy_action_codes: attempt.policy_action_codes.clone(),
                    },
                )
            })
            .collect(),
    }
}

fn operator_provider_balances(
    service_name: &str,
    balances: &[OperatorProviderBalanceSummary],
) -> HashMap<String, Vec<ProviderBalanceSnapshot>> {
    let mut grouped = HashMap::<String, Vec<ProviderBalanceSnapshot>>::new();
    for balance in balances {
        grouped
            .entry(balance.provider_id.clone())
            .or_default()
            .push(ProviderBalanceSnapshot {
                observation_provider_id: balance.observation_provider_id.clone(),
                provider_endpoint: ProviderEndpointKey::new(
                    service_name,
                    &balance.provider_id,
                    &balance.endpoint_id,
                ),
                source: "operator_read_model".to_string(),
                fetched_at_ms: balance.fetched_at_ms,
                stale_after_ms: balance.stale_after_ms,
                stale: balance.stale,
                status: balance.status,
                exhausted: balance.exhausted,
                exhaustion_affects_routing: balance.exhaustion_affects_routing,
                plan_name: balance.plan_name.clone(),
                total_balance_usd: balance.total_balance_usd.clone(),
                subscription_balance_usd: balance.subscription_balance_usd.clone(),
                paygo_balance_usd: balance.paygo_balance_usd.clone(),
                monthly_budget_usd: balance.monthly_budget_usd.clone(),
                monthly_spent_usd: balance.monthly_spent_usd.clone(),
                quota_period: balance.quota_period.clone(),
                quota_remaining_usd: balance.quota_remaining_usd.clone(),
                quota_limit_usd: balance.quota_limit_usd.clone(),
                quota_used_usd: balance.quota_used_usd.clone(),
                quota_resets_at_ms: balance.quota_resets_at_ms,
                unlimited_quota: balance.unlimited_quota,
                total_used_usd: balance.total_used_usd.clone(),
                today_used_usd: balance.today_used_usd.clone(),
                total_requests: balance.total_requests,
                today_requests: balance.today_requests,
                total_tokens: balance.total_tokens,
                today_tokens: balance.today_tokens,
                subscription_expires_at: balance.subscription_expires_at.clone(),
                usage_windows: balance.usage_windows.clone(),
                usage_rate: balance.usage_rate.clone(),
                usage_model_stats: balance.usage_model_stats.clone(),
                usage_alerts: balance
                    .alert_codes
                    .iter()
                    .map(|kind| codex_helper_core::balance::ProviderUsageAlert {
                        kind: *kind,
                        message: kind.as_str().to_string(),
                    })
                    .collect(),
                ..Default::default()
            });
    }
    grouped
}

pub(in crate::tui) fn filtered_requests_len(
    snapshot: &Snapshot,
    selected_session_idx: usize,
) -> usize {
    let Some(selected_row) = snapshot.rows.get(selected_session_idx) else {
        return snapshot.recent.iter().take(60).count();
    };
    snapshot
        .recent
        .iter()
        .filter(
            |r| match (selected_row.session_id.as_deref(), r.session_key.as_deref()) {
                (Some(sid), Some(rid)) => sid == rid,
                (Some(_), None) => false,
                (None, Some(_)) => false,
                (None, None) => true,
            },
        )
        .take(60)
        .count()
}

pub(in crate::tui) fn find_session_idx(snapshot: &Snapshot, sid: &str) -> Option<usize> {
    snapshot
        .rows
        .iter()
        .position(|row| row.session_id.as_deref() == Some(sid))
}

pub(in crate::tui) fn request_page_focus_session_id(
    snapshot: &Snapshot,
    explicit_focus: Option<&str>,
    selected_session_idx: usize,
) -> Option<String> {
    explicit_focus.map(ToOwned::to_owned).or_else(|| {
        snapshot
            .rows
            .get(selected_session_idx)
            .and_then(|row| row.session_id.clone())
    })
}

pub(in crate::tui) fn request_page_focus_is_runtime_observed(
    snapshot: &Snapshot,
    focused_sid: Option<&str>,
) -> bool {
    let Some(sid) = focused_sid else {
        return true;
    };
    snapshot
        .rows
        .iter()
        .any(|row| row.session_id.as_deref() == Some(sid))
}

pub(in crate::tui) fn request_matches_page_filters(
    request: &OperatorRequestSummary,
    control_evidence: Option<&RequestControlEvidence>,
    errors_only: bool,
    scope_session: bool,
    focused_sid: Option<&str>,
    control_filter: RequestControlFilter,
) -> bool {
    if errors_only && request.status_code < 400 {
        return false;
    }
    if !request_matches_control_filter(control_evidence, control_filter) {
        return false;
    }
    if !scope_session {
        return true;
    }

    match (focused_sid, request.session_key.as_deref()) {
        (Some(sid), Some(request_sid)) => sid == request_sid,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

pub(in crate::tui) fn request_matches_control_filter(
    control_evidence: Option<&RequestControlEvidence>,
    control_filter: RequestControlFilter,
) -> bool {
    match control_filter {
        RequestControlFilter::All => true,
        RequestControlFilter::AnyEvidence => control_evidence.is_some_and(|evidence| {
            evidence.has_provider_signals() || evidence.has_policy_actions()
        }),
        RequestControlFilter::Signals => {
            control_evidence.is_some_and(RequestControlEvidence::has_provider_signals)
        }
        RequestControlFilter::Actions => {
            control_evidence.is_some_and(RequestControlEvidence::has_policy_actions)
        }
    }
}

pub(in crate::tui) fn filtered_request_page_len(
    snapshot: &Snapshot,
    explicit_focus: Option<&str>,
    selected_session_idx: usize,
    errors_only: bool,
    scope_session: bool,
    control_filter: RequestControlFilter,
) -> usize {
    let focused_sid = request_page_focus_session_id(snapshot, explicit_focus, selected_session_idx);
    snapshot
        .recent
        .iter()
        .filter(|request| {
            request_matches_page_filters(
                request,
                snapshot.request_control_evidence.get(&request.id),
                errors_only,
                scope_session,
                focused_sid.as_deref(),
                control_filter,
            )
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

    fn balance_identity(
        observation_provider_id: &str,
        provider_id: &str,
        endpoint_id: &str,
    ) -> ProviderBalanceSnapshot {
        ProviderBalanceSnapshot {
            observation_provider_id: observation_provider_id.to_string(),
            provider_endpoint: ProviderEndpointKey::new("codex", provider_id, endpoint_id),
            ..ProviderBalanceSnapshot::default()
        }
    }

    fn operator_balance(
        observation_provider_id: &str,
        provider_id: &str,
        endpoint_id: &str,
        provider_endpoint_key: &str,
    ) -> OperatorProviderBalanceSummary {
        serde_json::from_value(serde_json::json!({
            "observation_provider_id": observation_provider_id,
            "provider_id": provider_id,
            "endpoint_id": endpoint_id,
            "provider_endpoint_key": provider_endpoint_key,
            "fetched_at_ms": 100,
            "stale": false,
            "status": "ok",
            "exhaustion_affects_routing": true
        }))
        .expect("operator balance summary")
    }

    #[test]
    fn operator_provider_balances_reconstructs_canonical_identity_without_parsing_opaque_key() {
        let grouped = operator_provider_balances(
            "codex",
            &[operator_balance(
                "quota-observer",
                "input",
                "responses",
                "opaque:wrong-service/wrong-provider/wrong-endpoint",
            )],
        );

        let snapshot = grouped
            .get("input")
            .and_then(|balances| balances.first())
            .expect("canonical provider balance");
        assert_eq!(snapshot.observation_provider_id, "quota-observer");
        assert_eq!(
            snapshot.provider_endpoint,
            ProviderEndpointKey::new("codex", "input", "responses")
        );
    }

    #[test]
    fn operator_provider_balances_keeps_same_observer_across_endpoints() {
        let grouped = operator_provider_balances(
            "codex",
            &[
                operator_balance("quota-observer", "input", "chat", "opaque:chat"),
                operator_balance("quota-observer", "input", "responses", "opaque:responses"),
            ],
        );

        let balances = grouped.get("input").expect("input balances");
        assert_eq!(balances.len(), 2);
        assert_eq!(
            balances
                .iter()
                .map(|snapshot| snapshot.provider_endpoint.endpoint_id.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["chat", "responses"])
        );
    }

    #[test]
    fn sanitize_upstream_origin_drops_path_query_and_credentials() {
        assert_eq!(
            sanitize_upstream_origin("https://user:secret@relay.example.test:8443/v1?q=token")
                .as_deref(),
            Some("https://relay.example.test:8443")
        );
    }

    fn empty_session_row() -> SessionRow {
        SessionRow {
            session_id: Some("sid".to_string()),
            local_session_id: None,
            observation_scope: SessionObservationScope::ObservedOnly,
            host_local_transcript_path: None,
            last_client_name: None,
            last_client_addr: None,
            cwd: None,
            active_count: 0,
            active_started_at_ms_min: None,
            active_last_method: None,
            active_last_path: None,
            last_status: None,
            last_duration_ms: None,
            last_ended_at_ms: None,
            last_model: None,
            last_reasoning_effort: None,
            last_service_tier: None,
            last_provider_id: None,
            last_usage: None,
            total_usage: None,
            turns_total: None,
            turns_with_usage: None,
            last_output_tokens_per_second: None,
            avg_output_tokens_per_second: None,
            binding_profile_name: None,
            binding_continuity_mode: None,
            last_route_decision: None,
            route_affinity: None,
            effective_model: None,
            effective_reasoning_effort: None,
            effective_service_tier: None,
        }
    }

    fn operator_request(id: u64, session_id: Option<&str>) -> OperatorRequestSummary {
        OperatorRequestSummary {
            id,
            session_key: session_id.map(ToOwned::to_owned),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            route_path: Vec::new(),
            upstream_origin: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: crate::dashboard_core::OperatorRequestObservability {
                duration_ms: Some(120),
                ttfb_ms: None,
                generation_ms: None,
                output_tokens_per_second: None,
                attempt_count: 1,
                route_attempt_count: 0,
                retried: false,
                cross_provider_failover: false,
                same_provider_retry: false,
                fast_mode: false,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 120,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
        }
    }

    #[test]
    fn operator_projection_preserves_local_handle_route_provenance_and_control_codes() {
        use crate::dashboard_core::{
            ApiV1OperatorSummary, OperatorRequestObservability, OperatorRetrySummary,
            OperatorRetrySummaryView, OperatorRouteAttemptSummary, OperatorRuntimeSummary,
            OperatorSessionRouteAffinitySummary, OperatorSummaryCounts,
        };
        use crate::state::RouteValueSource;

        let session_key = "session:sha256:opaque";
        let route_decision = RouteDecisionProvenance {
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.6".to_string(),
                source: RouteValueSource::SessionOverride,
            }),
            provider_id: Some("provider-a".to_string()),
            endpoint_id: Some("responses".to_string()),
            route_path: vec!["main".to_string(), "provider-a".to_string()],
            effective_upstream_base_url: Some(ResolvedRouteValue {
                value: "https://relay.example.test/v1/responses?token=secret".to_string(),
                source: RouteValueSource::ProviderMapping,
            }),
            ..RouteDecisionProvenance::default()
        };
        let session = OperatorSessionSummary {
            session_key: session_key.to_string(),
            active_count: 1,
            active_started_at_ms_min: Some(10),
            last_status: Some(200),
            last_duration_ms: Some(20),
            last_ended_at_ms: Some(30),
            last_model: Some("gpt-5.6".to_string()),
            last_reasoning_effort: Some("high".to_string()),
            last_service_tier: Some("priority".to_string()),
            last_provider_id: Some("provider-a".to_string()),
            last_usage: None,
            total_usage: None,
            turns_total: Some(1),
            turns_with_usage: Some(1),
            last_output_tokens_per_second: Some(12.5),
            avg_output_tokens_per_second: Some(10.0),
            binding_profile_name: Some("fast".to_string()),
            binding_continuity_mode: None,
            last_route_decision: Some(route_decision.clone()),
            route_affinity: Some(OperatorSessionRouteAffinitySummary {
                provider_id: "provider-a".to_string(),
                endpoint_id: "responses".to_string(),
                upstream_origin: "https://relay.example.test".to_string(),
                route_path: vec!["main".to_string(), "provider-a".to_string()],
                last_selected_at_ms: 10,
                last_changed_at_ms: 11,
                change_reason: "selected".to_string(),
            }),
            effective_model: Some(ResolvedRouteValue {
                value: "gpt-5.6".to_string(),
                source: RouteValueSource::SessionOverride,
            }),
            effective_reasoning_effort: Some(ResolvedRouteValue {
                value: "high".to_string(),
                source: RouteValueSource::SessionOverride,
            }),
            effective_service_tier: Some(ResolvedRouteValue {
                value: "priority".to_string(),
                source: RouteValueSource::SessionOverride,
            }),
        };
        let request = OperatorRequestSummary {
            id: 7,
            session_key: Some(session_key.to_string()),
            model: Some("gpt-5.6".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            provider_id: Some("provider-a".to_string()),
            endpoint_id: Some("responses".to_string()),
            provider_endpoint_key: Some("endpoint:sha256:opaque".to_string()),
            route_path: vec!["main".to_string(), "provider-a".to_string()],
            upstream_origin: Some("https://relay.example.test".to_string()),
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: Some(OperatorRetrySummaryView {
                attempts: 2,
                route_attempts: vec![OperatorRouteAttemptSummary {
                    attempt_index: 0,
                    provider_id: Some("provider-a".to_string()),
                    endpoint_id: Some("responses".to_string()),
                    provider_endpoint_key: Some("endpoint:sha256:opaque".to_string()),
                    preference_group: Some(0),
                    provider_attempt: Some(1),
                    upstream_attempt: Some(1),
                    provider_max_attempts: Some(2),
                    upstream_max_attempts: Some(2),
                    avoided_total: Some(0),
                    total_upstreams: Some(1),
                    code: "failed_status".to_string(),
                    status_code: Some(429),
                    model: Some("gpt-5.6".to_string()),
                    upstream_headers_ms: Some(5),
                    duration_ms: Some(10),
                    cooldown_secs: Some(30),
                    skipped: false,
                    provider_signal_codes: vec!["rate_limit".to_string()],
                    policy_action_codes: vec!["cooldown".to_string()],
                }],
            }),
            provider_signal_codes: vec!["rate_limit".to_string()],
            policy_action_codes: vec!["cooldown".to_string()],
            observability: OperatorRequestObservability {
                duration_ms: Some(20),
                ttfb_ms: Some(5),
                generation_ms: Some(15),
                output_tokens_per_second: Some(12.5),
                attempt_count: 2,
                route_attempt_count: 1,
                retried: true,
                cross_provider_failover: false,
                same_provider_retry: true,
                fast_mode: true,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 20,
            ttfb_ms: Some(5),
            streaming: false,
            ended_at_ms: 30,
        };
        let data = OperatorReadData {
            summary: ApiV1OperatorSummary {
                api_version: 1,
                service_name: "codex".to_string(),
                runtime: OperatorRuntimeSummary::default(),
                counts: OperatorSummaryCounts::default(),
                retry: OperatorRetrySummary::default(),
                sessions: vec![session],
                profiles: Vec::new(),
                providers: Vec::new(),
            },
            active_requests: Vec::new(),
            recent_requests: vec![request],
            usage_summaries: Vec::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            provider_balances: Vec::new(),
        };
        let local_session_ids =
            HashMap::from([(session_key.to_string(), "raw-session-id".to_string())]);

        let snapshot = snapshot_from_operator_data(&data, &local_session_ids);

        let row = snapshot.rows.first().expect("operator session row");
        assert_eq!(row.session_id.as_deref(), Some(session_key));
        assert_eq!(row.local_session_id.as_deref(), Some("raw-session-id"));
        assert_eq!(
            row.observed_upstream_origin().as_deref(),
            Some("https://relay.example.test")
        );
        assert_eq!(
            row.route_affinity
                .as_ref()
                .map(|affinity| affinity.endpoint_id.as_str()),
            Some("responses")
        );
        assert_eq!(
            row.effective_model
                .as_ref()
                .map(|value| value.value.as_str()),
            Some("gpt-5.6")
        );
        assert_eq!(
            row.effective_model.as_ref().map(|value| value.source),
            Some(RouteValueSource::SessionOverride)
        );
        let evidence = snapshot
            .request_control_evidence
            .get(&7)
            .expect("operator request control evidence");
        assert_eq!(evidence.provider_signal_codes, ["rate_limit"]);
        assert_eq!(evidence.policy_action_codes, ["cooldown"]);
        assert_eq!(
            evidence
                .route_attempts
                .get(&0)
                .map(|attempt| attempt.provider_signal_codes.as_slice()),
            Some(["rate_limit".to_string()].as_slice())
        );
        assert_eq!(row.local_command_session_id(), Some("raw-session-id"));
        assert_eq!(
            snapshot.recent[0].provider_endpoint_key.as_deref(),
            Some("endpoint:sha256:opaque")
        );
    }

    #[test]
    fn basename_handles_unix_and_windows_paths() {
        assert_eq!(basename("/a/b/c"), "c");
        assert_eq!(basename("/a/b/c/"), "c");
        assert_eq!(basename(r"C:\a\b\c"), "c");
        assert_eq!(basename(r"C:\a\b\c\"), "c");
    }

    #[test]
    fn shorten_respects_display_width_cjk() {
        let s = "你好世界";
        let out = shorten(s, 5);
        assert_eq!(out, "你好…");
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 5);
    }

    #[test]
    fn shorten_middle_keeps_both_ends() {
        let s = "abcdef";
        assert_eq!(shorten_middle(s, 5), "ab…ef");
    }

    #[test]
    fn provider_balance_compact_includes_plan_and_amount() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            plan_name: Some("CodeX Air".to_string()),
            total_balance_usd: Some("165.08".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert_eq!(
            provider_balance_compact(&snapshot, 80),
            "CodeX Air left $165.08"
        );
    }

    #[test]
    fn provider_balance_compact_prefers_quota_when_wallet_is_also_present() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            plan_name: Some("RightCode Daily".to_string()),
            total_balance_usd: Some("3.25".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("7.5".to_string()),
            quota_limit_usd: Some("20".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert_eq!(
            provider_balance_compact(&snapshot, 120),
            "RightCode Daily daily left $7.50 / $20.00 | left $3.25"
        );
        assert_eq!(
            provider_balance_compact(&snapshot, 18),
            "daily $7.50/$20.00"
        );
    }

    #[test]
    fn provider_balance_compact_prefers_unlimited_over_spend() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            plan_name: Some("cx".to_string()),
            unlimited_quota: Some(true),
            quota_used_usd: Some("106065.94".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert_eq!(provider_balance_compact(&snapshot, 80), "cx unlimited");
    }

    #[test]
    fn provider_balance_compact_shows_quota_window_instead_of_spend() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            plan_name: Some("CodeX Lite 年度".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            quota_limit_usd: Some("100".to_string()),
            quota_used_usd: Some("100.468025".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert_eq!(
            provider_balance_compact(&snapshot, 120),
            "exh CodeX Lite 年度 daily left $0 / $100.00"
        );
        assert_eq!(provider_balance_compact(&snapshot, 14), "$0/$100.00");
    }

    #[test]
    fn provider_balance_compact_falls_back_to_status_in_too_narrow_cells() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            quota_limit_usd: Some("300".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        let en = provider_balance_compact(&snapshot, 4);
        assert_eq!(en, "lazy");
        assert!(!en.contains('$'), "{en}");
        assert!(!en.contains('…'), "{en}");

        let zh = provider_balance_compact_lang(&snapshot, 6, Language::Zh);
        assert_eq!(zh, "不降级");
        assert!(!zh.contains('$'), "{zh}");
        assert!(!zh.contains('…'), "{zh}");
    }

    #[test]
    fn provider_balance_compact_marks_ignored_exhaustion_as_lazy() {
        let snapshot = ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            plan_name: Some("CodeX Lite 年度".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("0".to_string()),
            quota_limit_usd: Some("100".to_string()),
            ..ProviderBalanceSnapshot::default()
        };

        assert_eq!(
            provider_balance_compact(&snapshot, 120),
            "lazy CodeX Lite 年度 daily left $0 / $100.00"
        );
        assert_eq!(balance_snapshot_status_label(&snapshot), "lazy reset");
    }

    #[test]
    fn provider_balance_brief_prefers_usable_snapshot_and_keeps_warnings() {
        let balances = HashMap::from([(
            "input".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Exhausted,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("12.50".to_string()),
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        )]);

        assert_eq!(
            provider_balance_brief(&balances, "input", 24),
            "left $12.50 exh 1"
        );
        assert_eq!(
            balances
                .get("input")
                .and_then(|values| primary_balance_snapshot(values))
                .map(|snapshot| snapshot.status),
            Some(BalanceSnapshotStatus::Ok)
        );
    }

    #[test]
    fn provider_balance_brief_marks_ignored_exhaustion_as_lazy() {
        let balances = HashMap::from([(
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                exhausted: Some(true),
                exhaustion_affects_routing: false,
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("0".to_string()),
                quota_limit_usd: Some("100".to_string()),
                ..ProviderBalanceSnapshot::default()
            }],
        )]);

        assert_eq!(
            provider_balance_brief(&balances, "input", 80),
            "lazy daily left $0 / $100.00"
        );
    }

    #[test]
    fn provider_balance_brief_keeps_quota_amount_complete_when_counts_overflow() {
        let mut snapshots = vec![ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Ok,
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("93.83".to_string()),
            quota_limit_usd: Some("100".to_string()),
            ..balance_identity("routing-observer", "routing", "default")
        }];
        snapshots.extend((0..4).map(|_| ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Exhausted,
            exhausted: Some(true),
            exhaustion_affects_routing: false,
            ..balance_identity("routing-observer", "routing", "default")
        }));
        snapshots.push(ProviderBalanceSnapshot {
            status: BalanceSnapshotStatus::Error,
            ..balance_identity("routing-observer", "routing", "default")
        });
        let balances = HashMap::from([("routing".to_string(), snapshots)]);

        let brief = provider_balance_brief_lang(&balances, "routing", 42, Language::Zh);

        assert_eq!(brief, "daily 剩余 $93.83 / $100.00 不降级 4");
        assert!(!brief.contains('…'), "{brief}");
        assert!(brief.contains("$100.00"), "{brief}");
        assert!(UnicodeWidthStr::width(brief.as_str()) <= 42, "{brief}");

        let narrow = provider_balance_brief_lang(&balances, "routing", 14, Language::Zh);
        assert_eq!(narrow, "$93.83/$100.00");
        assert!(UnicodeWidthStr::width(narrow.as_str()) <= 14, "{narrow}");
    }

    #[test]
    fn session_observed_provider_balance_follows_canonical_provider() {
        let balances = HashMap::from([
            (
                "input".to_string(),
                vec![ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Ok,
                    unlimited_quota: Some(true),
                    ..balance_identity("input-observer", "input", "default")
                }],
            ),
            (
                "centos".to_string(),
                vec![ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Exhausted,
                    exhausted: Some(true),
                    exhaustion_affects_routing: false,
                    quota_period: Some("daily".to_string()),
                    quota_remaining_usd: Some("0".to_string()),
                    quota_limit_usd: Some("100".to_string()),
                    ..balance_identity("centos-observer", "centos", "default")
                }],
            ),
        ]);
        let mut row = empty_session_row();
        row.last_provider_id = Some("centos".to_string());

        let brief = session_observed_provider_balance_brief_lang(&row, &balances, 80, Language::En);

        assert_eq!(brief.as_deref(), Some("lazy daily left $0 / $100.00"));
        assert_eq!(
            session_observed_provider_balance_snapshot(&row, &balances)
                .map(|snapshot| snapshot.provider_endpoint.provider_id.as_str()),
            Some("centos")
        );
    }

    #[test]
    fn duration_short_uses_readable_units() {
        assert_eq!(duration_short(842), "842ms");
        assert_eq!(duration_short(1_250), "1.2s");
        assert_eq!(duration_short(12_000), "12s");
        assert_eq!(duration_short(83_000), "1m23s");
    }

    #[test]
    fn operator_projection_preserves_canonical_provider_inventory_without_auth_fields() {
        use crate::dashboard_core::{
            ApiV1OperatorSummary, OperatorPolicyActionSummary, OperatorProviderCapacity,
            OperatorProviderEndpointSummary, OperatorProviderSummary, OperatorRetrySummary,
            OperatorRuntimeSummary, OperatorSummaryCounts,
        };

        let data = OperatorReadData {
            summary: ApiV1OperatorSummary {
                api_version: 1,
                service_name: "codex".to_string(),
                runtime: OperatorRuntimeSummary::default(),
                counts: OperatorSummaryCounts::default(),
                retry: OperatorRetrySummary::default(),
                sessions: Vec::new(),
                profiles: Vec::new(),
                providers: vec![OperatorProviderSummary {
                    name: "alpha".to_string(),
                    alias: None,
                    configured_enabled: true,
                    effective_enabled: true,
                    routable_endpoints: 1,
                    endpoints: vec![OperatorProviderEndpointSummary {
                        provider_name: "alpha".to_string(),
                        name: "default".to_string(),
                        provider_endpoint_key: "endpoint:sha256:opaque".to_string(),
                        origin: Some("https://alpha.example".to_string()),
                        priority: 2,
                        configured_enabled: true,
                        effective_enabled: true,
                        routable: true,
                        runtime_enabled_override: None,
                        runtime_state: Default::default(),
                        runtime_state_override: None,
                        capacity: OperatorProviderCapacity::default(),
                        policy_actions: vec![OperatorPolicyActionSummary {
                            active_cooldown: true,
                            code: "cooldown".to_string(),
                            cooldown_remaining_secs: Some(42),
                        }],
                    }],
                    capacity: OperatorProviderCapacity::default(),
                }],
            },
            active_requests: Vec::new(),
            recent_requests: Vec::new(),
            usage_summaries: Vec::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            pricing_catalog: Default::default(),
            provider_balances: Vec::new(),
        };

        let providers = provider_options_from_operator_data(&data);
        assert_eq!(providers, data.summary.providers);
        assert_eq!(providers[0].endpoints[0].priority, 2);
        assert!(providers[0].endpoints[0].routable);
        let serialized = serde_json::to_string(&providers).expect("serialize providers");
        assert!(!serialized.contains("\"auth\""));
    }

    #[test]
    fn request_control_filter_matches_top_level_and_retry_evidence() {
        let top_signal = RequestControlEvidence {
            provider_signal_codes: vec!["rate_limit".to_string()],
            ..Default::default()
        };
        let top_action = RequestControlEvidence {
            policy_action_codes: vec!["cooldown".to_string()],
            ..Default::default()
        };
        let retry_signal = RequestControlEvidence {
            route_attempts: HashMap::from([(
                0,
                RequestAttemptControlEvidence {
                    provider_signal_codes: vec!["rate_limit".to_string()],
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        let retry_action = RequestControlEvidence {
            route_attempts: HashMap::from([(
                0,
                RequestAttemptControlEvidence {
                    policy_action_codes: vec!["cooldown".to_string()],
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };

        for evidence in [
            None,
            Some(&top_signal),
            Some(&top_action),
            Some(&retry_signal),
            Some(&retry_action),
        ] {
            assert!(request_matches_control_filter(
                evidence,
                RequestControlFilter::All
            ));
        }
        assert!(!request_matches_control_filter(
            None,
            RequestControlFilter::AnyEvidence
        ));
        for evidence in [&top_signal, &top_action, &retry_signal, &retry_action] {
            assert!(request_matches_control_filter(
                Some(evidence),
                RequestControlFilter::AnyEvidence
            ));
        }
        assert!(request_matches_control_filter(
            Some(&top_signal),
            RequestControlFilter::Signals
        ));
        assert!(request_matches_control_filter(
            Some(&retry_signal),
            RequestControlFilter::Signals
        ));
        assert!(!request_matches_control_filter(
            Some(&top_action),
            RequestControlFilter::Signals
        ));
        assert!(!request_matches_control_filter(
            Some(&retry_action),
            RequestControlFilter::Signals
        ));
        assert!(request_matches_control_filter(
            Some(&top_action),
            RequestControlFilter::Actions
        ));
        assert!(request_matches_control_filter(
            Some(&retry_action),
            RequestControlFilter::Actions
        ));
        assert!(!request_matches_control_filter(
            Some(&top_signal),
            RequestControlFilter::Actions
        ));
        assert!(!request_matches_control_filter(
            Some(&retry_signal),
            RequestControlFilter::Actions
        ));
    }

    #[test]
    fn request_page_focus_session_prefers_explicit_focus() {
        let snapshot = Snapshot {
            rows: vec![SessionRow {
                session_id: Some("sid-selected".to_string()),
                local_session_id: None,
                observation_scope: SessionObservationScope::ObservedOnly,
                host_local_transcript_path: None,
                last_client_name: None,
                last_client_addr: None,
                cwd: None,
                active_count: 0,
                active_started_at_ms_min: None,
                active_last_method: None,
                active_last_path: None,
                last_status: None,
                last_duration_ms: None,
                last_ended_at_ms: None,
                last_model: None,
                last_reasoning_effort: None,
                last_service_tier: None,
                last_provider_id: None,
                last_usage: None,
                total_usage: None,
                turns_total: None,
                turns_with_usage: None,
                last_output_tokens_per_second: None,
                avg_output_tokens_per_second: None,
                binding_profile_name: None,
                binding_continuity_mode: None,
                last_route_decision: None,
                route_affinity: None,
                effective_model: None,
                effective_reasoning_effort: None,
                effective_service_tier: None,
            }],
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        };

        let focused = request_page_focus_session_id(&snapshot, Some("sid-explicit"), 0);

        assert_eq!(focused.as_deref(), Some("sid-explicit"));
    }

    #[test]
    fn filtered_request_page_len_uses_explicit_focus() {
        let snapshot = Snapshot {
            rows: vec![SessionRow {
                session_id: Some("sid-selected".to_string()),
                local_session_id: None,
                observation_scope: SessionObservationScope::ObservedOnly,
                host_local_transcript_path: None,
                last_client_name: None,
                last_client_addr: None,
                cwd: None,
                active_count: 0,
                active_started_at_ms_min: None,
                active_last_method: None,
                active_last_path: None,
                last_status: None,
                last_duration_ms: None,
                last_ended_at_ms: None,
                last_model: None,
                last_reasoning_effort: None,
                last_service_tier: None,
                last_provider_id: None,
                last_usage: None,
                total_usage: None,
                turns_total: None,
                turns_with_usage: None,
                last_output_tokens_per_second: None,
                avg_output_tokens_per_second: None,
                binding_profile_name: None,
                binding_continuity_mode: None,
                last_route_decision: None,
                route_affinity: None,
                effective_model: None,
                effective_reasoning_effort: None,
                effective_service_tier: None,
            }],
            recent: vec![
                operator_request(1, Some("sid-selected")),
                operator_request(2, Some("sid-explicit")),
            ],
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        };

        let count = filtered_request_page_len(
            &snapshot,
            Some("sid-explicit"),
            0,
            false,
            true,
            RequestControlFilter::All,
        );

        assert_eq!(count, 1);
    }

    #[test]
    fn dashboard_request_len_scopes_unknown_session_row_to_unknown_requests() {
        let mut unknown_row = empty_session_row();
        unknown_row.session_id = None;
        let snapshot = Snapshot {
            rows: vec![unknown_row],
            recent: vec![
                operator_request(1, None),
                operator_request(2, Some("sid-known")),
            ],
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        };

        assert_eq!(filtered_requests_len(&snapshot, 0), 1);
    }

    #[test]
    fn request_page_focus_observation_distinguishes_history_only_session() {
        let snapshot = Snapshot {
            rows: vec![empty_session_row()],
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        };

        assert!(request_page_focus_is_runtime_observed(&snapshot, None));
        assert!(request_page_focus_is_runtime_observed(
            &snapshot,
            Some("sid")
        ));
        assert!(!request_page_focus_is_runtime_observed(
            &snapshot,
            Some("history-only")
        ));
    }

    #[test]
    fn build_session_rows_from_cards_skips_sessionless_activity() {
        let rows = build_session_rows_from_cards(&[
            SessionIdentityCard {
                session_id: None,
                active_count: 1,
                active_started_at_ms_min: Some(10),
                last_status: Some(200),
                ..SessionIdentityCard::default()
            },
            SessionIdentityCard {
                session_id: Some("sid-1".to_string()),
                ..SessionIdentityCard::default()
            },
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id.as_deref(), Some("sid-1"));
    }

    #[test]
    fn build_session_rows_from_cards_preserves_route_affinity() {
        let rows = build_session_rows_from_cards(&[SessionIdentityCard {
            session_id: Some("sid-1".to_string()),
            last_output_tokens_per_second: Some(123.4),
            avg_output_tokens_per_second: Some(98.7),
            route_affinity: Some(crate::state::SessionRouteAffinity {
                route_graph_key: "route:deadbeef".to_string(),
                session_identity_source: None,
                provider_endpoint: codex_helper_core::runtime_identity::ProviderEndpointKey::new(
                    "codex", "right", "default",
                ),
                upstream_base_url: "https://right.example/v1".to_string(),
                route_path: vec!["monthly_first".to_string(), "right".to_string()],
                last_selected_at_ms: 1_000,
                last_changed_at_ms: 900,
                change_reason: "failover_after_status_502".to_string(),
            }),
            ..SessionIdentityCard::default()
        }]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_output_tokens_per_second, Some(123.4));
        assert_eq!(rows[0].avg_output_tokens_per_second, Some(98.7));
        assert_eq!(
            rows[0]
                .route_affinity
                .as_ref()
                .map(|affinity| affinity.provider_id.as_str()),
            Some("right")
        );
        assert_eq!(
            rows[0]
                .route_affinity
                .as_ref()
                .map(|affinity| affinity.change_reason.as_str()),
            Some("failover_after_status_502")
        );
    }

    #[test]
    fn session_control_posture_reports_profile_binding() {
        let mut row = empty_session_row();
        row.binding_profile_name = Some("fast".to_string());

        let posture = session_control_posture(&row, false);

        assert!(posture.headline.contains("profile fast"));
        assert!(posture.detail.contains("stored profile binding"));
    }

    #[test]
    fn session_control_posture_reports_route_graph_fallback_context() {
        let row = empty_session_row();

        let posture = session_control_posture(&row, true);

        assert!(posture.headline.contains("no stored profile binding"));
        assert!(posture.detail.contains("route graph defaults"));
    }
}

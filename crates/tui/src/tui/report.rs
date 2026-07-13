use super::Language;
use super::i18n;
use super::model::tokens_short;
use super::state::{StatsProjectRowKind, UiState};
use super::types::StatsFocus;
use crate::quota_analytics::{PoolQuotaAnalytics, QuotaProjectRow};
use crate::quota_pool::{IdentityConfidence, QuotaQuantity, QuotaUnit};
use crate::state::{UsageBucket, UsageDayDimensionRow};

#[derive(Debug, Clone)]
pub(in crate::tui) enum StatsTarget {
    ProviderEndpoint(String, UsageBucket),
    Pool(PoolQuotaAnalytics),
    Project(PoolQuotaAnalytics, QuotaProjectRow),
    ProjectReconciliation(PoolQuotaAnalytics, String),
    Provider(String, UsageBucket),
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    total_ms
        .checked_div(n)
        .map(|avg| format!("{avg}ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn selected_stats_target_from_view(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
) -> Option<StatsTarget> {
    match ui.stats_focus {
        StatsFocus::ProviderEndpoints => snapshot
            .usage_day
            .provider_endpoint_rows
            .get(ui.selected_stats_provider_endpoint_idx)
            .map(|row| StatsTarget::ProviderEndpoint(row.name.clone(), row.bucket.clone())),
        StatsFocus::Pools => ui
            .selected_quota_pool(snapshot)
            .cloned()
            .map(StatsTarget::Pool),
        StatsFocus::Projects => {
            let (pool, row) = ui.selected_stats_project_row(snapshot)?;
            match row {
                StatsProjectRowKind::Project(index) => pool
                    .reconciliation
                    .projects
                    .get(index)
                    .cloned()
                    .map(|project| StatsTarget::Project(pool.clone(), project)),
                synthetic => project_reconciliation_target_label(ui.language, pool, synthetic)
                    .map(|label| StatsTarget::ProjectReconciliation(pool.clone(), label)),
            }
        }
        StatsFocus::Providers => snapshot
            .usage_day
            .provider_rows
            .get(ui.selected_stats_provider_idx)
            .map(|row| StatsTarget::Provider(row.name.clone(), row.bucket.clone())),
    }
}

fn project_reconciliation_target_label(
    language: Language,
    pool: &PoolQuotaAnalytics,
    row: StatsProjectRowKind,
) -> Option<String> {
    match row {
        StatsProjectRowKind::Project(_) => None,
        StatsProjectRowKind::Omitted => Some(match language {
            Language::Zh => format!("其余 {} 个项目", pool.reconciliation.omitted_projects),
            Language::En => format!("{} more projects", pool.reconciliation.omitted_projects),
        }),
        StatsProjectRowKind::LocalUnknown => Some(match language {
            Language::Zh => "本机未知项目".to_string(),
            Language::En => "local unknown project".to_string(),
        }),
        StatsProjectRowKind::ExternalUnattributed => Some(match language {
            Language::Zh => "外部 / 未归因".to_string(),
            Language::En => "external / unattributed".to_string(),
        }),
        StatsProjectRowKind::SignedGap => Some(match language {
            Language::Zh => "远端 - 本机差额".to_string(),
            Language::En => "remote - local gap".to_string(),
        }),
    }
}

pub(in crate::tui) fn build_stats_report(
    ui: &UiState,
    snapshot: &super::model::Snapshot,
    now_ms: u64,
) -> Option<String> {
    let usage = &snapshot.usage_day;
    let target = selected_stats_target_from_view(ui, snapshot)?;
    let (kind, name, target_bucket) = match &target {
        StatsTarget::Pool(pool) => (
            match ui.language {
                Language::Zh => "额度池",
                Language::En => "pool",
            }
            .to_string(),
            pool.identity.origin.clone(),
            None,
        ),
        StatsTarget::Project(_, project) => (
            i18n::label(ui.language, "project").to_string(),
            project.project.display_key().to_string(),
            None,
        ),
        StatsTarget::ProjectReconciliation(_, label) => (
            match ui.language {
                Language::Zh => "项目核对",
                Language::En => "project reconciliation",
            }
            .to_string(),
            label.clone(),
            None,
        ),
        StatsTarget::ProviderEndpoint(name, bucket) => (
            i18n::label(ui.language, "provider endpoint").to_string(),
            name.clone(),
            Some(bucket),
        ),
        StatsTarget::Provider(name, bucket) => (
            i18n::label(ui.language, "provider").to_string(),
            name.clone(),
            Some(bucket),
        ),
    };
    let l = |text| i18n::label(ui.language, text);

    let mut out = String::new();
    out.push_str(match ui.language {
        Language::Zh => "codex-helper TUI 今日用量报告\n",
        Language::En => "codex-helper TUI Daily Usage report\n",
    });
    out.push_str(&format!("generated_at_ms: {now_ms}\n"));
    out.push_str(&format!("service: {}\n", ui.service_name));
    out.push_str(&format!("day: {} ({})\n", usage.label, usage.day));
    out.push_str(&format!(
        "pricing_catalog: {} ({} models)\n",
        snapshot.pricing_catalog.source, snapshot.pricing_catalog.model_count
    ));
    out.push_str(&format!("{}: {kind} {name}\n", l("target")));
    out.push('\n');

    out.push_str(match ui.language {
        Language::Zh => "[今日汇总]\n",
        Language::En => "[day summary]\n",
    });
    append_bucket(&mut out, ui.language, &usage.summary, "total");
    if let Some(target_bucket) = target_bucket {
        append_bucket(&mut out, ui.language, target_bucket, &name);
    }
    out.push('\n');

    if let Some(pool) = match &target {
        StatsTarget::Pool(pool)
        | StatsTarget::Project(pool, _)
        | StatsTarget::ProjectReconciliation(pool, _) => Some(pool),
        StatsTarget::ProviderEndpoint(_, _) | StatsTarget::Provider(_, _) => {
            ui.selected_quota_pool(snapshot)
        }
    } {
        append_quota_pool(&mut out, pool);
    }

    out.push_str(match ui.language {
        Language::Zh => "[覆盖范围]\n",
        Language::En => "[coverage]\n",
    });
    out.push_str(&format!(
        "source={} loaded_requests={} first_ms={} last_ms={}\n",
        usage.coverage.source,
        usage.coverage.loaded_requests,
        usage
            .coverage
            .loaded_first_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        usage
            .coverage
            .loaded_last_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    out.push_str(&format!(
        "partial={} reason={}\n",
        usage.coverage.day_may_be_partial,
        usage.coverage.partial_reason.as_deref().unwrap_or("-")
    ));
    out.push('\n');

    out.push_str(match ui.language {
        Language::Zh => "[Retry Gate]\n",
        Language::En => "[retry gate]\n",
    });
    out.push_str(&format!(
        "active={} cooldown={} max_remaining_secs={}\n",
        usage.retry_gate.active,
        usage.retry_gate.active_cooldowns,
        usage
            .retry_gate
            .max_remaining_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    ));
    for row in &usage.retry_gate.reasons {
        out.push_str(&format!("  - {}: {}\n", row.reason, row.active));
    }
    out.push('\n');

    append_dimension(&mut out, ui.language, "providers", &usage.provider_rows);
    append_dimension(
        &mut out,
        ui.language,
        "provider endpoints",
        &usage.provider_endpoint_rows,
    );
    append_dimension(&mut out, ui.language, "models", &usage.model_rows);
    append_dimension(&mut out, ui.language, "sessions", &usage.session_rows);
    append_dimension(&mut out, ui.language, "projects", &usage.project_rows);

    Some(out)
}

fn append_quota_pool(out: &mut String, pool: &PoolQuotaAnalytics) {
    out.push_str("[quota pool]\n");
    out.push_str(&format!(
        "origin={} source={} scope={} confidence={} aggregation_eligible={} unit={} freshness={:?}\n",
        pool.identity.origin,
        pool.source,
        pool.identity.scope.as_key(),
        confidence_text(pool.identity.confidence),
        pool.identity.aggregation_eligible,
        pool.unit.as_str(),
        pool.freshness
    ));
    out.push_str(&format!(
        "observed_at_ms={} epoch_start_ms={} epoch_end_ms={} reset_at_ms={}\n",
        pool.observed_at_ms,
        pool.epoch_start_ms,
        optional_u64(pool.epoch_end_ms),
        optional_u64(pool.pacing.reset_at_ms)
    ));
    out.push_str(&format!(
        "window={:?} reset_semantics={:?} reset_timezone={} conversion_source={} conversion_generation={}\n",
        pool.window.kind,
        pool.window.reset,
        pool.window.reset_timezone.as_deref().unwrap_or("-"),
        pool.conversion
            .as_ref()
            .map(|conversion| format!("{:?}", conversion.source))
            .unwrap_or_else(|| "-".to_string()),
        pool.conversion
            .as_ref()
            .and_then(|conversion| conversion.generation)
            .map_or_else(|| "-".to_string(), |generation| generation.to_string())
    ));
    out.push_str(&format!(
        "used={} direct_total={} observed_burn={} remaining={} limit={}\n",
        quantity_report(pool.remote_used.as_ref()),
        quantity_report(pool.remote_direct_total.as_ref()),
        quantity_report(pool.observed_burn.as_ref()),
        quantity_report(pool.remote_remaining.as_ref()),
        quantity_report(pool.remote_limit.as_ref())
    ));
    out.push_str(&format!(
        "rate_15m={} status_15m={:?} samples_15m={} span_15m_ms={} lower_bound_15m={} rate_60m={} status_60m={:?} samples_60m={} span_60m_ms={} lower_bound_60m={}\n",
        quantity_report(pool.rate_15m.rate_per_hour.as_ref()),
        pool.rate_15m.status,
        pool.rate_15m.sample_count,
        pool.rate_15m.span_ms,
        pool.rate_15m.lower_bound,
        quantity_report(pool.rate_60m.rate_per_hour.as_ref()),
        pool.rate_60m.status,
        pool.rate_60m.sample_count,
        pool.rate_60m.span_ms,
        pool.rate_60m.lower_bound
    ));
    out.push_str(&format!(
        "pace={:?} required_rate={} pace_ratio_basis_points={} exhaustion_eta_ms={} projected_remaining_at_reset={}\n",
        pool.pacing.status,
        quantity_report(pool.pacing.required_rate_per_hour.as_ref()),
        optional_u32(pool.pacing.pace_ratio_basis_points),
        optional_u64(pool.pacing.exhaustion_eta_ms),
        quantity_report(pool.pacing.projected_remaining_at_reset.as_ref())
    ));
    let reconciliation = &pool.reconciliation;
    out.push_str(&format!(
        "reconciliation={:?} remote={} local_known={} local_unknown={} external_unattributed={} signed_gap={} omitted_projects={} omitted_local_known={}\n",
        reconciliation.status,
        quantity_report(reconciliation.remote_total.as_ref()),
        quantity_report(reconciliation.local_known.as_ref()),
        quantity_report(reconciliation.local_unknown.as_ref()),
        quantity_report(reconciliation.external_unattributed.as_ref()),
        reconciliation
            .signed_delta
            .map(|value| value.format_usd())
            .unwrap_or_else(|| "-".to_string()),
        reconciliation.omitted_projects,
        quantity_report(reconciliation.omitted_local_known.as_ref())
    ));
    out.push_str(&format!(
        "coverage_loaded_first_ms={} coverage_loaded_last_ms={} coverage_queried_first_ms={} coverage_queried_last_ms={}\n",
        optional_u64(reconciliation.coverage.loaded_first_ms),
        optional_u64(reconciliation.coverage.loaded_last_ms),
        optional_u64(reconciliation.coverage.queried_first_ms),
        optional_u64(reconciliation.coverage.queried_last_ms)
    ));
    out.push_str(&format!(
        "coverage_time_truncated={} count_truncated={} dedupe_truncated={} boundary_partial={} leading_boundary_partial={} trailing_boundary_partial={} cost_overflow={}\n",
        reconciliation.coverage.time_truncated,
        reconciliation.coverage.count_truncated,
        reconciliation.coverage.dedupe_truncated,
        reconciliation.coverage.boundary_partial,
        reconciliation.coverage.leading_boundary_partial,
        reconciliation.coverage.trailing_boundary_partial,
        reconciliation.coverage.cost_overflow
    ));
    out.push_str(&format!(
        "coverage_duplicate_requests={} partial_captured_price={} reconstructed_price={} invalid_captured_price={} unpriced={} unmatched_endpoint={} unmatched_pool={} unknown_project={} complete_for_reconciliation={}\n",
        reconciliation.coverage.duplicate_requests,
        reconciliation.coverage.partial_captured_price_requests,
        reconciliation.coverage.reconstructed_price_requests,
        reconciliation.coverage.invalid_captured_price_requests,
        reconciliation.coverage.unpriced_requests,
        reconciliation.coverage.unmatched_endpoint_requests,
        reconciliation.coverage.unmatched_pool_requests,
        reconciliation.coverage.unknown_project_requests,
        reconciliation.coverage.complete_for_reconciliation()
    ));
    for project in &reconciliation.projects {
        out.push_str(&format!(
            "  project={} local_cost={} requests={}\n",
            project.project.display_key(),
            quantity_report(Some(&project.local_cost)),
            project.requests
        ));
    }
    out.push('\n');
}

fn quantity_report(quantity: Option<&QuotaQuantity>) -> String {
    let Some(quantity) = quantity else {
        return "-".to_string();
    };
    let decimal = decimal_report(quantity.value, quantity.scale).unwrap_or_else(|| "-".to_string());
    match quantity.unit {
        QuotaUnit::Usd => format!("${decimal}"),
        QuotaUnit::Tokens => format!("{decimal} tokens"),
        QuotaUnit::Raw => format!("{decimal} raw"),
        QuotaUnit::Unknown => format!("{decimal} unknown"),
    }
}

fn decimal_report(value: i128, scale: u32) -> Option<String> {
    let scale = usize::try_from(scale).ok()?;
    if scale > 38 {
        return None;
    }
    let negative = value < 0;
    let mut digits = value.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits.insert_str(0, &"0".repeat(scale + 1 - digits.len()));
        }
        digits.insert(digits.len() - scale, '.');
    }
    Some(if negative {
        format!("-{digits}")
    } else {
        digits
    })
}

fn confidence_text(confidence: IdentityConfidence) -> &'static str {
    match confidence {
        IdentityConfidence::High => "high",
        IdentityConfidence::Medium => "medium",
        IdentityConfidence::Low => "low",
        IdentityConfidence::Unknown => "unknown",
    }
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn optional_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn append_bucket(out: &mut String, lang: Language, bucket: &UsageBucket, label: &str) {
    let l = |text| i18n::label(lang, text);
    out.push_str(&format!(
        "{label}: {}={} {}={} {}={} {}={} {}={} {}\n",
        l("requests"),
        bucket.requests_total,
        l("errors"),
        bucket.requests_error,
        l("error rate"),
        fmt_pct(bucket.requests_error, bucket.requests_total),
        l("avg"),
        fmt_avg_ms(bucket.duration_ms_total, bucket.requests_total),
        l("tokens"),
        tokens_short(bucket.usage.total_tokens),
        cost_text(bucket)
    ));
}

fn cost_text(bucket: &UsageBucket) -> String {
    format!(
        "cost={} priced={} unpriced={}",
        bucket.cost.display_total(),
        bucket.cost.priced_requests,
        bucket.cost.unpriced_requests
    )
}

fn append_dimension(
    out: &mut String,
    lang: Language,
    title: &'static str,
    rows: &[UsageDayDimensionRow],
) {
    let display_title = match (lang, title) {
        (Language::Zh, "providers") => "providers",
        (Language::Zh, "provider endpoints") => "provider endpoints",
        (Language::Zh, "models") => "models",
        (Language::Zh, "sessions") => "sessions",
        (Language::Zh, "projects") => "projects",
        _ => title,
    };
    out.push_str(&format!("[{display_title}]\n"));
    if rows.is_empty() {
        out.push_str("  -\n");
        return;
    }
    for row in rows.iter().take(30) {
        out.push_str(&format!(
            "  - {}: req={} err={} tok={} {}\n",
            row.name,
            row.bucket.requests_total,
            row.bucket.requests_error,
            tokens_short(row.bucket.usage.total_tokens),
            cost_text(&row.bucket)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_analytics::SignedUsdDelta;
    use crate::quota_analytics::{QuotaReconciliationStatus, QuotaReconciliationView};
    use crate::quota_pool::{
        ConversionSource, QuotaConversion, QuotaWindowKind, QuotaWindowSemantics,
    };
    #[test]
    fn stats_report_includes_operator_pricing_catalog_provenance() {
        let mut snapshot = crate::tui::model::Snapshot::default();
        snapshot.usage_day.provider_rows = vec![UsageDayDimensionRow {
            name: "relay".to_string(),
            bucket: UsageBucket::default(),
        }];
        snapshot.pricing_catalog.source = "remote".to_string();
        snapshot.pricing_catalog.model_count = 42;
        let ui = UiState {
            stats_focus: StatsFocus::Providers,
            ..UiState::default()
        };

        let report = build_stats_report(&ui, &snapshot, 7).expect("stats report");

        assert!(report.contains("pricing_catalog: remote (42 models)"));
    }

    #[test]
    fn quota_report_preserves_signed_negative_gap() {
        let mut pool = PoolQuotaAnalytics {
            unit: QuotaUnit::Usd,
            reconciliation: QuotaReconciliationView {
                status: QuotaReconciliationStatus::Available,
                remote_total: Some(QuotaQuantity::from_integer(50, QuotaUnit::Usd)),
                local_known: Some(QuotaQuantity::from_integer(60, QuotaUnit::Usd)),
                external_unattributed: Some(QuotaQuantity::from_integer(0, QuotaUnit::Usd)),
                signed_delta: Some(SignedUsdDelta::from_femto_usd(-10 * 10_i128.pow(15))),
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        };
        pool.identity.origin = "https://relay.example".to_string();

        let mut report = String::new();
        append_quota_pool(&mut report, &pool);

        assert!(report.contains("remote=$50"), "{report}");
        assert!(report.contains("local_known=$60"), "{report}");
        assert!(report.contains("external_unattributed=$0"), "{report}");
        assert!(report.contains("signed_gap=-10"), "{report}");
    }

    #[test]
    fn quota_report_keeps_raw_and_conversion_mismatch_semantics_visible() {
        let mut pool = PoolQuotaAnalytics {
            unit: QuotaUnit::Raw,
            remote_direct_total: Some(QuotaQuantity::from_integer(500_000, QuotaUnit::Raw)),
            conversion: Some(QuotaConversion {
                source: ConversionSource::Remote,
                divisor: Some(500_000),
                generation: Some(41),
            }),
            window: QuotaWindowSemantics {
                kind: QuotaWindowKind::Resetless,
                ..QuotaWindowSemantics::default()
            },
            reconciliation: QuotaReconciliationView {
                status: QuotaReconciliationStatus::IncompatibleGeneration,
                remote_total: Some(QuotaQuantity::from_integer(500_000, QuotaUnit::Raw)),
                local_known: Some(
                    QuotaQuantity::from_integer(1, QuotaUnit::Usd)
                        .with_conversion_generation(Some(42)),
                ),
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        };
        pool.identity.origin = "https://relay.example".to_string();

        let mut report = String::new();
        append_quota_pool(&mut report, &pool);

        assert!(report.contains("window=Resetless"), "{report}");
        assert!(report.contains("conversion_source=Remote"), "{report}");
        assert!(report.contains("conversion_generation=41"), "{report}");
        assert!(report.contains("direct_total=500000 raw"), "{report}");
        assert!(
            report.contains("reconciliation=IncompatibleGeneration"),
            "{report}"
        );
        assert!(report.contains("local_known=$1"), "{report}");
        assert!(report.contains("signed_gap=-"), "{report}");
    }

    #[test]
    fn quota_report_exports_complete_coverage_and_omission_details() {
        let coverage = crate::state::AttributionCoverage {
            loaded_first_ms: Some(10),
            queried_last_ms: Some(20),
            dedupe_truncated: true,
            leading_boundary_partial: true,
            cost_overflow: true,
            reconstructed_price_requests: 2,
            invalid_captured_price_requests: 3,
            duplicate_requests: 4,
            ..crate::state::AttributionCoverage::default()
        };
        let pool = PoolQuotaAnalytics {
            reconciliation: QuotaReconciliationView {
                omitted_projects: 5,
                omitted_local_known: Some(QuotaQuantity::from_integer(6, QuotaUnit::Usd)),
                coverage,
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        };

        let mut report = String::new();
        append_quota_pool(&mut report, &pool);

        assert!(report.contains("omitted_projects=5 omitted_local_known=$6"));
        assert!(report.contains("coverage_loaded_first_ms=10"));
        assert!(report.contains("dedupe_truncated=true"));
        assert!(report.contains("leading_boundary_partial=true"));
        assert!(report.contains("cost_overflow=true"));
        assert!(report.contains("reconstructed_price=2"));
        assert!(report.contains("invalid_captured_price=3"));
        assert!(report.contains("coverage_duplicate_requests=4"));
        assert!(report.contains("complete_for_reconciliation=false"));
    }

    #[test]
    fn quota_report_exports_partial_captured_price_as_incomplete() {
        let pool = PoolQuotaAnalytics {
            reconciliation: QuotaReconciliationView {
                coverage: crate::state::AttributionCoverage {
                    partial_captured_price_requests: 7,
                    ..crate::state::AttributionCoverage::default()
                },
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        };

        let mut report = String::new();
        append_quota_pool(&mut report, &pool);

        assert!(report.contains("partial_captured_price=7"), "{report}");
        assert!(
            report.contains("complete_for_reconciliation=false"),
            "{report}"
        );
    }

    #[test]
    fn selected_reconciliation_rows_export_explicit_report_targets() {
        let mut snapshot = crate::tui::model::Snapshot::default();
        snapshot.quota_analytics.pools = vec![PoolQuotaAnalytics {
            reconciliation: QuotaReconciliationView {
                omitted_projects: 2,
                omitted_local_known: Some(QuotaQuantity::from_integer(3, QuotaUnit::Usd)),
                local_unknown: Some(QuotaQuantity::from_integer(4, QuotaUnit::Usd)),
                external_unattributed: Some(QuotaQuantity::from_integer(5, QuotaUnit::Usd)),
                signed_delta: Some(SignedUsdDelta::from_femto_usd(6 * 10_i128.pow(15))),
                ..QuotaReconciliationView::default()
            },
            ..PoolQuotaAnalytics::default()
        }];

        for (index, expected_target) in [
            (0, "target: project reconciliation 2 more projects"),
            (1, "target: project reconciliation local unknown project"),
            (2, "target: project reconciliation external / unattributed"),
            (3, "target: project reconciliation remote - local gap"),
        ] {
            let mut ui = UiState {
                stats_focus: StatsFocus::Projects,
                selected_stats_project_idx: index,
                ..UiState::default()
            };
            ui.clamp_selection(&snapshot, 0);

            let report = build_stats_report(&ui, &snapshot, 7).expect("stats report");

            assert!(report.contains(expected_target), "{report}");
            assert!(!report.contains("target: pool"), "{report}");
        }
    }
}

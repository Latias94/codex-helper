use super::*;

#[derive(Debug, Clone)]
struct StationBalanceSummary {
    station_name: String,
    total_rows: usize,
    ok_rows: usize,
    stale_rows: usize,
    exhausted_rows: usize,
    error_rows: usize,
    unknown_rows: usize,
    primary: Option<crate::state::ProviderBalanceSnapshot>,
}

pub(super) fn render_balance_overview(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };

    let mut stations = summarize_station_balances(&snapshot.provider_balances);
    if stations.is_empty() {
        return;
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
    let stale_rows = stations
        .iter()
        .map(|station| station.stale_rows)
        .sum::<usize>();
    ui.separator();
    ui.label(pick(ctx.lang, "余额 / 额度", "Balance / quota"));
    let unknown_rows = stations
        .iter()
        .map(|station| station.unknown_rows + station.error_rows)
        .sum::<usize>();

    ui.label(format!(
        "stations={}  rows={}  exhausted={}  stale={}  unknown={}",
        stations.len(),
        total_rows,
        exhausted_rows,
        stale_rows,
        unknown_rows
    ));

    egui::ScrollArea::vertical()
        .id_salt("stats_balance_scroll")
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new("stats_balance_grid")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    ui.label(pick(ctx.lang, "站点", "Station"));
                    ui.label(pick(ctx.lang, "统计", "Counts"));
                    ui.label(pick(ctx.lang, "主快照", "Primary"));
                    ui.end_row();

                    for station in &stations {
                        ui.label(shorten(&station.station_name, 24));
                        ui.label(format!(
                            "rows={} ok={} stale={} exhausted={} unknown={}",
                            station.total_rows,
                            station.ok_rows,
                            station.stale_rows,
                            station.exhausted_rows,
                            station.unknown_rows + station.error_rows
                        ));
                        ui.label(shorten(
                            station
                                .primary
                                .as_ref()
                                .map(format_primary_balance)
                                .unwrap_or_else(|| "-".to_string())
                                .as_str(),
                            96,
                        ));
                        ui.end_row();
                    }
                });
        });
}

fn summarize_station_balances(
    provider_balances: &std::collections::HashMap<
        String,
        Vec<crate::state::ProviderBalanceSnapshot>,
    >,
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
                .filter(|balance| balance.status == crate::state::BalanceSnapshotStatus::Exhausted)
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
    balance_status_rank(left.status)
        .cmp(&balance_status_rank(right.status))
        .then_with(|| left.upstream_index.cmp(&right.upstream_index))
        .then_with(|| left.provider_id.cmp(&right.provider_id))
        .then_with(|| left.fetched_at_ms.cmp(&right.fetched_at_ms))
}

fn balance_status_rank(status: crate::state::BalanceSnapshotStatus) -> u8 {
    match status {
        crate::state::BalanceSnapshotStatus::Exhausted => 0,
        crate::state::BalanceSnapshotStatus::Stale => 1,
        crate::state::BalanceSnapshotStatus::Error
        | crate::state::BalanceSnapshotStatus::Unknown => 2,
        crate::state::BalanceSnapshotStatus::Ok => 3,
    }
}

fn station_priority(summary: &StationBalanceSummary) -> u8 {
    summary
        .primary
        .as_ref()
        .map(|snapshot| balance_status_rank(snapshot.status))
        .unwrap_or(5)
}

fn format_primary_balance(snapshot: &crate::state::ProviderBalanceSnapshot) -> String {
    let mut line = format!(
        "{}  #{}  {}  {}",
        shorten_middle(&snapshot.provider_id, 20),
        snapshot
            .upstream_index
            .map(|idx| idx.to_string())
            .unwrap_or_else(|| "-".to_string()),
        balance_status_label(snapshot.status),
        snapshot.amount_summary()
    );
    if let Some(err) = snapshot.error.as_deref()
        && !err.trim().is_empty()
    {
        line.push_str(&format!("  lookup_failed={}", shorten(err, 48)));
    }
    line
}

fn balance_status_label(status: crate::state::BalanceSnapshotStatus) -> &'static str {
    match status {
        crate::state::BalanceSnapshotStatus::Ok => "ok",
        crate::state::BalanceSnapshotStatus::Exhausted => "exhausted",
        crate::state::BalanceSnapshotStatus::Stale => "stale",
        crate::state::BalanceSnapshotStatus::Error
        | crate::state::BalanceSnapshotStatus::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_station_balances_prioritizes_problematic_stations() {
        let mut provider_balances = std::collections::HashMap::new();
        provider_balances.insert(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: "alpha-provider".to_string(),
                station_name: Some("alpha".to_string()),
                upstream_index: Some(0),
                status: crate::state::BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("4".to_string()),
                ..Default::default()
            }],
        );
        provider_balances.insert(
            "beta".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                provider_id: "beta-provider".to_string(),
                station_name: Some("beta".to_string()),
                upstream_index: Some(0),
                status: crate::state::BalanceSnapshotStatus::Exhausted,
                total_balance_usd: Some("0".to_string()),
                exhausted: Some(true),
                ..Default::default()
            }],
        );

        let mut summaries = summarize_station_balances(&provider_balances);
        summaries.sort_by(|left, right| {
            station_priority(left)
                .cmp(&station_priority(right))
                .then_with(|| left.station_name.cmp(&right.station_name))
        });

        assert_eq!(summaries[0].station_name, "beta");
        assert_eq!(
            summaries[0]
                .primary
                .as_ref()
                .map(|snapshot| snapshot.status),
            Some(crate::state::BalanceSnapshotStatus::Exhausted)
        );
    }
}

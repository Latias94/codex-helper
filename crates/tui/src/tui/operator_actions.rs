use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::control_plane_client::LocalOperatorClient;
use crate::proxy::{
    OperatorDefaultProfileMutationRequest, OperatorDefaultProfileMutationResponse,
    OperatorDefaultProfileMutationStatus, OperatorDefaultProfileScope, OperatorEndpointMode,
    OperatorRoutingCommand, OperatorRoutingMutationRequest, OperatorRoutingMutationResponse,
    OperatorRoutingMutationStatus, OperatorSessionAffinityCommand,
    OperatorSessionAffinityMutationRequest, OperatorSessionAffinityMutationResponse,
    OperatorSessionAffinityMutationStatus, OperatorSessionBindingCommand,
    OperatorSessionBindingMutationRequest, OperatorSessionBindingMutationResponse,
    OperatorSessionBindingMutationStatus, ProxyService,
};
use crate::usage_providers::UsageProviderRefreshSummary;

use super::Language;
use super::settings_relay::{
    CodexRelayDiagnosticsCompletion, CodexRelayDiagnosticsStart, CodexRelayLiveSmokeCompletion,
    CodexRelayLiveSmokeStart,
};
use super::state::UiState;

#[derive(Debug, Clone)]
pub(in crate::tui) enum PendingOperatorAction {
    RefreshBalances { force: bool },
    MutateRouting(OperatorRoutingMutationRequest),
    MutateSessionAffinity(OperatorSessionAffinityMutationRequest),
    MutateSessionBinding(OperatorSessionBindingMutationRequest),
    ReloadRuntime,
    MutateDefaultProfile(OperatorDefaultProfileMutationRequest),
    InspectRelayCapabilities(CodexRelayDiagnosticsStart),
    RunRelayLiveSmoke(CodexRelayLiveSmokeStart),
}

#[derive(Debug)]
pub(super) enum OperatorActionOutcome {
    RefreshBalances(Result<UsageProviderRefreshSummary, String>),
    MutateRouting {
        command: OperatorRoutingCommand,
        result: Result<OperatorRoutingMutationResponse, String>,
    },
    MutateSessionAffinity {
        command: OperatorSessionAffinityCommand,
        result: Result<OperatorSessionAffinityMutationResponse, String>,
    },
    MutateSessionBinding {
        command: OperatorSessionBindingCommand,
        result: Result<OperatorSessionBindingMutationResponse, String>,
    },
    ReloadRuntime(Result<crate::proxy::OperatorRuntimeReloadResponse, String>),
    MutateDefaultProfile {
        scope: OperatorDefaultProfileScope,
        result: Result<OperatorDefaultProfileMutationResponse, String>,
    },
    InspectRelayCapabilities(Box<CodexRelayDiagnosticsCompletion>),
    RunRelayLiveSmoke(Box<CodexRelayLiveSmokeCompletion>),
}

pub(super) type OperatorActionSender = mpsc::UnboundedSender<OperatorActionOutcome>;

const AUTO_BALANCE_REFRESH_COOLDOWN: Duration = Duration::from_secs(10);
const FORCED_BALANCE_REFRESH_COOLDOWN: Duration = Duration::from_secs(2);

pub(in crate::tui) fn queue_balance_refresh(ui: &mut UiState, force: bool, announce: bool) -> bool {
    queue_balance_refresh_with_cooldown(ui, force, announce, true)
}

fn queue_balance_refresh_with_cooldown(
    ui: &mut UiState,
    force: bool,
    announce: bool,
    enforce_cooldown: bool,
) -> bool {
    if !ui.can_refresh_provider_balances() {
        if announce {
            ui.toast = Some((read_only_action_message(ui), Instant::now()));
        }
        return false;
    }
    if enforce_cooldown {
        let cooldown = if force {
            FORCED_BALANCE_REFRESH_COOLDOWN
        } else {
            AUTO_BALANCE_REFRESH_COOLDOWN
        };
        if ui
            .last_balance_refresh_requested_at
            .is_some_and(|last| last.elapsed() < cooldown)
        {
            return false;
        }
    }
    if !force && !announce && (ui.operator_action_in_flight || ui.pending_operator_action.is_some())
    {
        ui.deferred_auto_balance_refresh = true;
        return true;
    }
    if ui.pending_operator_action.is_some() {
        if announce {
            ui.toast = Some((
                match ui.language {
                    Language::Zh => "已有本地操作正在进行".to_string(),
                    Language::En => "a local operator action is already in progress".to_string(),
                },
                Instant::now(),
            ));
        }
        return false;
    }
    let queued_behind_in_flight = ui.operator_action_in_flight;
    ui.pending_operator_action = Some(PendingOperatorAction::RefreshBalances { force });
    ui.deferred_auto_balance_refresh = false;
    if announce {
        ui.toast = Some((
            match (ui.language, queued_behind_in_flight) {
                (Language::Zh, true) => "余额/额度：已排队，将在当前操作后全量刷新".to_string(),
                (Language::En, true) => {
                    "balance/quota: full refresh queued behind the current action".to_string()
                }
                (Language::Zh, false) => "余额/额度：开始全量刷新".to_string(),
                (Language::En, false) => "balance/quota: starting full refresh".to_string(),
            },
            Instant::now(),
        ));
    }
    true
}

pub(in crate::tui) fn queue_session_affinity_mutation(
    ui: &mut UiState,
    request: OperatorSessionAffinityMutationRequest,
) -> bool {
    if !ui.can_mutate_session_affinity() {
        ui.toast = Some((read_only_action_message(ui), Instant::now()));
        return false;
    }
    if ui.pending_operator_action.is_some() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "已有本地操作正在进行".to_string(),
                Language::En => "a local operator action is already in progress".to_string(),
            },
            Instant::now(),
        ));
        return false;
    }
    let queued_behind_in_flight = ui.operator_action_in_flight;
    ui.pending_operator_action = Some(PendingOperatorAction::MutateSessionAffinity(request));
    if queued_behind_in_flight {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "会话 affinity 变更已排队，将在当前本机操作后执行".to_string(),
                Language::En => {
                    "session affinity change queued behind the current local action".to_string()
                }
            },
            Instant::now(),
        ));
    }
    true
}

pub(in crate::tui) fn queue_session_binding_mutation(
    ui: &mut UiState,
    request: OperatorSessionBindingMutationRequest,
) -> bool {
    if !ui.can_mutate_session_binding() {
        ui.toast = Some((read_only_action_message(ui), Instant::now()));
        return false;
    }
    if ui.pending_operator_action.is_some() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "已有本地操作正在进行".to_string(),
                Language::En => "a local operator action is already in progress".to_string(),
            },
            Instant::now(),
        ));
        return false;
    }
    let queued_behind_in_flight = ui.operator_action_in_flight;
    ui.pending_operator_action = Some(PendingOperatorAction::MutateSessionBinding(request));
    if queued_behind_in_flight {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "会话控制变更已排队，将在当前本机操作后执行".to_string(),
                Language::En => {
                    "session control change queued behind the current local action".to_string()
                }
            },
            Instant::now(),
        ));
    }
    true
}

pub(in crate::tui) fn queue_routing_mutation(
    ui: &mut UiState,
    request: OperatorRoutingMutationRequest,
) -> bool {
    if !ui.can_mutate_routing() {
        ui.toast = Some((read_only_action_message(ui), Instant::now()));
        return false;
    }
    if ui.pending_operator_action.is_some() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "已有本地操作正在进行".to_string(),
                Language::En => "a local operator action is already in progress".to_string(),
            },
            Instant::now(),
        ));
        return false;
    }
    let queued_behind_in_flight = ui.operator_action_in_flight;
    ui.pending_operator_action = Some(PendingOperatorAction::MutateRouting(request));
    if queued_behind_in_flight {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "路由变更已排队，将在当前本机操作后执行".to_string(),
                Language::En => "routing change queued behind the current local action".to_string(),
            },
            Instant::now(),
        ));
    }
    true
}

fn queue_settings_action(ui: &mut UiState, allowed: bool, action: PendingOperatorAction) -> bool {
    if !allowed {
        ui.toast = Some((read_only_action_message(ui), Instant::now()));
        return false;
    }
    if ui.pending_operator_action.is_some() {
        ui.toast = Some((
            match ui.language {
                Language::Zh => "已有本地操作正在进行".to_string(),
                Language::En => "a local operator action is already in progress".to_string(),
            },
            Instant::now(),
        ));
        return false;
    }
    ui.pending_operator_action = Some(action);
    true
}

pub(in crate::tui) fn queue_runtime_reload(ui: &mut UiState) -> bool {
    queue_settings_action(
        ui,
        ui.can_reload_runtime(),
        PendingOperatorAction::ReloadRuntime,
    )
}

pub(in crate::tui) fn queue_default_profile_mutation(
    ui: &mut UiState,
    request: OperatorDefaultProfileMutationRequest,
) -> bool {
    queue_settings_action(
        ui,
        ui.can_mutate_default_profile(),
        PendingOperatorAction::MutateDefaultProfile(request),
    )
}

pub(in crate::tui) fn queue_relay_capabilities(
    ui: &mut UiState,
    start: CodexRelayDiagnosticsStart,
) -> bool {
    let queued = queue_settings_action(
        ui,
        ui.can_inspect_relay_capabilities(),
        PendingOperatorAction::InspectRelayCapabilities(start),
    );
    if !queued {
        ui.codex_relay_diagnostics.loading = false;
    }
    queued
}

pub(in crate::tui) fn queue_relay_live_smoke(
    ui: &mut UiState,
    start: CodexRelayLiveSmokeStart,
) -> bool {
    let queued = queue_settings_action(
        ui,
        ui.can_run_relay_live_smoke(),
        PendingOperatorAction::RunRelayLiveSmoke(start),
    );
    if !queued {
        ui.codex_relay_live_smoke.loading = false;
    }
    queued
}

pub(super) fn start_integrated_operator_action(
    ui: &mut UiState,
    proxy: &ProxyService,
    tx: OperatorActionSender,
) -> bool {
    if ui.operator_action_in_flight {
        return false;
    }
    let Some(action) = ui.pending_operator_action.take() else {
        return false;
    };
    ui.operator_action_in_flight = true;
    let proxy = proxy.clone();
    match action {
        PendingOperatorAction::RefreshBalances { force } => {
            ui.last_balance_refresh_requested_at = Some(Instant::now());
            ui.balance_refresh_in_flight = true;
            tokio::spawn(async move {
                let result = proxy
                    .refresh_provider_balances(None, None, force)
                    .await
                    .map(|response| response.refresh)
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::RefreshBalances(result));
            });
        }
        PendingOperatorAction::MutateRouting(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = proxy
                    .mutate_operator_routing(request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateRouting { command, result });
            });
        }
        PendingOperatorAction::MutateSessionAffinity(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = proxy
                    .mutate_operator_session_affinity(request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateSessionAffinity { command, result });
            });
        }
        PendingOperatorAction::MutateSessionBinding(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = proxy
                    .mutate_operator_session_binding(request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateSessionBinding { command, result });
            });
        }
        PendingOperatorAction::ReloadRuntime => {
            tokio::spawn(async move {
                let result = proxy
                    .operator_runtime_reload(crate::proxy::OperatorRuntimeReloadRequest {})
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::ReloadRuntime(result));
            });
        }
        PendingOperatorAction::MutateDefaultProfile(request) => {
            let scope = request.scope;
            tokio::spawn(async move {
                let result = proxy
                    .mutate_operator_default_profile(request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateDefaultProfile { scope, result });
            });
        }
        PendingOperatorAction::InspectRelayCapabilities(start) => {
            tokio::spawn(async move {
                let result = proxy
                    .codex_relay_capabilities(start.request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::InspectRelayCapabilities(Box::new(
                    CodexRelayDiagnosticsCompletion {
                        generation: start.generation,
                        result,
                    },
                )));
            });
        }
        PendingOperatorAction::RunRelayLiveSmoke(start) => {
            tokio::spawn(async move {
                let result = proxy
                    .codex_relay_live_smoke(start.request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::RunRelayLiveSmoke(Box::new(
                    CodexRelayLiveSmokeCompletion {
                        generation: start.generation,
                        result,
                    },
                )));
            });
        }
    }
    true
}

pub(super) fn start_attached_operator_action(
    ui: &mut UiState,
    client: &LocalOperatorClient,
    tx: OperatorActionSender,
) -> bool {
    if ui.operator_action_in_flight {
        return false;
    }
    let Some(action) = ui.pending_operator_action.take() else {
        return false;
    };
    ui.operator_action_in_flight = true;
    let client = client.clone();
    match action {
        PendingOperatorAction::RefreshBalances { force } => {
            ui.last_balance_refresh_requested_at = Some(Instant::now());
            ui.balance_refresh_in_flight = true;
            tokio::spawn(async move {
                let result = client
                    .refresh_provider_balances(force)
                    .await
                    .map(|response| response.refresh)
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::RefreshBalances(result));
            });
        }
        PendingOperatorAction::MutateRouting(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = client
                    .mutate_operator_routing(&request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateRouting { command, result });
            });
        }
        PendingOperatorAction::MutateSessionAffinity(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = client
                    .mutate_operator_session_affinity(&request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateSessionAffinity { command, result });
            });
        }
        PendingOperatorAction::MutateSessionBinding(request) => {
            let command = request.command.clone();
            tokio::spawn(async move {
                let result = client
                    .mutate_operator_session_binding(&request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateSessionBinding { command, result });
            });
        }
        PendingOperatorAction::ReloadRuntime => {
            tokio::spawn(async move {
                let result = client
                    .reload_runtime()
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::ReloadRuntime(result));
            });
        }
        PendingOperatorAction::MutateDefaultProfile(request) => {
            let scope = request.scope;
            tokio::spawn(async move {
                let result = client
                    .mutate_operator_default_profile(&request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::MutateDefaultProfile { scope, result });
            });
        }
        PendingOperatorAction::InspectRelayCapabilities(start) => {
            tokio::spawn(async move {
                let result = client
                    .inspect_relay_capabilities(&start.request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::InspectRelayCapabilities(Box::new(
                    CodexRelayDiagnosticsCompletion {
                        generation: start.generation,
                        result,
                    },
                )));
            });
        }
        PendingOperatorAction::RunRelayLiveSmoke(start) => {
            tokio::spawn(async move {
                let result = client
                    .run_relay_live_smoke(&start.request)
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(OperatorActionOutcome::RunRelayLiveSmoke(Box::new(
                    CodexRelayLiveSmokeCompletion {
                        generation: start.generation,
                        result,
                    },
                )));
            });
        }
    }
    true
}

pub(super) fn apply_operator_action_outcome(ui: &mut UiState, outcome: OperatorActionOutcome) {
    ui.operator_action_in_flight = false;
    ui.needs_snapshot_refresh = true;
    let message = match outcome {
        OperatorActionOutcome::RefreshBalances(Ok(summary)) => {
            ui.balance_refresh_in_flight = false;
            ui.last_balance_refresh_finished_at = Some(Instant::now());
            ui.last_balance_refresh_error = None;
            let message = balance_refresh_message(ui.language, &summary);
            ui.last_balance_refresh_summary = Some(summary);
            ui.last_balance_refresh_message = Some(message.clone());
            message
        }
        OperatorActionOutcome::RefreshBalances(Err(error)) => {
            ui.balance_refresh_in_flight = false;
            ui.last_balance_refresh_finished_at = Some(Instant::now());
            ui.last_balance_refresh_summary = None;
            ui.last_balance_refresh_message = None;
            ui.last_balance_refresh_error = Some(error.clone());
            match ui.language {
                Language::Zh => format!("余额/额度刷新失败：{error}"),
                Language::En => format!("balance/quota refresh failed: {error}"),
            }
        }
        OperatorActionOutcome::MutateRouting { command, result } => {
            let refresh_balances = matches!(
                &result,
                Ok(response) if response.status == OperatorRoutingMutationStatus::Applied
            );
            let message = routing_mutation_message(ui.language, &command, result);
            if refresh_balances {
                // A route change can make a previously fresh balance misleading.
                let _ = queue_balance_refresh_with_cooldown(ui, true, false, false);
            }
            message
        }
        OperatorActionOutcome::MutateSessionAffinity { command, result } => {
            session_affinity_mutation_message(ui.language, &command, result)
        }
        OperatorActionOutcome::MutateSessionBinding { command, result } => {
            session_binding_mutation_message(ui.language, &command, result)
        }
        OperatorActionOutcome::ReloadRuntime(result) => runtime_reload_message(ui.language, result),
        OperatorActionOutcome::MutateDefaultProfile { scope, result } => {
            default_profile_mutation_message(ui.language, scope, result)
        }
        OperatorActionOutcome::InspectRelayCapabilities(completion) => {
            ui.codex_relay_diagnostics
                .apply_completion(*completion, Instant::now());
            relay_diagnostics_message(ui)
        }
        OperatorActionOutcome::RunRelayLiveSmoke(completion) => {
            ui.codex_relay_live_smoke
                .apply_completion(*completion, Instant::now());
            relay_live_smoke_message(ui)
        }
    };
    ui.toast = Some((message, Instant::now()));
    queue_deferred_auto_balance_refresh(ui);
}

fn queue_deferred_auto_balance_refresh(ui: &mut UiState) {
    if ui.pending_operator_action.is_some() || !ui.deferred_auto_balance_refresh {
        return;
    }
    let _ = queue_balance_refresh_with_cooldown(ui, false, false, false);
}

pub(in crate::tui) fn balance_refresh_summary_has_warning(
    summary: &UsageProviderRefreshSummary,
) -> bool {
    summary.providers_rejected > 0
        || summary.failed > 0
        || summary.missing_token > 0
        || summary.auto_failed > 0
}

fn balance_refresh_message(lang: Language, summary: &UsageProviderRefreshSummary) -> String {
    if summary.deduplicated > 0
        && summary.attempted == 0
        && !balance_refresh_summary_has_warning(summary)
    {
        return match lang {
            Language::Zh => "余额/额度刷新已由 daemon 合并处理".to_string(),
            Language::En => "balance/quota refresh was deduplicated by the daemon".to_string(),
        };
    }
    let mut message = match lang {
        Language::Zh => {
            let rejected = if summary.providers_rejected > 0 {
                format!("，跳过无效配置 {}", summary.providers_rejected)
            } else {
                String::new()
            };
            format!(
                "余额/额度：成功 {}/{}，失败 {}，缺少凭据 {}{rejected}",
                summary.refreshed, summary.attempted, summary.failed, summary.missing_token
            )
        }
        Language::En => {
            let rejected = if summary.providers_rejected > 0 {
                format!(
                    ", {} invalid configurations skipped",
                    summary.providers_rejected
                )
            } else {
                String::new()
            };
            format!(
                "balance/quota: {}/{} refreshed, {} failed, {} missing credentials{rejected}",
                summary.refreshed, summary.attempted, summary.failed, summary.missing_token
            )
        }
    };
    if let Some(diagnostic) = summary.rejected_providers.first() {
        match lang {
            Language::Zh => message.push_str(&format!(
                "；{} [{}]：{}",
                diagnostic.provider_id, diagnostic.code, diagnostic.message
            )),
            Language::En => message.push_str(&format!(
                "; {} [{}]: {}",
                diagnostic.provider_id, diagnostic.code, diagnostic.message
            )),
        }
        let remaining = summary.providers_rejected.saturating_sub(1);
        if remaining > 0 {
            match lang {
                Language::Zh => message.push_str(&format!("；另有 {remaining} 项")),
                Language::En => message.push_str(&format!("; {remaining} more")),
            }
        }
    }
    message
}

fn read_only_action_message(ui: &UiState) -> String {
    match (ui.language, ui.runtime_connection.is_remote_observer()) {
        (Language::Zh, true) => "远程观察模式为只读；请在 daemon 所在机器运行 TUI".to_string(),
        (Language::En, true) => {
            "remote observer mode is read-only; run the TUI on the daemon host".to_string()
        }
        (Language::Zh, false) => {
            "当前 daemon 未声明本机 operator action 能力；已降级为只读".to_string()
        }
        (Language::En, false) => {
            "the daemon did not advertise local operator actions; falling back to read-only"
                .to_string()
        }
    }
}

fn runtime_reload_message(
    language: Language,
    result: Result<crate::proxy::OperatorRuntimeReloadResponse, String>,
) -> String {
    match result {
        Ok(response) if response.changed => match language {
            Language::Zh => format!("运行时配置已重载（revision={}）", response.runtime_revision),
            Language::En => format!(
                "runtime config reloaded (revision={})",
                response.runtime_revision
            ),
        },
        Ok(_) => match language {
            Language::Zh => "运行时配置已是磁盘上的最新版本".to_string(),
            Language::En => "runtime config already matches disk".to_string(),
        },
        Err(error) => match language {
            Language::Zh => format!("运行时配置重载失败；继续使用 last-known-good：{error}"),
            Language::En => format!("runtime reload failed; keeping last-known-good: {error}"),
        },
    }
}

fn default_profile_mutation_message(
    language: Language,
    scope: OperatorDefaultProfileScope,
    result: Result<OperatorDefaultProfileMutationResponse, String>,
) -> String {
    let scope_label = match (language, scope) {
        (Language::Zh, OperatorDefaultProfileScope::Configured) => "配置默认 profile",
        (Language::Zh, OperatorDefaultProfileScope::Runtime) => "运行时默认 profile",
        (Language::En, OperatorDefaultProfileScope::Configured) => "configured default profile",
        (Language::En, OperatorDefaultProfileScope::Runtime) => "runtime default profile",
    };
    match result {
        Ok(response) if response.status == OperatorDefaultProfileMutationStatus::Applied => {
            let profile = match scope {
                OperatorDefaultProfileScope::Configured => {
                    response.profiles.configured_default_profile.as_deref()
                }
                OperatorDefaultProfileScope::Runtime => response
                    .profiles
                    .runtime_default_profile_override
                    .as_deref(),
            }
            .unwrap_or("<none>");
            format!("{scope_label}: {profile}")
        }
        Ok(_) => match language {
            Language::Zh => format!("{scope_label} 已处于所选状态"),
            Language::En => format!("{scope_label} is already in the selected state"),
        },
        Err(error) => match language {
            Language::Zh => format!("{scope_label} 更新失败：{error}"),
            Language::En => format!("failed to update {scope_label}: {error}"),
        },
    }
}

fn relay_diagnostics_message(ui: &UiState) -> String {
    if let Some(error) = ui.codex_relay_diagnostics.last_error.as_deref() {
        return match ui.language {
            Language::Zh => format!("relay 能力诊断失败：{error}"),
            Language::En => format!("relay capability diagnostic failed: {error}"),
        };
    }
    let summary = ui
        .codex_relay_diagnostics
        .last_result
        .as_ref()
        .map(|response| {
            format!(
                "{}/{} mismatches={}",
                response.provider_id,
                response.endpoint_id,
                response.mismatches.len()
            )
        })
        .unwrap_or_else(|| "-".to_string());
    match ui.language {
        Language::Zh => format!("relay 能力诊断完成：{summary}"),
        Language::En => format!("relay capability diagnostic complete: {summary}"),
    }
}

fn relay_live_smoke_message(ui: &UiState) -> String {
    if let Some(error) = ui.codex_relay_live_smoke.last_error.as_deref() {
        return match ui.language {
            Language::Zh => format!("relay live smoke 失败：{error}"),
            Language::En => format!("relay live smoke failed: {error}"),
        };
    }
    let (passed, total) = ui.codex_relay_live_smoke.passed_counts().unwrap_or((0, 0));
    match ui.language {
        Language::Zh => format!("relay live smoke：{passed}/{total} 通过"),
        Language::En => format!("relay live smoke: {passed}/{total} passed"),
    }
}

pub(in crate::tui) fn notify_read_only_operator_action(ui: &mut UiState) {
    ui.toast = Some((read_only_action_message(ui), Instant::now()));
}

fn endpoint_mode_label(lang: Language, mode: OperatorEndpointMode) -> &'static str {
    match (lang, mode) {
        (Language::Zh, OperatorEndpointMode::Enabled) => "启用",
        (Language::Zh, OperatorEndpointMode::Draining) => "排空",
        (Language::Zh, OperatorEndpointMode::Disabled) => "禁用",
        (Language::En, OperatorEndpointMode::Enabled) => "enabled",
        (Language::En, OperatorEndpointMode::Draining) => "draining",
        (Language::En, OperatorEndpointMode::Disabled) => "disabled",
    }
}

fn routing_mutation_message(
    lang: Language,
    command: &OperatorRoutingCommand,
    result: Result<OperatorRoutingMutationResponse, String>,
) -> String {
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            return match lang {
                Language::Zh => format!("路由更新失败：{error}"),
                Language::En => format!("routing update failed: {error}"),
            };
        }
    };
    match response.status {
        OperatorRoutingMutationStatus::Conflict => match lang {
            Language::Zh => "路由状态已发生变化；已刷新，请重试".to_string(),
            Language::En => "routing state changed; the view was refreshed, retry".to_string(),
        },
        OperatorRoutingMutationStatus::Unchanged => match lang {
            Language::Zh => "路由已经处于请求的状态".to_string(),
            Language::En => "routing is already in the requested state".to_string(),
        },
        OperatorRoutingMutationStatus::Applied => match (lang, command) {
            (
                Language::Zh,
                OperatorRoutingCommand::SetNewSessionPreference {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("新会话将优先使用 {provider_id}.{endpoint_id}"),
            (
                Language::En,
                OperatorRoutingCommand::SetNewSessionPreference {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("new sessions now prefer {provider_id}.{endpoint_id}"),
            (Language::Zh, OperatorRoutingCommand::ClearNewSessionPreference) => {
                "新会话偏好已清除；恢复自动调度".to_string()
            }
            (Language::En, OperatorRoutingCommand::ClearNewSessionPreference) => {
                "new-session preference cleared; automatic scheduling restored".to_string()
            }
            (
                Language::Zh,
                OperatorRoutingCommand::SetEndpointMode {
                    provider_id,
                    endpoint_id,
                    mode,
                },
            ) => format!(
                "端点 {provider_id}.{endpoint_id} 已设为 {}",
                endpoint_mode_label(lang, *mode)
            ),
            (
                Language::En,
                OperatorRoutingCommand::SetEndpointMode {
                    provider_id,
                    endpoint_id,
                    mode,
                },
            ) => format!(
                "endpoint {provider_id}.{endpoint_id} is now {}",
                endpoint_mode_label(lang, *mode)
            ),
        },
    }
}

fn session_affinity_mutation_message(
    lang: Language,
    command: &OperatorSessionAffinityCommand,
    result: Result<OperatorSessionAffinityMutationResponse, String>,
) -> String {
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            return match lang {
                Language::Zh => format!("会话 affinity 更新失败：{error}"),
                Language::En => format!("session affinity update failed: {error}"),
            };
        }
    };
    match response.status {
        OperatorSessionAffinityMutationStatus::Conflict => match lang {
            Language::Zh => "会话或路由状态已变化；已刷新，请重试".to_string(),
            Language::En => {
                "session or routing state changed; the view was refreshed, retry".to_string()
            }
        },
        OperatorSessionAffinityMutationStatus::Busy => match lang {
            Language::Zh => "会话仍有进行中的请求；请等待空闲后重试".to_string(),
            Language::En => {
                "the session still has an active request; retry when it is idle".to_string()
            }
        },
        OperatorSessionAffinityMutationStatus::Unchanged => match lang {
            Language::Zh => "会话 affinity 已经处于请求的状态".to_string(),
            Language::En => "session affinity is already in the requested state".to_string(),
        },
        OperatorSessionAffinityMutationStatus::Applied => match (lang, command) {
            (Language::Zh, OperatorSessionAffinityCommand::Clear) => {
                "会话 affinity 已清除；有状态请求不会静默切换端点".to_string()
            }
            (Language::En, OperatorSessionAffinityCommand::Clear) => {
                "session affinity cleared; state-bound requests will not silently move".to_string()
            }
            (
                Language::Zh,
                OperatorSessionAffinityCommand::Rebind {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("会话已重新绑定到 {provider_id}.{endpoint_id}"),
            (
                Language::En,
                OperatorSessionAffinityCommand::Bind {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("session bound to {provider_id}.{endpoint_id}"),
            (
                Language::Zh,
                OperatorSessionAffinityCommand::Bind {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("会话已绑定到 {provider_id}.{endpoint_id}"),
            (
                Language::En,
                OperatorSessionAffinityCommand::Rebind {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("session rebound to {provider_id}.{endpoint_id}"),
        },
    }
}

fn session_binding_mutation_message(
    lang: Language,
    command: &OperatorSessionBindingCommand,
    result: Result<OperatorSessionBindingMutationResponse, String>,
) -> String {
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            return match lang {
                Language::Zh => format!("会话控制更新失败：{error}"),
                Language::En => format!("session control update failed: {error}"),
            };
        }
    };
    match response.status {
        OperatorSessionBindingMutationStatus::Conflict => match lang {
            Language::Zh => "会话绑定已变化；已刷新，请重试".to_string(),
            Language::En => "session binding changed; the view was refreshed, retry".to_string(),
        },
        OperatorSessionBindingMutationStatus::Unchanged => match lang {
            Language::Zh => "会话控制已经处于请求的状态".to_string(),
            Language::En => "session control is already in the requested state".to_string(),
        },
        OperatorSessionBindingMutationStatus::Applied => match (lang, command) {
            (Language::Zh, OperatorSessionBindingCommand::SetProfile { profile_name }) => {
                match profile_name.as_deref() {
                    Some(name) => format!("会话已绑定 profile {name}"),
                    None => "会话 profile 绑定已清除".to_string(),
                }
            }
            (Language::En, OperatorSessionBindingCommand::SetProfile { profile_name }) => {
                match profile_name.as_deref() {
                    Some(name) => format!("session bound to profile {name}"),
                    None => "session profile binding cleared".to_string(),
                }
            }
            (Language::Zh, OperatorSessionBindingCommand::SetModel { model }) => {
                format!("会话 model：{}", model.as_deref().unwrap_or("<已清除>"))
            }
            (Language::En, OperatorSessionBindingCommand::SetModel { model }) => {
                format!("session model: {}", model.as_deref().unwrap_or("<cleared>"))
            }
            (
                Language::Zh,
                OperatorSessionBindingCommand::SetReasoningEffort { reasoning_effort },
            ) => format!(
                "会话 reasoning effort：{}",
                reasoning_effort.as_deref().unwrap_or("<已清除>")
            ),
            (
                Language::En,
                OperatorSessionBindingCommand::SetReasoningEffort { reasoning_effort },
            ) => format!(
                "session reasoning effort: {}",
                reasoning_effort.as_deref().unwrap_or("<cleared>")
            ),
            (Language::Zh, OperatorSessionBindingCommand::SetServiceTier { service_tier }) => {
                format!(
                    "会话 service tier：{}",
                    service_tier.as_deref().unwrap_or("<已清除>")
                )
            }
            (Language::En, OperatorSessionBindingCommand::SetServiceTier { service_tier }) => {
                format!(
                    "session service tier: {}",
                    service_tier.as_deref().unwrap_or("<cleared>")
                )
            }
            (Language::Zh, OperatorSessionBindingCommand::ResetManualOverrides) => {
                "会话手动控制已重置；下一请求恢复默认策略".to_string()
            }
            (Language::En, OperatorSessionBindingCommand::ResetManualOverrides) => {
                "session manual controls reset; the next request uses defaults".to_string()
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_providers::UsageProviderConfigDiagnostic;

    fn session_affinity_request() -> OperatorSessionAffinityMutationRequest {
        OperatorSessionAffinityMutationRequest {
            session_key: "session:sha256:test".to_string(),
            expected_affinity_revision: Some("affinity:v1:test".to_string()),
            command: OperatorSessionAffinityCommand::Clear,
        }
    }

    fn session_binding_request() -> OperatorSessionBindingMutationRequest {
        OperatorSessionBindingMutationRequest {
            session_key: "session:sha256:test".to_string(),
            expected_binding_revision: "binding:v1:test".to_string(),
            command: OperatorSessionBindingCommand::SetReasoningEffort {
                reasoning_effort: Some("high".to_string()),
            },
        }
    }

    fn routing_response(status: OperatorRoutingMutationStatus) -> OperatorRoutingMutationResponse {
        OperatorRoutingMutationResponse {
            status,
            routing: crate::dashboard_core::OperatorRoutingSummary {
                route_graph_key: "routing:sha256:test".to_string(),
                control_revision: 1,
                provider_policy_revision: 1,
                entry: "main".to_string(),
                entry_strategy: crate::config::RouteStrategy::RoundRobin,
                entry_target: None,
                new_session_preference: None,
                affinity_policy: crate::config::RouteAffinityPolicy::FallbackSticky,
                scheduling_preset: crate::config::SchedulingPreset::Balanced,
                fallback_ttl_ms: None,
                reprobe_preferred_after_ms: None,
                candidates: Vec::new(),
            },
        }
    }

    #[test]
    fn auto_balance_refresh_has_a_short_ui_cooldown() {
        let mut ui = UiState::default();
        assert!(queue_balance_refresh(&mut ui, false, false));
        ui.pending_operator_action = None;
        ui.last_balance_refresh_requested_at = Some(Instant::now());
        assert!(!queue_balance_refresh(&mut ui, false, false));
    }

    #[test]
    fn forced_balance_refresh_is_deduplicated_for_two_seconds() {
        let mut ui = UiState {
            last_balance_refresh_requested_at: Some(Instant::now()),
            ..UiState::default()
        };

        assert!(!queue_balance_refresh(&mut ui, true, true));
        assert!(ui.pending_operator_action.is_none());

        ui.last_balance_refresh_requested_at =
            Some(Instant::now() - FORCED_BALANCE_REFRESH_COOLDOWN);
        assert!(queue_balance_refresh(&mut ui, true, true));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: true })
        ));
    }

    #[test]
    fn applied_routing_mutation_queues_a_forced_balance_refresh_without_ui_cooldown() {
        let mut ui = UiState {
            last_balance_refresh_requested_at: Some(Instant::now()),
            ..UiState::default()
        };

        apply_operator_action_outcome(
            &mut ui,
            OperatorActionOutcome::MutateRouting {
                command: OperatorRoutingCommand::SetNewSessionPreference {
                    provider_id: "input".to_string(),
                    endpoint_id: "default".to_string(),
                },
                result: Ok(routing_response(OperatorRoutingMutationStatus::Applied)),
            },
        );

        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: true })
        ));
    }

    #[test]
    fn unchanged_or_conflicted_routing_mutation_does_not_refresh_balances() {
        for status in [
            OperatorRoutingMutationStatus::Unchanged,
            OperatorRoutingMutationStatus::Conflict,
        ] {
            let mut ui = UiState::default();
            apply_operator_action_outcome(
                &mut ui,
                OperatorActionOutcome::MutateRouting {
                    command: OperatorRoutingCommand::ClearNewSessionPreference,
                    result: Ok(routing_response(status)),
                },
            );
            assert!(ui.pending_operator_action.is_none(), "status={status:?}");
        }
    }

    #[test]
    fn partial_balance_refresh_retains_and_surfaces_rejected_provider_diagnostic() {
        let summary = UsageProviderRefreshSummary {
            providers_configured: 2,
            providers_rejected: 1,
            rejected_providers: vec![UsageProviderConfigDiagnostic {
                provider_id: "siliconflow".to_string(),
                code: "invalid_config".to_string(),
                message: "endpoint templates are not supported".to_string(),
            }],
            providers_matched: 1,
            upstreams_matched: 1,
            attempted: 1,
            refreshed: 1,
            ..Default::default()
        };
        let mut ui = UiState {
            language: Language::Zh,
            ..UiState::default()
        };

        apply_operator_action_outcome(
            &mut ui,
            OperatorActionOutcome::RefreshBalances(Ok(summary.clone())),
        );

        assert_eq!(ui.last_balance_refresh_summary.as_ref(), Some(&summary));
        assert!(balance_refresh_summary_has_warning(&summary));
        assert!(ui.last_balance_refresh_error.is_none());
        let message = ui.last_balance_refresh_message.as_deref().unwrap();
        assert!(message.contains("成功 1/1"));
        assert!(message.contains("跳过无效配置 1"));
        assert!(message.contains("siliconflow"));
        assert!(message.contains("invalid_config"));
        assert!(message.contains("endpoint templates are not supported"));
    }

    #[test]
    fn remote_observer_cannot_queue_any_operator_action() {
        let mut ui = UiState {
            runtime_connection: crate::tui::state::RuntimeConnectionKind::RemoteObserver,
            ..UiState::default()
        };
        let routing = OperatorRoutingMutationRequest {
            expected_route_graph_key: "routing:sha256:test".to_string(),
            expected_control_revision: 1,
            expected_policy_revision: 2,
            command: OperatorRoutingCommand::ClearNewSessionPreference,
        };

        assert!(!queue_balance_refresh(&mut ui, true, true));
        assert!(!queue_routing_mutation(&mut ui, routing));
        assert!(!queue_session_affinity_mutation(
            &mut ui,
            session_affinity_request()
        ));
        assert!(!queue_session_binding_mutation(
            &mut ui,
            session_binding_request()
        ));
        assert!(ui.pending_operator_action.is_none());
        assert!(!ui.deferred_auto_balance_refresh);
    }

    #[test]
    fn routing_mutation_can_queue_behind_an_in_flight_balance_refresh() {
        let mut ui = UiState {
            operator_action_in_flight: true,
            ..UiState::default()
        };
        let request = OperatorRoutingMutationRequest {
            expected_route_graph_key: "sha256:test".to_string(),
            expected_control_revision: 1,
            expected_policy_revision: 2,
            command: OperatorRoutingCommand::SetNewSessionPreference {
                provider_id: "input".to_string(),
                endpoint_id: "primary".to_string(),
            },
        };

        assert!(queue_routing_mutation(&mut ui, request.clone()));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::MutateRouting(ref queued)) if queued == &request
        ));
    }

    #[test]
    fn auto_balance_refresh_defers_behind_an_in_flight_routing_mutation() {
        let mut ui = UiState {
            operator_action_in_flight: true,
            ..UiState::default()
        };

        assert!(queue_balance_refresh(&mut ui, false, false));
        assert!(ui.deferred_auto_balance_refresh);
        assert!(ui.pending_operator_action.is_none());
    }

    #[test]
    fn deferred_auto_balance_refresh_ignores_ui_cooldown_after_operator_action() {
        let mut ui = UiState {
            operator_action_in_flight: true,
            ..UiState::default()
        };

        assert!(queue_balance_refresh(&mut ui, false, false));
        assert!(ui.deferred_auto_balance_refresh);
        assert!(ui.pending_operator_action.is_none());
        ui.last_balance_refresh_requested_at = Some(Instant::now());

        apply_operator_action_outcome(
            &mut ui,
            OperatorActionOutcome::MutateSessionAffinity {
                command: OperatorSessionAffinityCommand::Clear,
                result: Ok(OperatorSessionAffinityMutationResponse {
                    status: OperatorSessionAffinityMutationStatus::Unchanged,
                    session_key: "session:sha256:test".to_string(),
                    route_affinity: None,
                }),
            },
        );

        assert!(!ui.deferred_auto_balance_refresh);
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { force: false })
        ));
    }

    #[test]
    fn manual_balance_refresh_does_not_replace_a_queued_affinity_mutation() {
        let request = session_affinity_request();
        let mut ui = UiState {
            pending_operator_action: Some(PendingOperatorAction::MutateSessionAffinity(
                request.clone(),
            )),
            ..UiState::default()
        };

        assert!(!queue_balance_refresh(&mut ui, true, true));
        assert!(matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::MutateSessionAffinity(ref queued)) if queued == &request
        ));
    }

    #[test]
    fn endpoint_mode_labels_are_stable_and_localized() {
        assert_eq!(
            endpoint_mode_label(Language::Zh, OperatorEndpointMode::Draining),
            "排空"
        );
        assert_eq!(
            endpoint_mode_label(Language::En, OperatorEndpointMode::Disabled),
            "disabled"
        );
    }
}

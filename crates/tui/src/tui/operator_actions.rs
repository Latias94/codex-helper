use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::control_plane_client::LocalOperatorClient;
use crate::proxy::{
    OperatorEndpointMode, OperatorRoutingCommand, OperatorRoutingMutationRequest,
    OperatorRoutingMutationResponse, OperatorRoutingMutationStatus, OperatorSessionAffinityCommand,
    OperatorSessionAffinityMutationRequest, OperatorSessionAffinityMutationResponse,
    OperatorSessionAffinityMutationStatus, ProxyService,
};
use crate::usage_providers::UsageProviderRefreshSummary;

use super::Language;
use super::state::UiState;

#[derive(Debug, Clone)]
pub(in crate::tui) enum PendingOperatorAction {
    RefreshBalances { force: bool },
    MutateRouting(OperatorRoutingMutationRequest),
    MutateSessionAffinity(OperatorSessionAffinityMutationRequest),
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
}

pub(super) type OperatorActionSender = mpsc::UnboundedSender<OperatorActionOutcome>;

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
    if enforce_cooldown
        && !force
        && ui
            .last_balance_refresh_requested_at
            .is_some_and(|last| last.elapsed() < Duration::from_secs(10))
    {
        return false;
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
            ui.last_balance_refresh_message = Some(message.clone());
            message
        }
        OperatorActionOutcome::RefreshBalances(Err(error)) => {
            ui.balance_refresh_in_flight = false;
            ui.last_balance_refresh_finished_at = Some(Instant::now());
            ui.last_balance_refresh_message = None;
            ui.last_balance_refresh_error = Some(error.clone());
            match ui.language {
                Language::Zh => format!("余额/额度刷新失败：{error}"),
                Language::En => format!("balance/quota refresh failed: {error}"),
            }
        }
        OperatorActionOutcome::MutateRouting { command, result } => {
            routing_mutation_message(ui.language, &command, result)
        }
        OperatorActionOutcome::MutateSessionAffinity { command, result } => {
            session_affinity_mutation_message(ui.language, &command, result)
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

fn balance_refresh_message(lang: Language, summary: &UsageProviderRefreshSummary) -> String {
    if summary.deduplicated > 0 && summary.attempted == 0 {
        return match lang {
            Language::Zh => "余额/额度刷新已由 daemon 合并处理".to_string(),
            Language::En => "balance/quota refresh was deduplicated by the daemon".to_string(),
        };
    }
    match lang {
        Language::Zh => format!(
            "余额/额度：成功 {}/{}，失败 {}，缺少凭据 {}",
            summary.refreshed, summary.attempted, summary.failed, summary.missing_token
        ),
        Language::En => format!(
            "balance/quota: {}/{} refreshed, {} failed, {} missing credentials",
            summary.refreshed, summary.attempted, summary.failed, summary.missing_token
        ),
    }
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
                OperatorSessionAffinityCommand::Rebind {
                    provider_id,
                    endpoint_id,
                },
            ) => format!("session rebound to {provider_id}.{endpoint_id}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_affinity_request() -> OperatorSessionAffinityMutationRequest {
        OperatorSessionAffinityMutationRequest {
            session_key: "session:sha256:test".to_string(),
            expected_affinity_revision: Some("affinity:v1:test".to_string()),
            command: OperatorSessionAffinityCommand::Clear,
        }
    }

    #[test]
    fn auto_balance_refresh_has_a_short_ui_cooldown() {
        let mut ui = UiState::default();
        assert!(queue_balance_refresh(&mut ui, false, false));
        ui.pending_operator_action = None;
        ui.last_balance_refresh_requested_at = Some(Instant::now());
        assert!(!queue_balance_refresh(&mut ui, false, false));
        assert!(queue_balance_refresh(&mut ui, true, false));
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

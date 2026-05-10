use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use super::attached_discovery::attached_management_candidates;
use super::attached_refresh::{apply_attached_refresh_result, fetch_attached_refresh};
use super::running_refresh::{apply_running_refresh_result, build_running_refresh_result};
use super::types::{ProxyBackgroundRefreshResult, ProxyBackgroundRefreshTask};
use super::{ProxyController, ProxyMode};

impl ProxyController {
    pub(super) fn refresh_background_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        self.poll_background_refresh();
        if self.background_refresh.is_some() {
            return;
        }

        match &mut self.mode {
            ProxyMode::Running(r) => {
                if let Some(last_refresh) = r.last_refresh
                    && last_refresh.elapsed() < refresh_every
                {
                    return;
                }
                r.last_refresh = Some(Instant::now());

                let state = r.state.clone();
                let service_name = r.service_name.to_string();
                let cfg = r.cfg.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                let join = rt.spawn(async move {
                    let result = build_running_refresh_result(state, service_name, cfg)
                        .await
                        .map(|result| ProxyBackgroundRefreshResult::Running(Box::new(result)));
                    let _ = tx.send(result);
                });

                self.background_refresh = Some(ProxyBackgroundRefreshTask { rx, join });
            }
            ProxyMode::Attached(att) => {
                let refresh_every = refresh_every.max(Duration::from_secs(1));
                if let Some(last_refresh) = att.last_refresh
                    && last_refresh.elapsed() < refresh_every
                {
                    return;
                }
                att.last_refresh = Some(Instant::now());

                let client = self.http_client.clone();
                let base_candidates = attached_management_candidates(att);
                let (tx, rx) = std::sync::mpsc::channel();
                let join = rt.spawn(async move {
                    let result = fetch_attached_refresh(client, base_candidates)
                        .await
                        .map(|result| ProxyBackgroundRefreshResult::Attached(Box::new(result)));
                    let _ = tx.send(result);
                });

                self.background_refresh = Some(ProxyBackgroundRefreshTask { rx, join });
            }
            _ => {}
        }
    }

    pub(super) fn clear_background_refresh(&mut self) {
        if let Some(task) = self.background_refresh.take() {
            task.join.abort();
        }
    }

    fn poll_background_refresh(&mut self) {
        let outcome = match self.background_refresh.as_ref() {
            Some(task) => match task.rx.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err(anyhow::anyhow!("background refresh task disconnected")))
                }
            },
            None => None,
        };

        let Some(outcome) = outcome else {
            return;
        };
        if let Some(task) = self.background_refresh.take() {
            task.join.abort();
        }

        match outcome {
            Ok(ProxyBackgroundRefreshResult::Running(result)) => {
                if let ProxyMode::Running(r) = &mut self.mode {
                    apply_running_refresh_result(r, *result);
                }
            }
            Ok(ProxyBackgroundRefreshResult::Attached(result)) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    apply_attached_refresh_result(att, *result);
                }
            }
            Err(err) => self.set_background_refresh_error(err),
        }
    }

    fn set_background_refresh_error(&mut self, err: anyhow::Error) {
        match &mut self.mode {
            ProxyMode::Running(r) => r.last_error = Some(err.to_string()),
            ProxyMode::Attached(att) => att.last_error = Some(err.to_string()),
            _ => {}
        }
    }
}

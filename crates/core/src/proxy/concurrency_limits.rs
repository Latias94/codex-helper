use std::collections::{BTreeSet, HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConcurrencyLimit {
    max_concurrent_requests: NonZeroU32,
    runtime_revision: u64,
}

impl ConcurrencyLimit {
    pub(super) const fn new(max_concurrent_requests: u32, runtime_revision: u64) -> Option<Self> {
        let Some(max_concurrent_requests) = NonZeroU32::new(max_concurrent_requests) else {
            return None;
        };
        Some(Self {
            max_concurrent_requests,
            runtime_revision,
        })
    }

    fn value(self) -> u32 {
        self.max_concurrent_requests.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConcurrencySnapshot {
    pub(super) active: u32,
    pub(super) pending: u32,
    pub(super) limit: u32,
    pub(super) runtime_revision: u64,
    pub(super) saturated: bool,
}

#[derive(Debug)]
struct ConcurrencyGate {
    state: Mutex<ConcurrencyGateState>,
}

#[derive(Debug, Default)]
struct ConcurrencyGateState {
    active: u32,
    observed_limit: Option<ConcurrencyLimit>,
    next_waiter_id: u64,
    waiters: VecDeque<ConcurrencyWaiter>,
}

#[derive(Debug)]
struct ConcurrencyWaiter {
    id: u64,
    session_id: Option<String>,
    permit_tx: oneshot::Sender<ConcurrencyPermit>,
}

#[derive(Debug, Default)]
pub(super) struct ConcurrencyLimiter {
    gates: Mutex<HashMap<String, Arc<ConcurrencyGate>>>,
}

#[derive(Debug)]
pub(super) struct ConcurrencyPermit {
    gate: Arc<ConcurrencyGate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConcurrencyWaitPolicy {
    max_wait: Duration,
    max_pending: u32,
}

impl ConcurrencyWaitPolicy {
    pub(super) const fn new(max_wait: Duration, max_pending: u32) -> Self {
        Self {
            max_wait,
            max_pending,
        }
    }

    fn allows_waiting(self) -> bool {
        !self.max_wait.is_zero() && self.max_pending > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConcurrencyAcquireError {
    Saturated {
        active: u32,
        limit: u32,
    },
    QueueFull {
        pending: u32,
        limit: u32,
    },
    SessionAlreadyQueued {
        session_id: String,
    },
    WaitTimedOut {
        active: u32,
        limit: u32,
        waited: Duration,
    },
}

#[derive(Debug)]
struct PendingWaiter {
    gate: Arc<ConcurrencyGate>,
    id: u64,
    pending: bool,
}

enum ConcurrencyAdmission {
    Immediate(ConcurrencyPermit),
    Queued {
        registration: PendingWaiter,
        permit_rx: oneshot::Receiver<ConcurrencyPermit>,
    },
}

impl ConcurrencyLimiter {
    pub(super) fn snapshot(&self, key: &str, limit: ConcurrencyLimit) -> ConcurrencySnapshot {
        let gate = self.gate_for_key(key);
        gate.snapshot(limit)
    }

    #[cfg(test)]
    pub(super) fn try_acquire(
        &self,
        key: String,
        limit: ConcurrencyLimit,
    ) -> Result<ConcurrencyPermit, ConcurrencyAcquireError> {
        let gate = self.gate_for_key(key.as_str());
        gate.try_acquire(limit)
    }

    pub(super) async fn acquire(
        &self,
        key: String,
        limit: ConcurrencyLimit,
        session_id: Option<String>,
        policy: ConcurrencyWaitPolicy,
    ) -> Result<ConcurrencyPermit, ConcurrencyAcquireError> {
        let gate = self.gate_for_key(key.as_str());
        let (mut registration, mut permit_rx) = match gate.admit(limit, session_id, policy)? {
            ConcurrencyAdmission::Immediate(permit) => return Ok(permit),
            ConcurrencyAdmission::Queued {
                registration,
                permit_rx,
            } => (registration, permit_rx),
        };

        let deadline = tokio::time::Instant::now() + policy.max_wait;
        match tokio::time::timeout_at(deadline, &mut permit_rx).await {
            Ok(Ok(permit)) => {
                registration.pending = false;
                Ok(permit)
            }
            Ok(Err(_)) => {
                registration.pending = false;
                let snapshot = gate.current_snapshot();
                Err(ConcurrencyAcquireError::Saturated {
                    active: snapshot.active,
                    limit: snapshot.limit,
                })
            }
            Err(_) => {
                if registration.cancel() {
                    let snapshot = gate.current_snapshot();
                    return Err(ConcurrencyAcquireError::WaitTimedOut {
                        active: snapshot.active,
                        limit: snapshot.limit,
                        waited: policy.max_wait,
                    });
                }

                registration.pending = false;
                match permit_rx.await {
                    Ok(permit) => Ok(permit),
                    Err(_) => {
                        let snapshot = gate.current_snapshot();
                        Err(ConcurrencyAcquireError::WaitTimedOut {
                            active: snapshot.active,
                            limit: snapshot.limit,
                            waited: policy.max_wait,
                        })
                    }
                }
            }
        }
    }

    pub(super) fn prune_inactive(&self, active_keys: &BTreeSet<String>) {
        let mut gates = match self.gates.lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        };
        gates.retain(|key, gate| {
            active_keys.contains(key) || Arc::strong_count(gate) > 1 || !gate.is_idle()
        });
    }

    fn gate_for_key(&self, key: &str) -> Arc<ConcurrencyGate> {
        let mut gates = match self.gates.lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        };
        if let Some(gate) = gates.get(key) {
            return gate.clone();
        }

        let gate = Arc::new(ConcurrencyGate {
            state: Mutex::new(ConcurrencyGateState::default()),
        });
        gates.insert(key.to_string(), gate.clone());
        gate
    }

    #[cfg(test)]
    fn gate_keys(&self) -> BTreeSet<String> {
        let gates = match self.gates.lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        };
        gates.keys().cloned().collect()
    }
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        self.gate.release();
    }
}

impl ConcurrencyGate {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, ConcurrencyGateState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        }
    }

    fn snapshot(self: &Arc<Self>, limit: ConcurrencyLimit) -> ConcurrencySnapshot {
        let failed_permits = {
            let mut state = self.lock_state();
            observe_limit(&mut state, limit);
            self.promote_waiters_locked(&mut state)
        };
        drop(failed_permits);
        self.current_snapshot()
    }

    fn current_snapshot(&self) -> ConcurrencySnapshot {
        let state = self.lock_state();
        let limit = state
            .observed_limit
            .expect("concurrency gate must observe a limit before use");
        ConcurrencySnapshot {
            active: state.active,
            pending: pending_count(state.waiters.len()),
            limit: limit.value(),
            runtime_revision: limit.runtime_revision,
            saturated: state.active >= limit.value(),
        }
    }

    fn is_idle(&self) -> bool {
        let state = self.lock_state();
        state.active == 0 && state.waiters.is_empty()
    }

    #[cfg(test)]
    fn try_acquire(
        self: &Arc<Self>,
        limit: ConcurrencyLimit,
    ) -> Result<ConcurrencyPermit, ConcurrencyAcquireError> {
        let (result, failed_permits) = {
            let mut state = self.lock_state();
            observe_limit(&mut state, limit);
            let failed_permits = self.promote_waiters_locked(&mut state);
            let current_limit = current_limit(&state);
            let result = if state.waiters.is_empty() && state.active < current_limit {
                state.active += 1;
                Ok(ConcurrencyPermit { gate: self.clone() })
            } else {
                Err(ConcurrencyAcquireError::Saturated {
                    active: state.active,
                    limit: current_limit,
                })
            };
            (result, failed_permits)
        };
        drop(failed_permits);
        result
    }

    fn admit(
        self: &Arc<Self>,
        limit: ConcurrencyLimit,
        session_id: Option<String>,
        policy: ConcurrencyWaitPolicy,
    ) -> Result<ConcurrencyAdmission, ConcurrencyAcquireError> {
        let (result, failed_permits) = {
            let mut state = self.lock_state();
            observe_limit(&mut state, limit);
            let failed_permits = self.promote_waiters_locked(&mut state);
            let current_limit = current_limit(&state);
            let result = if state.waiters.is_empty() && state.active < current_limit {
                state.active += 1;
                Ok(ConcurrencyAdmission::Immediate(ConcurrencyPermit {
                    gate: self.clone(),
                }))
            } else if !policy.allows_waiting() {
                Err(ConcurrencyAcquireError::Saturated {
                    active: state.active,
                    limit: current_limit,
                })
            } else if let Some(session_id) = session_id.as_deref()
                && state
                    .waiters
                    .iter()
                    .any(|waiter| waiter.session_id.as_deref() == Some(session_id))
            {
                Err(ConcurrencyAcquireError::SessionAlreadyQueued {
                    session_id: session_id.to_string(),
                })
            } else {
                let pending = pending_count(state.waiters.len());
                if pending >= policy.max_pending {
                    Err(ConcurrencyAcquireError::QueueFull {
                        pending,
                        limit: policy.max_pending,
                    })
                } else {
                    let id = state.next_waiter_id;
                    state.next_waiter_id = state.next_waiter_id.wrapping_add(1);
                    let (permit_tx, permit_rx) = oneshot::channel();
                    state.waiters.push_back(ConcurrencyWaiter {
                        id,
                        session_id,
                        permit_tx,
                    });
                    Ok(ConcurrencyAdmission::Queued {
                        registration: PendingWaiter {
                            gate: self.clone(),
                            id,
                            pending: true,
                        },
                        permit_rx,
                    })
                }
            };
            (result, failed_permits)
        };
        drop(failed_permits);
        result
    }

    fn release(self: &Arc<Self>) {
        let failed_permits = {
            let mut state = self.lock_state();
            debug_assert!(state.active > 0, "permit count must not underflow");
            state.active = state.active.saturating_sub(1);
            self.promote_waiters_locked(&mut state)
        };
        drop(failed_permits);
    }

    fn cancel_waiter(self: &Arc<Self>, id: u64) -> bool {
        let (removed, failed_permits) = {
            let mut state = self.lock_state();
            let removed = state
                .waiters
                .iter()
                .position(|waiter| waiter.id == id)
                .and_then(|index| state.waiters.remove(index))
                .is_some();
            let failed_permits = self.promote_waiters_locked(&mut state);
            (removed, failed_permits)
        };
        drop(failed_permits);
        removed
    }

    fn promote_waiters_locked(
        self: &Arc<Self>,
        state: &mut ConcurrencyGateState,
    ) -> Vec<ConcurrencyPermit> {
        let mut failed_permits = Vec::new();
        let limit = current_limit(state);
        while state.active < limit {
            let Some(waiter) = state.waiters.pop_front() else {
                break;
            };
            state.active += 1;
            let permit = ConcurrencyPermit { gate: self.clone() };
            if let Err(permit) = waiter.permit_tx.send(permit) {
                failed_permits.push(permit);
            }
        }
        failed_permits
    }
}

impl PendingWaiter {
    fn cancel(&mut self) -> bool {
        if !self.pending {
            return false;
        }
        let removed = self.gate.cancel_waiter(self.id);
        if removed {
            self.pending = false;
        }
        removed
    }
}

impl Drop for PendingWaiter {
    fn drop(&mut self) {
        if self.pending {
            self.gate.cancel_waiter(self.id);
        }
    }
}

fn pending_count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

fn observe_limit(state: &mut ConcurrencyGateState, observed: ConcurrencyLimit) {
    match state.observed_limit {
        None => state.observed_limit = Some(observed),
        Some(current) if observed.runtime_revision > current.runtime_revision => {
            state.observed_limit = Some(observed);
        }
        Some(current)
            if observed.runtime_revision == current.runtime_revision
                && observed.value() < current.value() =>
        {
            state.observed_limit = Some(observed);
        }
        Some(_) => {}
    }
}

fn current_limit(state: &ConcurrencyGateState) -> u32 {
    state
        .observed_limit
        .expect("concurrency gate must observe a limit before use")
        .value()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::time::Duration;

    fn capacity(limit: u32, revision: u64) -> ConcurrencyLimit {
        ConcurrencyLimit::new(limit, revision).expect("non-zero test capacity")
    }

    async fn wait_for_pending(
        limiter: &ConcurrencyLimiter,
        key: &str,
        limit: ConcurrencyLimit,
        expected: u32,
    ) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if limiter.snapshot(key, limit).pending == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending waiter count should converge");
    }

    #[test]
    fn limiter_releases_capacity_when_permit_drops() {
        let limiter = ConcurrencyLimiter::default();
        let permit = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("first permit");
        assert!(matches!(
            limiter.try_acquire("relay".to_string(), capacity(1, 1)),
            Err(ConcurrencyAcquireError::Saturated {
                active: 1,
                limit: 1
            })
        ));

        drop(permit);

        let snapshot = limiter.snapshot("relay", capacity(1, 1));
        assert_eq!(snapshot.active, 0);
        assert!(!snapshot.saturated);
        let _permit = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("permit after release");
    }

    #[test]
    fn limiter_keeps_active_count_when_limit_changes() {
        let limiter = ConcurrencyLimiter::default();
        let permit = limiter
            .try_acquire("relay".to_string(), capacity(5, 1))
            .expect("permit under old limit");

        let lowered = limiter.snapshot("relay", capacity(1, 2));
        assert_eq!(lowered.active, 1);
        assert_eq!(lowered.limit, 1);
        assert!(lowered.saturated);
        assert!(matches!(
            limiter.try_acquire("relay".to_string(), capacity(5, 1)),
            Err(ConcurrencyAcquireError::Saturated {
                active: 1,
                limit: 1
            })
        ));

        drop(permit);
        let _permit = limiter
            .try_acquire("relay".to_string(), capacity(1, 2))
            .expect("permit after old request exits");
    }

    #[tokio::test]
    async fn limiter_admits_bounded_waiters_in_fifo_order() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let first = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("first permit");
        let policy = ConcurrencyWaitPolicy::new(Duration::from_secs(1), 2);

        let first_waiter = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-a".to_string()),
                        policy,
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 1).await;

        let second_waiter = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-b".to_string()),
                        policy,
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 2).await;

        drop(first);
        let first_waiter_permit = tokio::time::timeout(Duration::from_secs(1), first_waiter)
            .await
            .expect("first queued waiter should be admitted")
            .expect("first waiter task")
            .expect("first waiter permit");
        assert_eq!(limiter.snapshot("relay", capacity(1, 1)).pending, 1);
        assert!(!second_waiter.is_finished());

        drop(first_waiter_permit);
        let second_waiter_permit = tokio::time::timeout(Duration::from_secs(1), second_waiter)
            .await
            .expect("second queued waiter should be admitted")
            .expect("second waiter task")
            .expect("second waiter permit");
        drop(second_waiter_permit);

        let snapshot = limiter.snapshot("relay", capacity(1, 1));
        assert_eq!(snapshot.active, 0);
        assert_eq!(snapshot.pending, 0);
    }

    #[tokio::test]
    async fn limiter_rejects_queue_overflow_and_duplicate_session_waiter() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let active = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("active permit");
        let policy = ConcurrencyWaitPolicy::new(Duration::from_secs(1), 1);

        let queued = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-a".to_string()),
                        policy,
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 1).await;

        assert!(matches!(
            limiter
                .acquire(
                    "relay".to_string(),
                    capacity(1, 1),
                    Some("session-a".to_string()),
                    ConcurrencyWaitPolicy::new(Duration::from_secs(1), 2),
                )
                .await,
            Err(ConcurrencyAcquireError::SessionAlreadyQueued { .. })
        ));
        assert!(matches!(
            limiter
                .acquire(
                    "relay".to_string(),
                    capacity(1, 1),
                    Some("session-b".to_string()),
                    policy,
                )
                .await,
            Err(ConcurrencyAcquireError::QueueFull { .. })
        ));

        queued.abort();
        let _ = queued.await;
        wait_for_pending(&limiter, "relay", capacity(1, 1), 0).await;
        drop(active);
    }

    #[tokio::test]
    async fn limiter_timeout_and_task_cancellation_remove_pending_accounting() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let active = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("active permit");

        let timeout_error = limiter
            .acquire(
                "relay".to_string(),
                capacity(1, 1),
                Some("session-timeout".to_string()),
                ConcurrencyWaitPolicy::new(Duration::from_millis(25), 2),
            )
            .await
            .expect_err("wait should time out while capacity remains occupied");
        assert!(matches!(
            timeout_error,
            ConcurrencyAcquireError::WaitTimedOut { .. }
        ));
        assert_eq!(limiter.snapshot("relay", capacity(1, 1)).pending, 0);

        let canceled = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-canceled".to_string()),
                        ConcurrencyWaitPolicy::new(Duration::from_secs(30), 2),
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 1).await;
        canceled.abort();
        let _ = canceled.await;
        wait_for_pending(&limiter, "relay", capacity(1, 1), 0).await;

        drop(active);
        assert_eq!(limiter.snapshot("relay", capacity(1, 1)).active, 0);
    }

    #[tokio::test]
    async fn limiter_reserves_released_capacity_before_waking_the_fifo_head() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let active = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("active permit");
        let queued = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-a".to_string()),
                        ConcurrencyWaitPolicy::new(Duration::from_secs(1), 1),
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 1).await;

        drop(active);

        let handoff = limiter.snapshot("relay", capacity(1, 1));
        assert_eq!(handoff.active, 1);
        assert_eq!(handoff.pending, 0);
        assert!(handoff.saturated);
        drop(queued.await.expect("queued task").expect("queued permit"));
    }

    #[tokio::test]
    async fn limiter_applies_newer_limits_to_existing_waiters() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let first = limiter
            .try_acquire("relay".to_string(), capacity(3, 1))
            .expect("first permit");
        let second = limiter
            .try_acquire("relay".to_string(), capacity(3, 1))
            .expect("second permit");
        let third = limiter
            .try_acquire("relay".to_string(), capacity(3, 1))
            .expect("third permit");
        let queued = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(3, 1),
                        Some("session-old-revision".to_string()),
                        ConcurrencyWaitPolicy::new(Duration::from_secs(1), 2),
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(3, 1), 1).await;

        let lowered = limiter.snapshot("relay", capacity(1, 2));
        assert_eq!(lowered.limit, 1);
        drop(first);
        drop(second);
        assert_eq!(limiter.snapshot("relay", capacity(3, 1)).pending, 1);
        assert!(!queued.is_finished());

        drop(third);
        let permit = queued.await.expect("queued task").expect("queued permit");
        let promoted = limiter.snapshot("relay", capacity(3, 1));
        assert_eq!(promoted.limit, 1, "stale revisions cannot raise the limit");
        assert_eq!(promoted.active, 1);
        assert_eq!(promoted.pending, 0);
        drop(permit);
    }

    #[tokio::test]
    async fn limiter_promotes_waiters_when_a_newer_revision_raises_the_limit() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let active = limiter
            .try_acquire("relay".to_string(), capacity(1, 1))
            .expect("active permit");
        let queued = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        "relay".to_string(),
                        capacity(1, 1),
                        Some("session-a".to_string()),
                        ConcurrencyWaitPolicy::new(Duration::from_secs(1), 1),
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, "relay", capacity(1, 1), 1).await;

        let raised = limiter.snapshot("relay", capacity(2, 2));
        assert_eq!(raised.limit, 2);
        assert_eq!(raised.active, 2);
        assert_eq!(raised.pending, 0);

        drop(active);
        drop(queued.await.expect("queued task").expect("queued permit"));
    }

    #[tokio::test]
    async fn limiter_prunes_only_idle_gates_outside_the_current_runtime() {
        let limiter = Arc::new(ConcurrencyLimiter::default());
        let stale = limiter
            .try_acquire("endpoint:codex/stale/default".to_string(), capacity(1, 1))
            .expect("stale permit");
        drop(stale);

        let active = limiter
            .try_acquire(
                "endpoint:codex/in-flight/default".to_string(),
                capacity(1, 1),
            )
            .expect("in-flight permit");
        let retained = limiter
            .try_acquire("group:codex/current".to_string(), capacity(1, 1))
            .expect("current runtime permit");
        drop(retained);

        let pending_key = "endpoint:codex/pending/default";
        let pending_blocker = limiter
            .try_acquire(pending_key.to_string(), capacity(1, 1))
            .expect("pending waiter blocker");
        let pending_waiter = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter
                    .acquire(
                        pending_key.to_string(),
                        capacity(1, 1),
                        Some("pending-session".to_string()),
                        ConcurrencyWaitPolicy::new(Duration::from_secs(1), 1),
                    )
                    .await
            })
        };
        wait_for_pending(&limiter, pending_key, capacity(1, 1), 1).await;

        limiter.prune_inactive(&BTreeSet::from(["group:codex/current".to_string()]));

        assert_eq!(
            limiter.gate_keys(),
            BTreeSet::from([
                "endpoint:codex/in-flight/default".to_string(),
                "endpoint:codex/pending/default".to_string(),
                "group:codex/current".to_string(),
            ])
        );

        drop(pending_blocker);
        drop(
            pending_waiter
                .await
                .expect("pending waiter task")
                .expect("pending waiter permit"),
        );
        drop(active);
        limiter.prune_inactive(&BTreeSet::new());
        assert!(limiter.gate_keys().is_empty());
    }
}

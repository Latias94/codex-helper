use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConcurrencySnapshot {
    pub(super) active: u32,
    pub(super) limit: u32,
    pub(super) saturated: bool,
}

#[derive(Debug)]
struct ConcurrencyGate {
    active: AtomicU32,
}

#[derive(Debug, Default)]
pub(super) struct ConcurrencyLimiter {
    gates: Mutex<HashMap<String, Arc<ConcurrencyGate>>>,
}

#[derive(Debug)]
pub(super) struct ConcurrencyPermit {
    #[allow(dead_code)]
    key: String,
    #[allow(dead_code)]
    limit: u32,
    gate: Arc<ConcurrencyGate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConcurrencyAcquireError {
    Saturated { active: u32, limit: u32 },
}

impl ConcurrencyLimiter {
    pub(super) fn snapshot(&self, key: &str, limit: u32) -> ConcurrencySnapshot {
        let gate = self.gate_for_key(key);
        let active = gate.active.load(Ordering::Acquire);
        ConcurrencySnapshot {
            active,
            limit,
            saturated: active >= limit,
        }
    }

    pub(super) fn try_acquire(
        &self,
        key: String,
        limit: u32,
    ) -> Result<ConcurrencyPermit, ConcurrencyAcquireError> {
        let gate = self.gate_for_key(key.as_str());
        loop {
            let active = gate.active.load(Ordering::Acquire);
            if active >= limit {
                return Err(ConcurrencyAcquireError::Saturated { active, limit });
            }
            let next = match active.checked_add(1) {
                Some(value) => value,
                None => return Err(ConcurrencyAcquireError::Saturated { active, limit }),
            };
            if gate
                .active
                .compare_exchange_weak(active, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(ConcurrencyPermit { key, limit, gate });
            }
        }
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
            active: AtomicU32::new(0),
        });
        gates.insert(key.to_string(), gate.clone());
        gate
    }
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_releases_capacity_when_permit_drops() {
        let limiter = ConcurrencyLimiter::default();
        let permit = limiter
            .try_acquire("relay".to_string(), 1)
            .expect("first permit");
        assert!(matches!(
            limiter.try_acquire("relay".to_string(), 1),
            Err(ConcurrencyAcquireError::Saturated {
                active: 1,
                limit: 1
            })
        ));

        drop(permit);

        let snapshot = limiter.snapshot("relay", 1);
        assert_eq!(snapshot.active, 0);
        assert!(!snapshot.saturated);
        let _permit = limiter
            .try_acquire("relay".to_string(), 1)
            .expect("permit after release");
    }

    #[test]
    fn limiter_keeps_active_count_when_limit_changes() {
        let limiter = ConcurrencyLimiter::default();
        let permit = limiter
            .try_acquire("relay".to_string(), 5)
            .expect("permit under old limit");

        let lowered = limiter.snapshot("relay", 1);
        assert_eq!(lowered.active, 1);
        assert_eq!(lowered.limit, 1);
        assert!(lowered.saturated);
        assert!(matches!(
            limiter.try_acquire("relay".to_string(), 1),
            Err(ConcurrencyAcquireError::Saturated {
                active: 1,
                limit: 1
            })
        ));

        drop(permit);
        let _permit = limiter
            .try_acquire("relay".to_string(), 1)
            .expect("permit after old request exits");
    }
}

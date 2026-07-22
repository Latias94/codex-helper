pub const FAILURE_THRESHOLD: u32 = 3;
pub const COOLDOWN_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RouteCapability {
    Inference,
    RemoteCompaction,
    HostedImageGeneration,
    ResponsesWebSocket,
    ModelCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RuntimeHealthDomain {
    EndpointTransport,
    Credential,
    Capability(RouteCapability),
    Capacity(RouteCapability),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RuntimeHealthHalfOpenTerminal {
    Success {
        now_ms: u64,
    },
    CountedFailure {
        domain: RuntimeHealthDomain,
        failure_threshold_cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    },
    Penalty {
        domain: RuntimeHealthDomain,
        cooldown_secs: u64,
        cooldown_backoff: CooldownBackoff,
    },
    Neutral,
}

#[derive(Debug, Clone, Copy)]
pub struct CooldownBackoff {
    pub factor: u64,
    pub max_secs: u64,
}

impl CooldownBackoff {
    pub(crate) fn effective_cooldown_secs(&self, base_secs: u64, penalty_streak: u32) -> u64 {
        if base_secs == 0 {
            return 0;
        }
        if self.factor <= 1 {
            return base_secs;
        }
        let cap = if self.max_secs == 0 {
            base_secs
        } else {
            self.max_secs.max(base_secs)
        };

        let mut secs = base_secs;
        for _ in 0..penalty_streak.min(64) {
            secs = secs.saturating_mul(self.factor);
            if secs >= cap {
                return cap;
            }
        }
        secs.min(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::CooldownBackoff;

    #[test]
    fn cooldown_backoff_is_capped() {
        let backoff = CooldownBackoff {
            factor: 2,
            max_secs: 120,
        };

        assert_eq!(backoff.effective_cooldown_secs(30, 0), 30);
        assert_eq!(backoff.effective_cooldown_secs(30, 1), 60);
        assert_eq!(backoff.effective_cooldown_secs(30, 2), 120);
        assert_eq!(backoff.effective_cooldown_secs(30, 8), 120);
    }

    #[test]
    fn disabled_backoff_preserves_base_cooldown() {
        let backoff = CooldownBackoff {
            factor: 1,
            max_secs: 0,
        };

        assert_eq!(backoff.effective_cooldown_secs(30, 8), 30);
        assert_eq!(backoff.effective_cooldown_secs(0, 8), 0);
    }
}

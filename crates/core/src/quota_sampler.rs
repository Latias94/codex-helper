use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use rand::RngExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub(crate) const MIN_QUOTA_SAMPLE_INTERVAL: Duration = Duration::from_secs(2 * 60);
pub(crate) const DEFAULT_QUOTA_SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);

const MAX_FAILURE_BACKOFF_EXPONENT: u32 = 4;

type RefreshFuture = Pin<Box<dyn Future<Output = QuotaSamplerRefreshOutcome> + Send + 'static>>;
type RefreshExecutor = dyn Fn() -> RefreshFuture + Send + Sync + 'static;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuotaSamplerRefreshOutcome {
    Refreshed,
    Failed(String),
    Suppressed { wake_at: tokio::time::Instant },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuotaSamplerConfig {
    interval: Duration,
    jitter_percent: u8,
}

impl Default for QuotaSamplerConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_QUOTA_SAMPLE_INTERVAL,
            jitter_percent: 10,
        }
    }
}

impl QuotaSamplerConfig {
    fn normalized(self) -> Self {
        Self {
            interval: self.interval.max(MIN_QUOTA_SAMPLE_INTERVAL),
            jitter_percent: self.jitter_percent.min(50),
        }
    }

    fn next_delay(self, consecutive_failures: u32) -> Duration {
        self.next_jittered_delay()
            .saturating_mul(failure_backoff_multiplier(consecutive_failures))
    }

    fn next_jittered_delay(self) -> Duration {
        let normalized = self.normalized();
        if normalized.jitter_percent == 0 {
            return normalized.interval;
        }

        let interval_ms = u64::try_from(normalized.interval.as_millis()).unwrap_or(u64::MAX);
        let jitter_ms = interval_ms
            .saturating_mul(u64::from(normalized.jitter_percent))
            .saturating_div(100);
        let offset = rand::rng().random_range(0..=jitter_ms);
        normalized.delay_for_jitter_offset(offset)
    }

    fn delay_for_jitter_offset(self, offset_ms: u64) -> Duration {
        let normalized = self.normalized();
        let interval_ms = u64::try_from(normalized.interval.as_millis()).unwrap_or(u64::MAX);
        let jitter_ms = interval_ms
            .saturating_mul(u64::from(normalized.jitter_percent))
            .saturating_div(100);
        Duration::from_millis(interval_ms.saturating_add(offset_ms.min(jitter_ms)))
    }
}

fn failure_backoff_multiplier(consecutive_failures: u32) -> u32 {
    1_u32
        << consecutive_failures
            .saturating_sub(1)
            .min(MAX_FAILURE_BACKOFF_EXPONENT)
}

pub(crate) struct QuotaSampler {
    config: QuotaSamplerConfig,
    refresh: Box<RefreshExecutor>,
}

impl QuotaSampler {
    #[cfg(test)]
    pub(crate) fn new<F, Fut>(config: QuotaSamplerConfig, refresh: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self::new_with_outcome(config, move || {
            let refresh = refresh();
            async move {
                match refresh.await {
                    Ok(()) => QuotaSamplerRefreshOutcome::Refreshed,
                    Err(error) => QuotaSamplerRefreshOutcome::Failed(error),
                }
            }
        })
    }

    pub(crate) fn new_with_outcome<F, Fut>(config: QuotaSamplerConfig, refresh: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = QuotaSamplerRefreshOutcome> + Send + 'static,
    {
        Self {
            config: config.normalized(),
            refresh: Box::new(move || Box::pin(refresh())),
        }
    }

    pub(crate) fn spawn(self, shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(self.run(shutdown_rx))
    }

    async fn run(self, mut shutdown_rx: watch::Receiver<bool>) {
        let mut consecutive_failures = 0_u32;

        loop {
            if *shutdown_rx.borrow() {
                return;
            }

            let refresh = (self.refresh)();
            let result = tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => return,
                result = refresh => result,
            };

            let delay = match result {
                QuotaSamplerRefreshOutcome::Refreshed => {
                    consecutive_failures = 0;
                    self.config.next_delay(consecutive_failures)
                }
                QuotaSamplerRefreshOutcome::Failed(error) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::warn!(
                        error = %error,
                        consecutive_failures,
                        "quota sampler refresh failed"
                    );
                    self.config.next_delay(consecutive_failures)
                }
                QuotaSamplerRefreshOutcome::Suppressed { wake_at } => {
                    consecutive_failures = 0;
                    self.config
                        .next_jittered_delay()
                        .min(wake_at.saturating_duration_since(tokio::time::Instant::now()))
                }
            };
            tokio::select! {
                biased;
                _ = wait_for_shutdown(&mut shutdown_rx) => return,
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }
}

async fn wait_for_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    loop {
        if *shutdown_rx.borrow() || shutdown_rx.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn config(interval: Duration) -> QuotaSamplerConfig {
        QuotaSamplerConfig {
            interval,
            jitter_percent: 0,
        }
    }

    async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
        for _ in 0..32 {
            if calls.load(Ordering::SeqCst) == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), expected);
    }

    #[test]
    fn config_enforces_floor_and_bounds_jitter() {
        let normalized = QuotaSamplerConfig {
            interval: Duration::from_secs(1),
            jitter_percent: 90,
        }
        .normalized();

        assert_eq!(normalized.interval, Duration::from_secs(120));
        assert_eq!(normalized.jitter_percent, 50);
    }

    #[test]
    fn jitter_never_runs_before_the_configured_interval() {
        let config = QuotaSamplerConfig {
            interval: Duration::from_secs(10 * 60),
            jitter_percent: 10,
        };

        assert_eq!(
            config.delay_for_jitter_offset(0),
            Duration::from_secs(10 * 60)
        );
        assert_eq!(
            config.delay_for_jitter_offset(u64::MAX),
            Duration::from_secs(11 * 60)
        );
    }

    #[test]
    fn consecutive_failure_backoff_is_exponential_and_bounded() {
        let config = config(Duration::from_secs(5 * 60));

        assert_eq!(config.next_delay(0), Duration::from_secs(5 * 60));
        assert_eq!(config.next_delay(1), Duration::from_secs(5 * 60));
        assert_eq!(config.next_delay(2), Duration::from_secs(10 * 60));
        assert_eq!(config.next_delay(3), Duration::from_secs(20 * 60));
        assert_eq!(config.next_delay(4), Duration::from_secs(40 * 60));
        assert_eq!(config.next_delay(5), Duration::from_secs(80 * 60));
        assert_eq!(config.next_delay(100), Duration::from_secs(80 * 60));
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_refreshes_while_idle_at_the_default_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(config(DEFAULT_QUOTA_SAMPLE_INTERVAL), move || {
            let calls = calls_for_refresh.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(DEFAULT_QUOTA_SAMPLE_INTERVAL - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 2).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_enforces_the_hard_floor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(config(Duration::from_secs(1)), move || {
            let calls = calls_for_refresh.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(Duration::from_secs(119)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 2).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_honors_a_slower_configured_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(config(Duration::from_secs(15 * 60)), move || {
            let calls = calls_for_refresh.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(DEFAULT_QUOTA_SAMPLE_INTERVAL).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(10 * 60)).await;
        wait_for_calls(&calls, 2).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_backs_off_after_consecutive_failures_and_resets_after_success() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(config(DEFAULT_QUOTA_SAMPLE_INTERVAL), move || {
            let calls = calls_for_refresh.clone();
            async move {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                if matches!(call, 1 | 2 | 4) {
                    Err("injected refresh failure".to_string())
                } else {
                    Ok(())
                }
            }
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        wait_for_calls(&calls, 2).await;
        tokio::time::advance(Duration::from_secs(10 * 60)).await;
        wait_for_calls(&calls, 3).await;
        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        wait_for_calls(&calls, 4).await;
        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        wait_for_calls(&calls, 5).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
    }

    #[tokio::test(start_paused = true)]
    async fn suppression_uses_earlier_wake_and_resumes_base_interval_without_backoff() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler =
            QuotaSampler::new_with_outcome(config(DEFAULT_QUOTA_SAMPLE_INTERVAL), move || {
                let calls = calls_for_refresh.clone();
                async move {
                    match calls.fetch_add(1, Ordering::SeqCst) + 1 {
                        1 => QuotaSamplerRefreshOutcome::Suppressed {
                            wake_at: tokio::time::Instant::now() + Duration::from_secs(2 * 60),
                        },
                        2 => QuotaSamplerRefreshOutcome::Suppressed {
                            wake_at: tokio::time::Instant::now() + Duration::from_secs(10 * 60),
                        },
                        _ => QuotaSamplerRefreshOutcome::Refreshed,
                    }
                }
            });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        tokio::time::advance(Duration::from_secs(2 * 60) - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 2).await;

        tokio::time::advance(DEFAULT_QUOTA_SAMPLE_INTERVAL - Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_calls(&calls, 3).await;

        tokio::time::advance(DEFAULT_QUOTA_SAMPLE_INTERVAL).await;
        wait_for_calls(&calls, 4).await;

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_an_in_flight_refresh_and_prevents_later_io() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(QuotaSamplerConfig::default(), move || {
            let calls = calls_for_refresh.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::pending::<Result<(), String>>().await
            }
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = sampler.spawn(shutdown_rx);

        wait_for_calls(&calls, 1).await;
        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("join sampler");
        tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_does_not_refresh_when_already_shutdown() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_refresh = calls.clone();
        let sampler = QuotaSampler::new(QuotaSamplerConfig::default(), move || {
            let calls = calls_for_refresh.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let (_shutdown_tx, shutdown_rx) = watch::channel(true);

        sampler.run(shutdown_rx).await;

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

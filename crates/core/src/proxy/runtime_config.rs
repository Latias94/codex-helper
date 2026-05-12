use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};
#[cfg(not(test))]
use tracing::warn;

use crate::config::{ProxyConfig, ProxyConfigV4};

pub(super) struct RuntimeConfig {
    current: AsyncRwLock<Arc<ProxyConfig>>,
    current_v4: AsyncRwLock<Option<Arc<ProxyConfigV4>>>,
    last_loaded_at_ms: AtomicU64,
    #[cfg_attr(test, allow(dead_code))]
    reload: AsyncMutex<RuntimeConfigReloadState>,
}

#[derive(Debug)]
#[cfg_attr(test, allow(dead_code))]
struct RuntimeConfigReloadState {
    last_check_at: Instant,
    last_mtime: Option<SystemTime>,
}

impl RuntimeConfig {
    pub(super) fn new_with_v4(
        initial: Arc<ProxyConfig>,
        initial_v4: Option<Arc<ProxyConfigV4>>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            current: AsyncRwLock::new(initial),
            current_v4: AsyncRwLock::new(initial_v4),
            last_loaded_at_ms: AtomicU64::new(now),
            reload: AsyncMutex::new(RuntimeConfigReloadState {
                last_check_at: Instant::now()
                    .checked_sub(Duration::from_secs(60))
                    .unwrap_or_else(Instant::now),
                last_mtime: None,
            }),
        }
    }

    pub(super) async fn snapshot(&self) -> Arc<ProxyConfig> {
        self.current.read().await.clone()
    }

    pub(super) async fn v4_snapshot(&self) -> Option<Arc<ProxyConfigV4>> {
        self.current_v4.read().await.clone()
    }

    pub(super) fn last_loaded_at_ms(&self) -> u64 {
        self.last_loaded_at_ms.load(Ordering::Relaxed)
    }

    pub(super) async fn last_mtime_ms(&self) -> Option<u64> {
        let st = self.reload.lock().await;
        st.last_mtime
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    }

    pub(super) async fn force_reload_from_disk(&self) -> anyhow::Result<bool> {
        let path = crate::config::config_file_path();
        let mtime = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());

        let loaded = crate::config::load_config_with_v4_source().await?;
        *self.current.write().await = Arc::new(loaded.runtime);
        *self.current_v4.write().await = loaded.v4.map(Arc::new);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.last_loaded_at_ms.store(now, Ordering::Relaxed);

        let mut st = self.reload.lock().await;
        let changed = st.last_mtime != mtime;
        st.last_mtime = mtime;
        Ok(changed)
    }

    #[cfg(test)]
    pub(super) async fn maybe_reload_from_disk(&self) -> bool {
        false
    }

    #[cfg(not(test))]
    pub(super) async fn maybe_reload_from_disk(&self) -> bool {
        const MIN_CHECK_INTERVAL: Duration = Duration::from_millis(800);

        let last_mtime = {
            let mut st = self.reload.lock().await;
            if st.last_check_at.elapsed() < MIN_CHECK_INTERVAL {
                return false;
            }
            st.last_check_at = Instant::now();
            st.last_mtime
        };

        let path = crate::config::config_file_path();
        let mtime = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());
        if mtime == last_mtime {
            return false;
        }

        let mut reloaded = false;
        match crate::config::load_config_with_v4_source().await {
            Ok(loaded) => {
                *self.current.write().await = Arc::new(loaded.runtime);
                *self.current_v4.write().await = loaded.v4.map(Arc::new);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                self.last_loaded_at_ms.store(now, Ordering::Relaxed);
                reloaded = true;
            }
            Err(err) => {
                warn!("failed to reload config from disk: {}", err);
            }
        }

        let mut st = self.reload.lock().await;
        st.last_mtime = mtime;
        reloaded
    }
}

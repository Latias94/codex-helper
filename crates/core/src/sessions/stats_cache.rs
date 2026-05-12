use super::*;

const SESSION_STATS_CACHE_VERSION: u32 = 1;
const MAX_STATS_CACHE_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedSessionStats {
    mtime_ms: u64,
    size: u64,
    user_turns: usize,
    assistant_turns: usize,
    last_response_at: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionStatsCacheFile {
    version: u32,
    entries: HashMap<String, CachedSessionStats>,
}

#[derive(Debug, Clone)]
pub(super) struct SessionStatsSnapshot {
    pub(super) user_turns: usize,
    pub(super) assistant_turns: usize,
    pub(super) last_response_at: Option<String>,
}

pub(super) struct SessionStatsCache {
    path: PathBuf,
    data: SessionStatsCacheFile,
    dirty: bool,
}

impl SessionStatsCache {
    pub(super) async fn load_default() -> Self {
        let path = crate::config::proxy_home_dir()
            .join("cache")
            .join("session_stats.json");
        let mut cache = Self {
            path,
            data: SessionStatsCacheFile {
                version: SESSION_STATS_CACHE_VERSION,
                entries: HashMap::new(),
            },
            dirty: false,
        };
        let bytes = match fs::read(&cache.path).await {
            Ok(b) => b,
            Err(_) => return cache,
        };
        let parsed = serde_json::from_slice::<SessionStatsCacheFile>(&bytes);
        if let Ok(mut data) = parsed {
            if data.version != SESSION_STATS_CACHE_VERSION {
                data.version = SESSION_STATS_CACHE_VERSION;
                data.entries.clear();
                cache.dirty = true;
            }
            cache.data = data;
        }
        cache
    }

    pub(super) async fn save_if_dirty(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if self.data.entries.len() > MAX_STATS_CACHE_ENTRIES {
            // Best-effort bounding: drop everything to avoid unbounded growth.
            self.data.entries.clear();
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.ok();
        }

        let bytes = serde_json::to_vec_pretty(&self.data)?;
        write_bytes_file_async(&self.path, &bytes).await?;
        self.dirty = false;
        Ok(())
    }

    pub(super) fn lookup(
        &self,
        key: &str,
        mtime_ms: u64,
        size: u64,
    ) -> Option<SessionStatsSnapshot> {
        if mtime_ms == 0 {
            return None;
        }
        let cached = self.data.entries.get(key)?;
        if cached.mtime_ms != mtime_ms || cached.size != size {
            return None;
        }
        Some(SessionStatsSnapshot {
            user_turns: cached.user_turns,
            assistant_turns: cached.assistant_turns,
            last_response_at: cached.last_response_at.clone(),
        })
    }

    pub(super) fn insert(
        &mut self,
        key: String,
        mtime_ms: u64,
        size: u64,
        stats: &SessionStatsSnapshot,
    ) {
        if mtime_ms == 0 {
            return;
        }
        self.data.entries.insert(
            key,
            CachedSessionStats {
                mtime_ms,
                size,
                user_turns: stats.user_turns,
                assistant_turns: stats.assistant_turns,
                last_response_at: stats.last_response_at.clone(),
            },
        );
        self.dirty = true;
    }
}

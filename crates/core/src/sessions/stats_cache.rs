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

    pub(super) async fn get_or_compute(
        &mut self,
        path: &Path,
    ) -> Result<(usize, usize, Option<String>)> {
        let key = path.to_string_lossy().to_string();
        let meta = fs::metadata(path)
            .await
            .with_context(|| format!("failed to stat session file {:?}", path))?;
        let size = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if mtime_ms > 0
            && let Some(cached) = self.data.entries.get(&key)
            && cached.mtime_ms == mtime_ms
            && cached.size == size
        {
            return Ok((
                cached.user_turns,
                cached.assistant_turns,
                cached.last_response_at.clone(),
            ));
        }

        let (user_turns, assistant_turns) = count_turns_in_file(path).await?;
        let last_response_at = read_last_assistant_timestamp_from_tail(path).await?;

        if mtime_ms > 0 {
            self.data.entries.insert(
                key,
                CachedSessionStats {
                    mtime_ms,
                    size,
                    user_turns,
                    assistant_turns,
                    last_response_at: last_response_at.clone(),
                },
            );
            self.dirty = true;
        }

        Ok((user_turns, assistant_turns, last_response_at))
    }
}

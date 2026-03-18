use super::*;
use std::sync::{Mutex, OnceLock};

mod basic;
mod io_bootstrap;
mod retry_profiles;
mod v2_schema;

struct ScopedEnv {
    saved: Vec<(String, Option<String>)>,
}

impl ScopedEnv {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    unsafe fn set(&mut self, key: &str, value: &Path) {
        self.saved.push((key.to_string(), std::env::var(key).ok()));
        unsafe { std::env::set_var(key, value) };
    }

    unsafe fn set_str(&mut self, key: &str, value: &str) {
        self.saved.push((key.to_string(), std::env::var(key).ok()));
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for (key, old) in self.saved.drain(..).rev() {
            unsafe {
                match old {
                    Some(v) => std::env::set_var(&key, v),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

struct TestEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    _env: ScopedEnv,
    home: PathBuf,
}

fn setup_temp_codex_home() -> TestEnv {
    let lock = env_lock();
    let mut dir = std::env::temp_dir();
    let suffix = format!("codex-helper-test-{}", uuid::Uuid::new_v4());
    dir.push(suffix);
    std::fs::create_dir_all(&dir).expect("create temp codex home");
    let mut scoped = ScopedEnv::new();
    let proxy_home = dir.join(".codex-helper");
    std::fs::create_dir_all(&proxy_home).expect("create temp proxy home");
    unsafe {
        scoped.set("CODEX_HELPER_HOME", &proxy_home);
        scoped.set("CODEX_HOME", &dir);
        // 将 HOME 也指向该目录，确保 proxy_home_dir()/config.json 也被隔离在测试目录中。
        scoped.set("HOME", &dir);
        // Windows: dirs::home_dir() prefers USERPROFILE.
        scoped.set("USERPROFILE", &dir);
        // 避免本机真实环境变量（例如 OPENAI_API_KEY）影响测试断言。
        scoped.set_str("OPENAI_API_KEY", "");
        scoped.set_str("MISTRAL_API_KEY", "");
        scoped.set_str("RIGHTCODE_API_KEY", "");
        scoped.set_str("PACKYAPI_API_KEY", "");
    }
    TestEnv {
        _lock: lock,
        _env: scoped,
        home: dir,
    }
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, content).expect("write test file");
}

mod config_doc;

pub mod codex;
pub mod config;
pub mod doctor;
pub mod pricing;
pub mod provider;
mod route_view;
pub mod routing;
pub mod session;
pub mod usage;

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    use tokio::sync::{Mutex, MutexGuard};

    pub(crate) async fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    #[derive(Default)]
    pub(crate) struct ScopedEnv {
        saved: Vec<(String, Option<OsString>)>,
    }

    impl ScopedEnv {
        pub(crate) unsafe fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
            if !self.saved.iter().any(|(saved_key, _)| saved_key == key) {
                self.saved.push((key.to_string(), std::env::var_os(key)));
            }
            unsafe {
                std::env::set_var(key, value);
            }
        }

        pub(crate) unsafe fn set_path(&mut self, key: &str, value: &Path) {
            unsafe {
                self.set(key, value.as_os_str());
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => unsafe {
                        std::env::set_var(key, value);
                    },
                    None => unsafe {
                        std::env::remove_var(key);
                    },
                }
            }
        }
    }

    pub(crate) struct TempTestDir {
        path: PathBuf,
        temp_root: PathBuf,
    }

    impl TempTestDir {
        pub(crate) fn new(prefix: &str) -> Self {
            let temp_root = std::env::temp_dir();
            let path = temp_root.join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).expect("create temporary test directory");
            Self { path, temp_root }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempTestDir {
        fn drop(&mut self) {
            let is_owned_child = self.path.parent() == Some(self.temp_root.as_path())
                && self
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("codex-helper-cli-test-"));
            if is_owned_child {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}

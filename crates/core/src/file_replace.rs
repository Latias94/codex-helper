use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

fn temp_path_for(path: &Path) -> PathBuf {
    path.with_extension("tmp.codex-helper")
}

#[cfg(windows)]
fn replace_existing_file_sync(tmp_path: &Path, path: &Path) -> Result<()> {
    if path.exists() {
        fs::copy(tmp_path, path).with_context(|| format!("copy {:?} -> {:?}", tmp_path, path))?;
        fs::remove_file(tmp_path).with_context(|| format!("remove {:?}", tmp_path))?;
    } else {
        fs::rename(tmp_path, path)
            .with_context(|| format!("rename {:?} -> {:?}", tmp_path, path))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_existing_file_sync(tmp_path: &Path, path: &Path) -> Result<()> {
    fs::rename(tmp_path, path).with_context(|| format!("rename {:?} -> {:?}", tmp_path, path))?;
    Ok(())
}

#[cfg(windows)]
async fn replace_existing_file_async(tmp_path: &Path, path: &Path) -> Result<()> {
    if path.exists() {
        tokio::fs::copy(tmp_path, path)
            .await
            .with_context(|| format!("copy {:?} -> {:?}", tmp_path, path))?;
        tokio::fs::remove_file(tmp_path)
            .await
            .with_context(|| format!("remove {:?}", tmp_path))?;
    } else {
        tokio::fs::rename(tmp_path, path)
            .await
            .with_context(|| format!("rename {:?} -> {:?}", tmp_path, path))?;
    }
    Ok(())
}

#[cfg(not(windows))]
async fn replace_existing_file_async(tmp_path: &Path, path: &Path) -> Result<()> {
    tokio::fs::rename(tmp_path, path)
        .await
        .with_context(|| format!("rename {:?} -> {:?}", tmp_path, path))?;
    Ok(())
}

pub fn write_text_file(path: &Path, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create_dir_all {:?}", parent))?;
    }

    let tmp_path = temp_path_for(path);
    {
        let mut file =
            fs::File::create(&tmp_path).with_context(|| format!("create {:?}", tmp_path))?;
        file.write_all(data.as_bytes())
            .with_context(|| format!("write {:?}", tmp_path))?;
        file.sync_all().ok();
    }

    replace_existing_file_sync(&tmp_path, path)
}

pub async fn write_bytes_file_async(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create_dir_all {:?}", parent))?;
    }

    let tmp_path = temp_path_for(path);
    tokio::fs::write(&tmp_path, data)
        .await
        .with_context(|| format!("write {:?}", tmp_path))?;

    replace_existing_file_async(&tmp_path, path).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "codex-helper-file-replace-{}",
                uuid::Uuid::new_v4()
            ))
            .join(name)
    }

    #[test]
    fn write_text_file_overwrites_existing_file() {
        let path = temp_path("state.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }

        std::fs::write(&path, "old").expect("write old file");
        write_text_file(&path, "new").expect("overwrite file");

        let text = std::fs::read_to_string(&path).expect("read new file");
        assert_eq!(text, "new");
        assert!(
            !path.with_extension("tmp.codex-helper").exists(),
            "temp file should be cleaned up"
        );
    }

    #[tokio::test]
    async fn write_bytes_file_async_overwrites_existing_file() {
        let path = temp_path("state.json");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .expect("create parent");
        }

        tokio::fs::write(&path, b"old")
            .await
            .expect("write old file");
        write_bytes_file_async(&path, b"new")
            .await
            .expect("overwrite file");

        let text = tokio::fs::read_to_string(&path)
            .await
            .expect("read new file");
        assert_eq!(text, "new");
        assert!(
            !path.with_extension("tmp.codex-helper").exists(),
            "temp file should be cleaned up"
        );
    }
}

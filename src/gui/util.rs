pub fn open_in_file_manager(path: &std::path::Path, select_file: bool) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::process::Command;
        if select_file {
            Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn()?;
        } else {
            Command::new("explorer.exe").arg(path).spawn()?;
        }
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let _ = Command::new("open").arg(path).spawn()?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use std::process::Command;
        let _ = Command::new("xdg-open").arg(path).spawn()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Ok(())
}

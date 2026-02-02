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

fn basename_lower(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_lowercase()
}

pub fn spawn_windows_terminal_wt_new_tab(
    wt_window: i32,
    workdir: &str,
    shell: &str,
    keep_open: bool,
    command: &str,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::process::Command;
        let shell_base = basename_lower(shell);
        let mut args: Vec<String> = Vec::new();
        args.push("-w".to_string());
        args.push(wt_window.to_string());
        args.push("new-tab".to_string());
        args.push("-d".to_string());
        args.push(workdir.to_string());
        args.push(shell.to_string());

        if shell_base.contains("pwsh") || shell_base.contains("powershell") {
            args.push("-ExecutionPolicy".to_string());
            args.push("Bypass".to_string());
            if keep_open {
                args.push("-NoExit".to_string());
            }
            args.push("-Command".to_string());
            args.push(command.to_string());
        } else if shell_base == "cmd" || shell_base == "cmd.exe" {
            args.push(if keep_open { "/k" } else { "/c" }.to_string());
            args.push(command.to_string());
        } else {
            anyhow::bail!(
                "unsupported shell for wt: {} (supported: pwsh/powershell/cmd)",
                shell
            );
        }

        Command::new("wt").args(&args).spawn().map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn wt; is Windows Terminal installed and `wt` in PATH? ({e})"
            )
        })?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = (wt_window, workdir, shell, keep_open, command);
        anyhow::bail!("wt is only supported on Windows");
    }
}

pub fn spawn_windows_terminal_wt_tabs_in_one_window(
    items: &[(String, String)],
    shell: &str,
    keep_open: bool,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::process::Command;

        if items.is_empty() {
            return Ok(());
        }

        let shell_base = basename_lower(shell);
        let mut args: Vec<String> = Vec::new();

        for (idx, (workdir, command)) in items.iter().enumerate() {
            if idx > 0 {
                args.push(";".to_string());
            }

            args.push("new-tab".to_string());
            args.push("-d".to_string());
            args.push(workdir.to_string());
            args.push(shell.to_string());

            if shell_base.contains("pwsh") || shell_base.contains("powershell") {
                args.push("-ExecutionPolicy".to_string());
                args.push("Bypass".to_string());
                if keep_open {
                    args.push("-NoExit".to_string());
                }
                args.push("-Command".to_string());
                args.push(command.to_string());
            } else if shell_base == "cmd" || shell_base == "cmd.exe" {
                args.push(if keep_open { "/k" } else { "/c" }.to_string());
                args.push(command.to_string());
            } else {
                anyhow::bail!(
                    "unsupported shell for wt: {} (supported: pwsh/powershell/cmd)",
                    shell
                );
            }
        }

        Command::new("wt").args(&args).spawn().map_err(|e| {
            anyhow::anyhow!(
                "failed to spawn wt; is Windows Terminal installed and `wt` in PATH? ({e})"
            )
        })?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = (items, shell, keep_open);
        anyhow::bail!("wt is only supported on Windows");
    }
}

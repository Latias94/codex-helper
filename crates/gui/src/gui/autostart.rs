#[cfg(windows)]
pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "codex-helper-gui";

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(RUN_KEY)?;

    if enabled {
        let exe = std::env::current_exe()?;
        let cmd = format!("\"{}\" --autostart", exe.display());
        key.set_value(VALUE_NAME, &cmd)?;
    } else {
        let _ = key.delete_value(VALUE_NAME);
    }

    Ok(())
}

#[cfg(windows)]
pub fn is_enabled() -> anyhow::Result<bool> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "codex-helper-gui";

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey(RUN_KEY) {
        Ok(k) => k,
        Err(_) => return Ok(false),
    };

    let v: Result<String, _> = key.get_value(VALUE_NAME);
    Ok(v.is_ok())
}

#[cfg(not(windows))]
pub fn set_enabled(_enabled: bool) -> anyhow::Result<()> {
    anyhow::bail!("autostart is only supported on Windows for now")
}

#[cfg(not(windows))]
pub fn is_enabled() -> anyhow::Result<bool> {
    Ok(false)
}

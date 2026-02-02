use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::i18n::Language;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GuiConfig {
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub proxy: ProxyUiConfig,
    #[serde(default)]
    pub attach: AttachConfig,
    #[serde(default)]
    pub history: HistoryConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub tray: TrayConfig,
    #[serde(default)]
    pub autostart: AutostartConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_refresh_ms")]
    pub refresh_ms: u64,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: default_language(),
            refresh_ms: default_refresh_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyUiConfig {
    #[serde(default = "default_service")]
    pub default_service: String,
    #[serde(default = "default_port")]
    pub default_port: u16,
    #[serde(default = "default_false")]
    pub auto_attach_or_start: bool,
    #[serde(default = "default_true")]
    pub discovery_scan_fallback: bool,
}

impl Default for ProxyUiConfig {
    fn default() -> Self {
        Self {
            default_service: default_service(),
            default_port: default_port(),
            auto_attach_or_start: default_false(),
            discovery_scan_fallback: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachConfig {
    #[serde(default = "default_on_port_in_use")]
    pub on_port_in_use: String,
    #[serde(default)]
    pub remember_choice: bool,
    #[serde(default)]
    pub last_port: Option<u16>,
}

impl Default for AttachConfig {
    fn default() -> Self {
        Self {
            on_port_in_use: default_on_port_in_use(),
            remember_choice: false,
            last_port: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// How to spawn Windows Terminal for batch resume: `tabs` or `windows`.
    #[serde(default = "default_wt_batch_mode")]
    pub wt_batch_mode: String,
    /// Default shell for opening sessions in Windows Terminal (`pwsh`/`powershell`/`cmd`).
    #[serde(default = "default_history_shell")]
    pub shell: String,
    /// Keep the spawned shell open after running the resume command.
    #[serde(default = "default_true")]
    pub keep_open: bool,
    /// Resume command template. Use `{id}` placeholder for session id.
    #[serde(default = "default_history_resume_cmd")]
    pub resume_cmd: String,
    /// Workdir mode for "copy root+id" and "open in wt": `cwd` or `git_root`.
    #[serde(default = "default_history_workdir_mode")]
    pub workdir_mode: String,
    /// When grouped by project: open the most recent N sessions in that group.
    #[serde(default = "default_history_group_open_recent_n")]
    pub group_open_recent_n: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            wt_batch_mode: default_wt_batch_mode(),
            shell: default_history_shell(),
            keep_open: default_true(),
            resume_cmd: default_history_resume_cmd(),
            workdir_mode: default_history_workdir_mode(),
            group_open_recent_n: default_history_group_open_recent_n(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Saved presets for proxy routing (service/port/pinned config).
    #[serde(default)]
    pub profiles: Vec<RoutingProfile>,
    /// Selected preset name (if any).
    #[serde(default)]
    pub selected_profile: Option<String>,
    /// Apply the selected profile automatically after start/attach.
    #[serde(default = "default_true")]
    pub apply_on_connect: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            selected_profile: None,
            apply_on_connect: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingProfile {
    pub name: String,
    /// `codex` or `claude`
    #[serde(default = "default_service")]
    pub service: String,
    /// Preferred port to start/attach (optional).
    #[serde(default)]
    pub port: Option<u16>,
    /// Pinned config name for API v1 (optional). `None` means auto.
    #[serde(default)]
    pub pinned_config: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_close_behavior")]
    pub close_behavior: String,
    #[serde(default = "default_startup_behavior")]
    pub startup_behavior: String,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            close_behavior: default_close_behavior(),
            startup_behavior: default_startup_behavior(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutostartConfig {
    #[serde(default)]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_language() -> String {
    "zh".to_string()
}

fn default_refresh_ms() -> u64 {
    500
}

fn default_service() -> String {
    "codex".to_string()
}

fn default_port() -> u16 {
    3211
}

fn default_on_port_in_use() -> String {
    "ask".to_string()
}

fn default_wt_batch_mode() -> String {
    "windows".to_string()
}

fn default_history_shell() -> String {
    "pwsh".to_string()
}

fn default_history_resume_cmd() -> String {
    "codex resume {id}".to_string()
}

fn default_history_workdir_mode() -> String {
    "cwd".to_string()
}

fn default_history_group_open_recent_n() -> usize {
    5
}

fn default_close_behavior() -> String {
    "minimize_to_tray".to_string()
}

fn default_startup_behavior() -> String {
    "minimize_to_tray".to_string()
}

impl GuiConfig {
    pub fn path() -> PathBuf {
        crate::config::proxy_home_dir().join("gui.toml")
    }

    pub fn language_enum(&self) -> Language {
        let s = self.ui.language.trim().to_ascii_lowercase();
        match s.as_str() {
            "en" | "english" => Language::En,
            _ => Language::Zh,
        }
    }

    pub fn set_language_enum(&mut self, lang: Language) {
        self.ui.language = match lang {
            Language::Zh => "zh".to_string(),
            Language::En => "en".to_string(),
        };
    }

    pub fn service_kind(&self) -> crate::config::ServiceKind {
        match self
            .proxy
            .default_service
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "claude" => crate::config::ServiceKind::Claude,
            _ => crate::config::ServiceKind::Codex,
        }
    }

    pub fn set_service_kind(&mut self, kind: crate::config::ServiceKind) {
        self.proxy.default_service = match kind {
            crate::config::ServiceKind::Codex => "codex".to_string(),
            crate::config::ServiceKind::Claude => "claude".to_string(),
        };
    }

    pub fn load_or_default() -> Self {
        let path = Self::path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        let parsed = toml::from_str::<Self>(&text);
        parsed.unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(())
    }
}

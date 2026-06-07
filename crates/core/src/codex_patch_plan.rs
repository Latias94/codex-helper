use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CodexPatchMode {
    /// Keep the historical codex-helper patch behavior.
    #[default]
    Default,
    /// Keep Codex/ChatGPT account auth for app/mobile features while model traffic goes through
    /// codex-helper.
    ChatGptBridge,
    /// Use a minimal ChatGPT-looking auth facade to expose Codex hosted image generation while
    /// request credentials still come from codex-helper routing/upstream configuration.
    ImagegenBridge,
    /// Advertise the local relay as the official OpenAI Responses provider so Codex can use
    /// first-party HTTP features that helper can safely forward, starting with remote compaction
    /// v1. Request credentials still come from codex-helper routing/upstream configuration.
    #[serde(alias = "official-relay", alias = "official_relay")]
    OfficialRelayBridge,
    /// Combine official relay provider identity for remote compaction with the image generation
    /// ChatGPT auth facade. Request credentials still come from codex-helper routing/upstream
    /// configuration.
    #[serde(alias = "official-imagegen", alias = "official_imagegen")]
    OfficialImagegenBridge,
}

impl CodexPatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ChatGptBridge => "chatgpt-bridge",
            Self::ImagegenBridge => "imagegen-bridge",
            Self::OfficialRelayBridge => "official-relay-bridge",
            Self::OfficialImagegenBridge => "official-imagegen-bridge",
        }
    }

    pub fn as_preset_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ChatGptBridge => "chatgpt-bridge",
            Self::ImagegenBridge => "imagegen-bridge",
            Self::OfficialRelayBridge => "official-relay",
            Self::OfficialImagegenBridge => "official-imagegen",
        }
    }

    pub fn is_default(self) -> bool {
        matches!(self, Self::Default)
    }

    pub fn strips_codex_client_auth(self) -> bool {
        matches!(
            self,
            Self::ChatGptBridge
                | Self::ImagegenBridge
                | Self::OfficialRelayBridge
                | Self::OfficialImagegenBridge
        )
    }

    pub fn enables_official_relay_features(self) -> bool {
        matches!(
            self,
            Self::OfficialRelayBridge | Self::OfficialImagegenBridge
        )
    }

    pub fn enables_imagegen_facade(self) -> bool {
        matches!(self, Self::ImagegenBridge | Self::OfficialImagegenBridge)
    }
}

impl std::fmt::Display for CodexPatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CodexCompactionStrategy {
    /// Keep the preset-derived behavior and do not rewrite Codex's existing
    /// `remote_compaction_v2` feature flag.
    #[default]
    Auto,
    /// Force Codex to see a helper-shaped provider so the client chooses local compaction.
    Local,
    /// Force remote compaction v1 through `/responses/compact`.
    #[serde(alias = "remote_v1")]
    RemoteV1,
    /// Force remote compaction v2 through `/responses` with `compaction_trigger`.
    #[serde(alias = "remote_v2")]
    RemoteV2,
}

impl CodexCompactionStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Local => "local",
            Self::RemoteV1 => "remote-v1",
            Self::RemoteV2 => "remote-v2",
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }

    pub fn provider_identity_for_mode(self, mode: CodexPatchMode) -> CodexPatchProviderIdentity {
        match self {
            Self::Auto => {
                if mode.enables_official_relay_features() {
                    CodexPatchProviderIdentity::OfficialOpenAi
                } else {
                    CodexPatchProviderIdentity::HelperRelay
                }
            }
            Self::Local => CodexPatchProviderIdentity::HelperRelay,
            Self::RemoteV1 | Self::RemoteV2 => CodexPatchProviderIdentity::OfficialOpenAi,
        }
    }

    pub fn remote_compaction_v2_feature_patch(self) -> CodexFeatureBoolPatch {
        match self {
            Self::Auto => CodexFeatureBoolPatch::Preserve,
            Self::Local | Self::RemoteV1 => CodexFeatureBoolPatch::Set(false),
            Self::RemoteV2 => CodexFeatureBoolPatch::Set(true),
        }
    }
}

impl std::fmt::Display for CodexCompactionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct CodexSwitchOptions {
    /// Advertise `model_providers.codex_proxy.supports_websockets = true` so Codex may choose
    /// Responses WebSocket transport. This is intentionally separate from `CodexPatchMode` to
    /// avoid mode-combination explosion.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub responses_websocket: bool,
    /// Choose Codex's compaction path without adding more client presets.
    #[serde(default, skip_serializing_if = "CodexCompactionStrategy::is_auto")]
    pub compaction: CodexCompactionStrategy,
}

impl CodexSwitchOptions {
    pub fn validate_for_mode(self, mode: CodexPatchMode) -> Result<()> {
        if matches!(
            self.compaction,
            CodexCompactionStrategy::RemoteV1 | CodexCompactionStrategy::RemoteV2
        ) && !mode.enables_official_relay_features()
        {
            return Err(anyhow!(
                "remote compaction strategies require --preset official-relay or --preset official-imagegen"
            ));
        }

        let provider_identity = self.compaction.provider_identity_for_mode(mode);
        if self.responses_websocket
            && provider_identity != CodexPatchProviderIdentity::OfficialOpenAi
        {
            return Err(anyhow!(
                "Responses WebSocket transport requires remote compaction identity; use --preset official-relay or --preset official-imagegen without --compaction local"
            ));
        }
        Ok(())
    }
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexPatchProviderIdentity {
    HelperRelay,
    OfficialOpenAi,
}

impl CodexPatchProviderIdentity {
    pub fn provider_name(self) -> &'static str {
        match self {
            Self::HelperRelay => "codex-helper",
            Self::OfficialOpenAi => "OpenAI",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexTomlBoolPatch {
    Remove,
    Set(bool),
}

impl CodexTomlBoolPatch {
    pub fn value(self) -> Option<bool> {
        match self {
            Self::Remove => None,
            Self::Set(value) => Some(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexFeatureBoolPatch {
    Preserve,
    Set(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexProviderPatchPlan {
    identity: CodexPatchProviderIdentity,
    requires_openai_auth: CodexTomlBoolPatch,
    supports_websockets: CodexTomlBoolPatch,
}

impl CodexProviderPatchPlan {
    pub fn identity(self) -> CodexPatchProviderIdentity {
        self.identity
    }

    pub fn provider_name(self) -> &'static str {
        self.identity.provider_name()
    }

    pub fn requires_openai_auth(self) -> CodexTomlBoolPatch {
        self.requires_openai_auth
    }

    pub fn supports_websockets(self) -> CodexTomlBoolPatch {
        self.supports_websockets
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAuthPatchPlan {
    RestoreOriginalIfHelperPatched,
    PatchChatGptBridge,
    PatchImagegenFacade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSwitchOnEffectOrder {
    ConfigAuthState,
    StateConfigAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexPatchPlan {
    mode: CodexPatchMode,
    options: CodexSwitchOptions,
    provider: CodexProviderPatchPlan,
    remote_compaction_v2: CodexFeatureBoolPatch,
    auth: CodexAuthPatchPlan,
    effect_order: CodexSwitchOnEffectOrder,
}

impl CodexPatchPlan {
    pub fn for_switch_on(mode: CodexPatchMode, options: CodexSwitchOptions) -> Result<Self> {
        options.validate_for_mode(mode)?;

        let provider = CodexProviderPatchPlan {
            identity: options.compaction.provider_identity_for_mode(mode),
            requires_openai_auth: match mode {
                CodexPatchMode::ChatGptBridge => CodexTomlBoolPatch::Set(true),
                CodexPatchMode::Default
                | CodexPatchMode::ImagegenBridge
                | CodexPatchMode::OfficialRelayBridge
                | CodexPatchMode::OfficialImagegenBridge => CodexTomlBoolPatch::Remove,
            },
            supports_websockets: match mode {
                CodexPatchMode::Default | CodexPatchMode::ImagegenBridge => {
                    CodexTomlBoolPatch::Remove
                }
                CodexPatchMode::ChatGptBridge => CodexTomlBoolPatch::Set(false),
                CodexPatchMode::OfficialRelayBridge | CodexPatchMode::OfficialImagegenBridge => {
                    CodexTomlBoolPatch::Set(options.responses_websocket)
                }
            },
        };

        let auth = match mode {
            CodexPatchMode::Default | CodexPatchMode::OfficialRelayBridge => {
                CodexAuthPatchPlan::RestoreOriginalIfHelperPatched
            }
            CodexPatchMode::ChatGptBridge => CodexAuthPatchPlan::PatchChatGptBridge,
            CodexPatchMode::ImagegenBridge | CodexPatchMode::OfficialImagegenBridge => {
                CodexAuthPatchPlan::PatchImagegenFacade
            }
        };

        let effect_order = match auth {
            CodexAuthPatchPlan::RestoreOriginalIfHelperPatched => {
                CodexSwitchOnEffectOrder::ConfigAuthState
            }
            CodexAuthPatchPlan::PatchChatGptBridge | CodexAuthPatchPlan::PatchImagegenFacade => {
                CodexSwitchOnEffectOrder::StateConfigAuth
            }
        };

        Ok(Self {
            mode,
            options,
            provider,
            remote_compaction_v2: options.compaction.remote_compaction_v2_feature_patch(),
            auth,
            effect_order,
        })
    }

    pub fn mode(self) -> CodexPatchMode {
        self.mode
    }

    pub fn options(self) -> CodexSwitchOptions {
        self.options
    }

    pub fn provider(self) -> CodexProviderPatchPlan {
        self.provider
    }

    pub fn remote_compaction_v2_feature(self) -> CodexFeatureBoolPatch {
        self.remote_compaction_v2
    }

    pub fn auth(self) -> CodexAuthPatchPlan {
        self.auth
    }

    pub fn effect_order(self) -> CodexSwitchOnEffectOrder {
        self.effect_order
    }

    pub fn requires_bridge_runtime_ready(self) -> bool {
        self.mode.strips_codex_client_auth() && self.mode != CodexPatchMode::ChatGptBridge
    }
}

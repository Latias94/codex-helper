use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};

pub(crate) const CODEX_CLIENT_RUNTIME_PATCH_HEADER: &str = "x-codex-helper-client-patch";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexClientPreset {
    #[default]
    Default,
    #[serde(rename = "chatgpt-bridge", alias = "chatgpt_bridge")]
    ChatGptBridge,
    #[serde(alias = "imagegen_bridge")]
    ImagegenBridge,
    #[serde(
        rename = "official-relay",
        alias = "official_relay",
        alias = "official-relay-bridge",
        alias = "official_relay_bridge"
    )]
    OfficialRelay,
    #[serde(
        rename = "official-imagegen",
        alias = "official_imagegen",
        alias = "official-imagegen-bridge",
        alias = "official_imagegen_bridge"
    )]
    OfficialImagegen,
}

impl CodexClientPreset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ChatGptBridge => "chatgpt-bridge",
            Self::ImagegenBridge => "imagegen-bridge",
            Self::OfficialRelay => "official-relay",
            Self::OfficialImagegen => "official-imagegen",
        }
    }

    pub const fn uses_official_identity(self) -> bool {
        matches!(self, Self::OfficialRelay | Self::OfficialImagegen)
    }

    pub const fn exposes_image_extension(self) -> bool {
        matches!(self, Self::ImagegenBridge | Self::OfficialImagegen)
    }

    pub const fn requires_openai_auth(self) -> bool {
        matches!(self, Self::ChatGptBridge)
    }
}

impl std::fmt::Display for CodexClientPreset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexCompactionStrategy {
    #[default]
    Auto,
    Local,
    #[serde(alias = "remote_v1")]
    RemoteV1,
    #[serde(alias = "remote_v2")]
    RemoteV2,
}

impl CodexCompactionStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Local => "local",
            Self::RemoteV1 => "remote-v1",
            Self::RemoteV2 => "remote-v2",
        }
    }

    pub const fn uses_official_identity(self, preset: CodexClientPreset) -> bool {
        match self {
            Self::Auto => preset.uses_official_identity(),
            Self::Local => false,
            Self::RemoteV1 | Self::RemoteV2 => true,
        }
    }

    fn is_default(value: &Self) -> bool {
        *value == Self::default()
    }
}

impl std::fmt::Display for CodexCompactionStrategy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexHostedImageGenerationMode {
    #[default]
    Auto,
    #[serde(alias = "enable", alias = "on", alias = "true")]
    Enabled,
    #[serde(alias = "disable", alias = "off", alias = "false")]
    Disabled,
}

impl CodexHostedImageGenerationMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub const fn filters_client_image_requests(self) -> bool {
        matches!(self, Self::Disabled)
    }

    fn is_default(value: &Self) -> bool {
        *value == Self::default()
    }
}

impl std::fmt::Display for CodexHostedImageGenerationMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
pub struct CodexClientPatchConfig {
    #[serde(default)]
    pub preset: CodexClientPreset,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub responses_websocket: bool,
    #[serde(default, skip_serializing_if = "CodexCompactionStrategy::is_default")]
    pub compaction: CodexCompactionStrategy,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub translate_models: bool,
    #[serde(
        default,
        skip_serializing_if = "CodexHostedImageGenerationMode::is_default"
    )]
    pub hosted_image_generation: CodexHostedImageGenerationMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexClientRuntimePatch {
    pub(crate) translate_models: bool,
    pub(crate) hosted_image_generation: CodexHostedImageGenerationMode,
}

impl CodexClientRuntimePatch {
    pub(crate) fn encode(self) -> String {
        format!(
            "v1;models={};hosted={}",
            u8::from(self.translate_models),
            self.hosted_image_generation.as_str()
        )
    }

    pub(crate) fn decode(value: &str) -> Option<Self> {
        let mut fields = value.split(';');
        if fields.next()? != "v1" {
            return None;
        }
        let translate_models = match fields.next()? {
            "models=0" => false,
            "models=1" => true,
            _ => return None,
        };
        let hosted_image_generation = match fields.next()? {
            "hosted=auto" => CodexHostedImageGenerationMode::Auto,
            "hosted=enabled" => CodexHostedImageGenerationMode::Enabled,
            "hosted=disabled" => CodexHostedImageGenerationMode::Disabled,
            _ => return None,
        };
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            translate_models,
            hosted_image_generation,
        })
    }
}

impl From<CodexClientPatchConfig> for CodexClientRuntimePatch {
    fn from(value: CodexClientPatchConfig) -> Self {
        Self {
            translate_models: value.translate_models,
            hosted_image_generation: value.hosted_image_generation,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawCodexClientPatchConfig {
    preset: Option<CodexClientPreset>,
    mode: Option<CodexClientPreset>,
    responses_websocket: bool,
    compaction: CodexCompactionStrategy,
    translate_models: bool,
    hosted_image_generation: CodexHostedImageGenerationMode,
}

impl<'de> Deserialize<'de> for CodexClientPatchConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawCodexClientPatchConfig::deserialize(deserializer)?;
        let preset = match (raw.preset, raw.mode) {
            (Some(preset), Some(mode)) if preset != mode => {
                return Err(serde::de::Error::custom(format!(
                    "conflicting codex.client_patch preset/mode values; keep only preset = \"{}\"",
                    preset.as_str()
                )));
            }
            (Some(preset), _) | (None, Some(preset)) => preset,
            (None, None) => CodexClientPreset::Default,
        };
        Ok(Self {
            preset,
            responses_websocket: raw.responses_websocket,
            compaction: raw.compaction,
            translate_models: raw.translate_models,
            hosted_image_generation: raw.hosted_image_generation,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodexClientPatchOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<CodexClientPreset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses_websocket: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CodexCompactionStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_models: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_image_generation: Option<CodexHostedImageGenerationMode>,
}

impl CodexClientPatchOverrides {
    pub const fn is_empty(&self) -> bool {
        self.preset.is_none()
            && self.responses_websocket.is_none()
            && self.compaction.is_none()
            && self.translate_models.is_none()
            && self.hosted_image_generation.is_none()
    }
}

impl CodexClientPatchConfig {
    pub fn with_overrides(mut self, overrides: CodexClientPatchOverrides) -> Self {
        if let Some(preset) = overrides.preset {
            self.preset = preset;
            self.responses_websocket = false;
            self.compaction = CodexCompactionStrategy::Auto;
        }
        if let Some(responses_websocket) = overrides.responses_websocket {
            self.responses_websocket = responses_websocket;
        }
        if let Some(compaction) = overrides.compaction {
            self.compaction = compaction;
        }
        if let Some(translate_models) = overrides.translate_models {
            self.translate_models = translate_models;
        }
        if let Some(hosted_image_generation) = overrides.hosted_image_generation {
            self.hosted_image_generation = hosted_image_generation;
        }
        self
    }

    pub fn with_field_overrides(mut self, overrides: CodexClientPatchOverrides) -> Self {
        if let Some(preset) = overrides.preset {
            self.preset = preset;
        }
        if let Some(responses_websocket) = overrides.responses_websocket {
            self.responses_websocket = responses_websocket;
        }
        if let Some(compaction) = overrides.compaction {
            self.compaction = compaction;
        }
        if let Some(translate_models) = overrides.translate_models {
            self.translate_models = translate_models;
        }
        if let Some(hosted_image_generation) = overrides.hosted_image_generation {
            self.hosted_image_generation = hosted_image_generation;
        }
        self
    }

    pub fn validate(&self) -> Result<()> {
        if matches!(
            self.compaction,
            CodexCompactionStrategy::RemoteV1 | CodexCompactionStrategy::RemoteV2
        ) && !self.preset.uses_official_identity()
        {
            anyhow::bail!(
                "codex.client_patch.compaction={} requires preset=official-relay or preset=official-imagegen",
                self.compaction
            );
        }
        if self.responses_websocket && !self.compaction.uses_official_identity(self.preset) {
            anyhow::bail!(
                "codex.client_patch.responses_websocket requires an official provider identity; use preset=official-relay or preset=official-imagegen without compaction=local"
            );
        }
        Ok(())
    }

    pub const fn uses_official_identity(&self) -> bool {
        self.compaction.uses_official_identity(self.preset)
    }

    pub fn compile(&self) -> Result<CompiledCodexClientPatch> {
        self.validate()?;
        let provider_identity = if self.uses_official_identity() {
            CodexProviderIdentity::OfficialOpenAi
        } else {
            CodexProviderIdentity::HelperRelay
        };
        let requires_openai_auth = if self.preset.requires_openai_auth() {
            CodexTomlBoolPatch::Set(true)
        } else {
            CodexTomlBoolPatch::Remove
        };
        let supports_websockets = match self.preset {
            CodexClientPreset::Default | CodexClientPreset::ImagegenBridge => {
                CodexTomlBoolPatch::Remove
            }
            CodexClientPreset::ChatGptBridge => CodexTomlBoolPatch::Set(false),
            CodexClientPreset::OfficialRelay | CodexClientPreset::OfficialImagegen => {
                CodexTomlBoolPatch::Set(self.responses_websocket)
            }
        };
        let remote_compaction_v2 = match self.compaction {
            CodexCompactionStrategy::Auto => CodexFeatureBoolPatch::Preserve,
            CodexCompactionStrategy::Local | CodexCompactionStrategy::RemoteV1 => {
                CodexFeatureBoolPatch::Set(false)
            }
            CodexCompactionStrategy::RemoteV2 => CodexFeatureBoolPatch::Set(true),
        };
        let image_generation = match self.hosted_image_generation {
            CodexHostedImageGenerationMode::Auto => CodexFeatureBoolPatch::Preserve,
            CodexHostedImageGenerationMode::Enabled => CodexFeatureBoolPatch::Set(true),
            CodexHostedImageGenerationMode::Disabled => CodexFeatureBoolPatch::Set(false),
        };
        let actor_marker = self.preset.exposes_image_extension()
            && self.hosted_image_generation != CodexHostedImageGenerationMode::Disabled;
        let auth_facade = match self.preset {
            CodexClientPreset::ChatGptBridge => CodexAuthFacadeStrategy::ChatGpt,
            CodexClientPreset::ImagegenBridge | CodexClientPreset::OfficialImagegen => {
                CodexAuthFacadeStrategy::EmptyChatGpt
            }
            CodexClientPreset::Default | CodexClientPreset::OfficialRelay => {
                CodexAuthFacadeStrategy::Preserve
            }
        };

        Ok(CompiledCodexClientPatch {
            provider_identity,
            requires_openai_auth,
            supports_websockets,
            actor_marker,
            auth_facade,
            remote_compaction_v2,
            image_generation,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexProviderIdentity {
    HelperRelay,
    OfficialOpenAi,
}

impl CodexProviderIdentity {
    pub const fn provider_name(self) -> &'static str {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexFeatureBoolPatch {
    Preserve,
    Set(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAuthFacadeStrategy {
    Preserve,
    ChatGpt,
    EmptyChatGpt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledCodexClientPatch {
    pub provider_identity: CodexProviderIdentity,
    pub requires_openai_auth: CodexTomlBoolPatch,
    pub supports_websockets: CodexTomlBoolPatch,
    pub actor_marker: bool,
    pub auth_facade: CodexAuthFacadeStrategy,
    pub remote_compaction_v2: CodexFeatureBoolPatch,
    pub image_generation: CodexFeatureBoolPatch,
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(
        preset: CodexClientPreset,
        compaction: CodexCompactionStrategy,
    ) -> CodexClientPatchConfig {
        CodexClientPatchConfig {
            preset,
            compaction,
            ..CodexClientPatchConfig::default()
        }
    }

    #[test]
    fn compile_maps_all_presets_without_conflating_orthogonal_features() {
        let cases = [
            (
                CodexClientPreset::Default,
                CodexProviderIdentity::HelperRelay,
                CodexTomlBoolPatch::Remove,
                CodexTomlBoolPatch::Remove,
                false,
                CodexAuthFacadeStrategy::Preserve,
            ),
            (
                CodexClientPreset::ChatGptBridge,
                CodexProviderIdentity::HelperRelay,
                CodexTomlBoolPatch::Set(true),
                CodexTomlBoolPatch::Set(false),
                false,
                CodexAuthFacadeStrategy::ChatGpt,
            ),
            (
                CodexClientPreset::ImagegenBridge,
                CodexProviderIdentity::HelperRelay,
                CodexTomlBoolPatch::Remove,
                CodexTomlBoolPatch::Remove,
                true,
                CodexAuthFacadeStrategy::EmptyChatGpt,
            ),
            (
                CodexClientPreset::OfficialRelay,
                CodexProviderIdentity::OfficialOpenAi,
                CodexTomlBoolPatch::Remove,
                CodexTomlBoolPatch::Set(false),
                false,
                CodexAuthFacadeStrategy::Preserve,
            ),
            (
                CodexClientPreset::OfficialImagegen,
                CodexProviderIdentity::OfficialOpenAi,
                CodexTomlBoolPatch::Remove,
                CodexTomlBoolPatch::Set(false),
                true,
                CodexAuthFacadeStrategy::EmptyChatGpt,
            ),
        ];

        for (preset, identity, requires_auth, websockets, marker, auth_facade) in cases {
            let compiled = config(preset, CodexCompactionStrategy::Auto)
                .compile()
                .expect("compile preset");
            assert_eq!(compiled.provider_identity, identity);
            assert_eq!(compiled.requires_openai_auth, requires_auth);
            assert_eq!(compiled.supports_websockets, websockets);
            assert_eq!(compiled.actor_marker, marker);
            assert_eq!(compiled.auth_facade, auth_facade);
            assert_eq!(
                compiled.remote_compaction_v2,
                CodexFeatureBoolPatch::Preserve
            );
        }
    }

    #[test]
    fn compile_projects_each_compaction_strategy_explicitly() {
        for (strategy, identity, feature) in [
            (
                CodexCompactionStrategy::Auto,
                CodexProviderIdentity::OfficialOpenAi,
                CodexFeatureBoolPatch::Preserve,
            ),
            (
                CodexCompactionStrategy::Local,
                CodexProviderIdentity::HelperRelay,
                CodexFeatureBoolPatch::Set(false),
            ),
            (
                CodexCompactionStrategy::RemoteV1,
                CodexProviderIdentity::OfficialOpenAi,
                CodexFeatureBoolPatch::Set(false),
            ),
            (
                CodexCompactionStrategy::RemoteV2,
                CodexProviderIdentity::OfficialOpenAi,
                CodexFeatureBoolPatch::Set(true),
            ),
        ] {
            let compiled = config(CodexClientPreset::OfficialRelay, strategy)
                .compile()
                .expect("compile compaction strategy");
            assert_eq!(compiled.provider_identity, identity);
            assert_eq!(compiled.remote_compaction_v2, feature);
        }
    }

    #[test]
    fn field_overrides_do_not_reset_unspecified_global_fields() {
        let global = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            responses_websocket: true,
            compaction: CodexCompactionStrategy::RemoteV2,
            translate_models: true,
            hosted_image_generation: CodexHostedImageGenerationMode::Disabled,
        };

        let resolved = global.with_field_overrides(CodexClientPatchOverrides {
            preset: Some(CodexClientPreset::OfficialRelay),
            compaction: Some(CodexCompactionStrategy::RemoteV1),
            ..CodexClientPatchOverrides::default()
        });

        assert_eq!(resolved.preset, CodexClientPreset::OfficialRelay);
        assert!(resolved.responses_websocket);
        assert_eq!(resolved.compaction, CodexCompactionStrategy::RemoteV1);
        assert!(resolved.translate_models);
        assert_eq!(
            resolved.hosted_image_generation,
            CodexHostedImageGenerationMode::Disabled
        );
    }

    #[test]
    fn official_imagegen_disabled_keeps_preset_auth_facade_without_image_marker() {
        let compiled = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            hosted_image_generation: CodexHostedImageGenerationMode::Disabled,
            ..CodexClientPatchConfig::default()
        }
        .compile()
        .expect("compile orthogonal image policy");

        assert_eq!(
            compiled.provider_identity,
            CodexProviderIdentity::OfficialOpenAi
        );
        assert!(!compiled.actor_marker);
        assert_eq!(compiled.auth_facade, CodexAuthFacadeStrategy::EmptyChatGpt);
        assert_eq!(compiled.image_generation, CodexFeatureBoolPatch::Set(false));
    }

    #[test]
    fn explicit_hosted_image_enable_remains_orthogonal_to_client_preset() {
        for preset in [
            CodexClientPreset::Default,
            CodexClientPreset::ChatGptBridge,
            CodexClientPreset::ImagegenBridge,
            CodexClientPreset::OfficialRelay,
            CodexClientPreset::OfficialImagegen,
        ] {
            let compiled = CodexClientPatchConfig {
                preset,
                hosted_image_generation: CodexHostedImageGenerationMode::Enabled,
                ..CodexClientPatchConfig::default()
            }
            .compile()
            .expect("hosted-image feature override");

            assert_eq!(compiled.image_generation, CodexFeatureBoolPatch::Set(true));
        }
    }

    #[test]
    fn runtime_patch_marker_round_trips_all_supported_values() {
        for translate_models in [false, true] {
            for hosted_image_generation in [
                CodexHostedImageGenerationMode::Auto,
                CodexHostedImageGenerationMode::Enabled,
                CodexHostedImageGenerationMode::Disabled,
            ] {
                let patch = CodexClientRuntimePatch {
                    translate_models,
                    hosted_image_generation,
                };
                assert_eq!(
                    CodexClientRuntimePatch::decode(&patch.encode()),
                    Some(patch)
                );
            }
        }
    }

    #[test]
    fn runtime_patch_marker_rejects_partial_or_extended_values() {
        for value in [
            "",
            "v2;models=1;hosted=enabled",
            "v1;models=true;hosted=enabled",
            "v1;models=1",
            "v1;hosted=enabled;models=1",
            "v1;models=1;hosted=unknown",
            "v1;models=1;hosted=enabled;extra=1",
        ] {
            assert_eq!(CodexClientRuntimePatch::decode(value), None, "{value}");
        }
    }
}

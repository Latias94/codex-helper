use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::codex_integration::CodexPatchMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCapabilitySupport {
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexCapabilityDecision {
    pub support: CodexCapabilitySupport,
    pub reason: String,
}

impl CodexCapabilityDecision {
    pub fn supported(reason: impl Into<String>) -> Self {
        Self {
            support: CodexCapabilitySupport::Supported,
            reason: reason.into(),
        }
    }

    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            support: CodexCapabilitySupport::Unsupported,
            reason: reason.into(),
        }
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            support: CodexCapabilitySupport::Unknown,
            reason: reason.into(),
        }
    }

    pub fn is_supported(&self) -> bool {
        self.support == CodexCapabilitySupport::Supported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexProviderIdentity {
    HelperRelay,
    OfficialOpenAi,
}

impl CodexProviderIdentity {
    pub fn from_patch_mode(patch_mode: CodexPatchMode) -> Self {
        if patch_mode.enables_official_relay_features() {
            Self::OfficialOpenAi
        } else {
            Self::HelperRelay
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthShape {
    None,
    EmptyChatGptFacade,
    CompleteChatGptLogin,
}

impl CodexAuthShape {
    pub fn from_patch_mode(patch_mode: CodexPatchMode) -> Self {
        match patch_mode {
            CodexPatchMode::Default | CodexPatchMode::OfficialRelayBridge => Self::None,
            CodexPatchMode::ImagegenBridge | CodexPatchMode::OfficialImagegenBridge => {
                Self::EmptyChatGptFacade
            }
            CodexPatchMode::ChatGptBridge => Self::CompleteChatGptLogin,
        }
    }

    pub fn allows_codex_backend_tools(self) -> bool {
        matches!(self, Self::EmptyChatGptFacade | Self::CompleteChatGptLogin)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexModelCatalogShape {
    Unknown,
    CodexModels,
    OpenAiDataList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexModelSelection {
    NotRequested,
    Selected,
    Missing,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexModelCapabilityProfile {
    pub slug: String,
    pub accepts_image_input: CodexCapabilityDecision,
    pub supports_web_search: CodexCapabilityDecision,
    pub supports_apply_patch: CodexCapabilityDecision,
    pub supports_reasoning_summaries: CodexCapabilityDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexModelCatalogProfile {
    pub shape: CodexModelCatalogShape,
    pub selection: CodexModelSelection,
    pub translation_required: bool,
    pub selected_model: Option<CodexModelCapabilityProfile>,
    pub reason: String,
}

impl CodexModelCatalogProfile {
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            shape: CodexModelCatalogShape::Unknown,
            selection: CodexModelSelection::NotApplicable,
            translation_required: false,
            selected_model: None,
            reason: reason.into(),
        }
    }

    pub fn from_models_response_json(value: &Value, selected_slug: Option<&str>) -> Self {
        if value.get("models").is_some() {
            return Self::from_codex_models_response(value, selected_slug);
        }
        if value.get("data").and_then(Value::as_array).is_some() {
            return Self {
                shape: CodexModelCatalogShape::OpenAiDataList,
                selection: CodexModelSelection::NotApplicable,
                translation_required: true,
                selected_model: None,
                reason: "OpenAI-style data list requires helper translation before Codex can use model metadata".to_string(),
            };
        }
        Self::unknown("response does not look like a Codex or OpenAI models list")
    }

    fn from_codex_models_response(value: &Value, selected_slug: Option<&str>) -> Self {
        let Some(models) = value.get("models").and_then(Value::as_array) else {
            return Self::unknown("models field is present but is not an array");
        };
        let Some(selected_slug) = selected_slug else {
            return Self {
                shape: CodexModelCatalogShape::CodexModels,
                selection: CodexModelSelection::NotRequested,
                translation_required: false,
                selected_model: None,
                reason: "Codex model catalog was provided but no selected model was requested"
                    .to_string(),
            };
        };
        let selected = models.iter().find(|model| {
            model
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| slug == selected_slug)
        });
        let Some(selected) = selected else {
            return Self {
                shape: CodexModelCatalogShape::CodexModels,
                selection: CodexModelSelection::Missing,
                translation_required: false,
                selected_model: None,
                reason: format!(
                    "selected model `{selected_slug}` is missing from the Codex catalog"
                ),
            };
        };
        let selected_model = CodexModelCapabilityProfile::from_codex_model_json(selected);
        Self {
            shape: CodexModelCatalogShape::CodexModels,
            selection: CodexModelSelection::Selected,
            translation_required: false,
            selected_model: Some(selected_model),
            reason: "selected model metadata came from a Codex models catalog".to_string(),
        }
    }

    pub fn selected_image_input_support(&self) -> CodexCapabilityDecision {
        self.selected_model
            .as_ref()
            .map(|model| model.accepts_image_input.clone())
            .unwrap_or_else(|| CodexCapabilityDecision::unknown(self.reason.clone()))
    }

    pub fn selected_web_search_support(&self) -> CodexCapabilityDecision {
        self.selected_model
            .as_ref()
            .map(|model| model.supports_web_search.clone())
            .unwrap_or_else(|| CodexCapabilityDecision::unknown(self.reason.clone()))
    }

    pub fn selected_apply_patch_support(&self) -> CodexCapabilityDecision {
        self.selected_model
            .as_ref()
            .map(|model| model.supports_apply_patch.clone())
            .unwrap_or_else(|| CodexCapabilityDecision::unknown(self.reason.clone()))
    }

    pub fn selected_reasoning_summary_support(&self) -> CodexCapabilityDecision {
        self.selected_model
            .as_ref()
            .map(|model| model.supports_reasoning_summaries.clone())
            .unwrap_or_else(|| CodexCapabilityDecision::unknown(self.reason.clone()))
    }
}

impl CodexModelCapabilityProfile {
    pub fn from_codex_model_json(model: &Value) -> Self {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>")
            .to_string();
        Self {
            slug: slug.clone(),
            accepts_image_input: image_input_support(model),
            supports_web_search: web_search_support(model),
            supports_apply_patch: apply_patch_support(model),
            supports_reasoning_summaries: reasoning_summary_support(model),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexCapabilityProfile {
    pub patch_mode: CodexPatchMode,
    pub provider_identity: CodexProviderIdentity,
    pub auth_shape: CodexAuthShape,
    pub provider_supports_websockets: bool,
    pub model_catalog: CodexModelCatalogProfile,
    pub remote_compaction_v1: CodexCapabilityDecision,
    pub hosted_image_generation: CodexCapabilityDecision,
    pub responses_websocket: CodexCapabilityDecision,
    pub web_search: CodexCapabilityDecision,
    pub apply_patch: CodexCapabilityDecision,
    pub reasoning_summaries: CodexCapabilityDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexPatchModeRecommendationConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPatchModeRecommendationInput {
    pub current_patch_mode: CodexPatchMode,
    pub model_catalog: CodexModelCatalogProfile,
    pub responses: CodexCapabilitySupport,
    pub responses_compact: CodexCapabilitySupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPatchModeRecommendation {
    pub current_patch_mode: CodexPatchMode,
    pub recommended_patch_mode: CodexPatchMode,
    pub changes_current_mode: bool,
    pub confidence: CodexPatchModeRecommendationConfidence,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

impl CodexPatchModeRecommendation {
    pub fn for_input(input: CodexPatchModeRecommendationInput) -> Self {
        let mut reasons = Vec::new();
        let mut warnings = Vec::new();

        match input.responses {
            CodexCapabilitySupport::Supported => {
                reasons.push("/responses endpoint is available".to_string());
            }
            CodexCapabilitySupport::Unsupported => {
                reasons.push("/responses endpoint is not available".to_string());
                warnings.push(
                    "no Codex patch mode can compensate for a relay that does not expose /responses"
                        .to_string(),
                );
                return Self::new(
                    input.current_patch_mode,
                    CodexPatchMode::Default,
                    CodexPatchModeRecommendationConfidence::High,
                    reasons,
                    warnings,
                );
            }
            CodexCapabilitySupport::Unknown => {
                reasons.push("/responses endpoint support is unknown".to_string());
                warnings.push(
                    "do not upgrade patch mode until ordinary Codex model requests are proven"
                        .to_string(),
                );
                return Self::new(
                    input.current_patch_mode,
                    CodexPatchMode::Default,
                    CodexPatchModeRecommendationConfidence::Low,
                    reasons,
                    warnings,
                );
            }
        }

        let image_support = input.model_catalog.selected_image_input_support();
        let image_capable = image_support.support == CodexCapabilitySupport::Supported;
        match image_support.support {
            CodexCapabilitySupport::Supported => {
                reasons.push(
                    "selected model metadata allows hosted image generation gates".to_string(),
                );
                warnings.push(
                    "hosted image generation is not actively probed because that can create artifacts or spend quota"
                        .to_string(),
                );
            }
            CodexCapabilitySupport::Unsupported => {
                reasons.push(
                    "selected model metadata does not allow hosted image generation".to_string(),
                );
            }
            CodexCapabilitySupport::Unknown => {
                warnings.push(format!(
                    "hosted image generation remains uncertain: {}",
                    image_support.reason
                ));
            }
        }

        let (recommended_patch_mode, confidence) = match input.responses_compact {
            CodexCapabilitySupport::Supported if image_capable => {
                reasons.push("/responses/compact is available".to_string());
                reasons.push(
                    "combine official relay identity with the image-generation auth facade"
                        .to_string(),
                );
                (
                    CodexPatchMode::OfficialImagegenBridge,
                    CodexPatchModeRecommendationConfidence::Medium,
                )
            }
            CodexCapabilitySupport::Supported => {
                reasons.push("/responses/compact is available".to_string());
                reasons.push(
                    "use official relay identity but avoid exposing hosted image generation"
                        .to_string(),
                );
                (
                    CodexPatchMode::OfficialRelayBridge,
                    confidence_without_image_uncertainty(image_support.support),
                )
            }
            CodexCapabilitySupport::Unsupported if image_capable => {
                reasons.push("/responses/compact is unavailable".to_string());
                reasons.push(
                    "keep the image-generation auth facade and local compaction fallback"
                        .to_string(),
                );
                (
                    CodexPatchMode::ImagegenBridge,
                    CodexPatchModeRecommendationConfidence::Medium,
                )
            }
            CodexCapabilitySupport::Unsupported => {
                reasons.push("/responses/compact is unavailable".to_string());
                reasons.push(
                    "avoid official relay identity so Codex keeps local compaction fallback"
                        .to_string(),
                );
                (
                    CodexPatchMode::Default,
                    confidence_without_image_uncertainty(image_support.support),
                )
            }
            CodexCapabilitySupport::Unknown if image_capable => {
                reasons.push("/responses/compact support is unknown".to_string());
                warnings.push(
                    "avoid official relay identity until remote compaction is actively proven"
                        .to_string(),
                );
                (
                    CodexPatchMode::ImagegenBridge,
                    CodexPatchModeRecommendationConfidence::Low,
                )
            }
            CodexCapabilitySupport::Unknown => {
                reasons.push("/responses/compact support is unknown".to_string());
                warnings.push(
                    "avoid official relay identity until remote compaction is actively proven"
                        .to_string(),
                );
                (
                    CodexPatchMode::Default,
                    CodexPatchModeRecommendationConfidence::Low,
                )
            }
        };

        Self::new(
            input.current_patch_mode,
            recommended_patch_mode,
            confidence,
            reasons,
            warnings,
        )
    }

    fn new(
        current_patch_mode: CodexPatchMode,
        recommended_patch_mode: CodexPatchMode,
        confidence: CodexPatchModeRecommendationConfidence,
        reasons: Vec<String>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            current_patch_mode,
            recommended_patch_mode,
            changes_current_mode: current_patch_mode != recommended_patch_mode,
            confidence,
            reasons,
            warnings,
        }
    }
}

fn confidence_without_image_uncertainty(
    image_support: CodexCapabilitySupport,
) -> CodexPatchModeRecommendationConfidence {
    match image_support {
        CodexCapabilitySupport::Unknown => CodexPatchModeRecommendationConfidence::Medium,
        CodexCapabilitySupport::Supported | CodexCapabilitySupport::Unsupported => {
            CodexPatchModeRecommendationConfidence::High
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCapabilityProfileInput {
    pub patch_mode: CodexPatchMode,
    pub provider_identity: CodexProviderIdentity,
    pub auth_shape: CodexAuthShape,
    pub provider_supports_websockets: bool,
    pub model_catalog: CodexModelCatalogProfile,
}

impl CodexCapabilityProfileInput {
    pub fn from_patch_mode(
        patch_mode: CodexPatchMode,
        model_catalog: CodexModelCatalogProfile,
    ) -> Self {
        Self::from_patch_mode_with_transport(patch_mode, false, model_catalog)
    }

    pub fn from_patch_mode_with_transport(
        patch_mode: CodexPatchMode,
        provider_supports_websockets: bool,
        model_catalog: CodexModelCatalogProfile,
    ) -> Self {
        Self {
            patch_mode,
            provider_identity: CodexProviderIdentity::from_patch_mode(patch_mode),
            auth_shape: CodexAuthShape::from_patch_mode(patch_mode),
            provider_supports_websockets,
            model_catalog,
        }
    }
}

impl CodexCapabilityProfile {
    pub fn for_patch_mode(
        patch_mode: CodexPatchMode,
        model_catalog: CodexModelCatalogProfile,
    ) -> Self {
        Self::for_input(CodexCapabilityProfileInput::from_patch_mode(
            patch_mode,
            model_catalog,
        ))
    }

    pub fn for_input(input: CodexCapabilityProfileInput) -> Self {
        let CodexCapabilityProfileInput {
            patch_mode,
            provider_identity,
            auth_shape,
            provider_supports_websockets,
            model_catalog,
        } = input;
        let remote_compaction_v1 = remote_compaction_v1_support(provider_identity);
        let hosted_image_generation = hosted_image_generation_support(auth_shape, &model_catalog);
        let responses_websocket = responses_websocket_support(provider_supports_websockets);
        let web_search = model_catalog.selected_web_search_support();
        let apply_patch = model_catalog.selected_apply_patch_support();
        let reasoning_summaries = model_catalog.selected_reasoning_summary_support();

        Self {
            patch_mode,
            provider_identity,
            auth_shape,
            provider_supports_websockets,
            model_catalog,
            remote_compaction_v1,
            hosted_image_generation,
            responses_websocket,
            web_search,
            apply_patch,
            reasoning_summaries,
        }
    }

    pub fn for_models_response_json(
        patch_mode: CodexPatchMode,
        models_response: &Value,
        selected_slug: Option<&str>,
    ) -> Self {
        Self::for_patch_mode(
            patch_mode,
            CodexModelCatalogProfile::from_models_response_json(models_response, selected_slug),
        )
    }
}

fn remote_compaction_v1_support(
    provider_identity: CodexProviderIdentity,
) -> CodexCapabilityDecision {
    match provider_identity {
        CodexProviderIdentity::OfficialOpenAi => CodexCapabilityDecision::supported(
            "provider identity is OpenAI, which makes Codex choose /responses/compact",
        ),
        CodexProviderIdentity::HelperRelay => CodexCapabilityDecision::unsupported(
            "provider identity is codex-helper, so Codex uses local compaction fallback",
        ),
    }
}

fn hosted_image_generation_support(
    auth_shape: CodexAuthShape,
    model_catalog: &CodexModelCatalogProfile,
) -> CodexCapabilityDecision {
    if !auth_shape.allows_codex_backend_tools() {
        return CodexCapabilityDecision::unsupported(
            "current patch mode does not make Codex auth look like Codex backend auth",
        );
    }

    match model_catalog.selected_image_input_support().support {
        CodexCapabilitySupport::Supported => CodexCapabilityDecision::supported(
            "auth shape allows Codex backend tools and selected model accepts image input",
        ),
        CodexCapabilitySupport::Unsupported => CodexCapabilityDecision::unsupported(
            "selected model metadata does not include image input modality",
        ),
        CodexCapabilitySupport::Unknown => CodexCapabilityDecision::unknown(format!(
            "auth shape allows Codex backend tools, but selected model image support is unknown: {}",
            model_catalog.reason
        )),
    }
}

fn responses_websocket_support(provider_supports_websockets: bool) -> CodexCapabilityDecision {
    if provider_supports_websockets {
        CodexCapabilityDecision::supported(
            "provider advertises supports_websockets, so Codex may choose Responses WebSocket transport",
        )
    } else {
        CodexCapabilityDecision::unsupported(
            "provider does not advertise Responses WebSocket transport",
        )
    }
}

fn image_input_support(model: &Value) -> CodexCapabilityDecision {
    let Some(modalities) = model.get("input_modalities") else {
        return CodexCapabilityDecision::supported(
            "Codex defaults missing input_modalities to text and image",
        );
    };
    let Some(modalities) = modalities.as_array() else {
        return CodexCapabilityDecision::unknown("input_modalities is not an array");
    };
    if modalities
        .iter()
        .any(|modality| modality.as_str() == Some("image"))
    {
        CodexCapabilityDecision::supported("input_modalities includes image")
    } else {
        CodexCapabilityDecision::unsupported("input_modalities does not include image")
    }
}

fn web_search_support(model: &Value) -> CodexCapabilityDecision {
    match model.get("supports_search_tool").and_then(Value::as_bool) {
        Some(true) => CodexCapabilityDecision::supported("supports_search_tool is true"),
        Some(false) => CodexCapabilityDecision::unsupported("supports_search_tool is false"),
        None => CodexCapabilityDecision::unsupported(
            "Codex defaults missing supports_search_tool to false",
        ),
    }
}

fn apply_patch_support(model: &Value) -> CodexCapabilityDecision {
    match model.get("apply_patch_tool_type").and_then(Value::as_str) {
        Some("freeform") => CodexCapabilityDecision::supported("apply_patch_tool_type is freeform"),
        Some(_) => CodexCapabilityDecision::unsupported("apply_patch_tool_type is not freeform"),
        None => CodexCapabilityDecision::unsupported("apply_patch_tool_type is missing"),
    }
}

fn reasoning_summary_support(model: &Value) -> CodexCapabilityDecision {
    match model
        .get("supports_reasoning_summaries")
        .and_then(Value::as_bool)
    {
        Some(true) => CodexCapabilityDecision::supported("supports_reasoning_summaries is true"),
        Some(false) => {
            CodexCapabilityDecision::unsupported("supports_reasoning_summaries is false")
        }
        None => CodexCapabilityDecision::unknown("supports_reasoning_summaries is missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn codex_models_response(model: Value) -> Value {
        json!({ "models": [model] })
    }

    fn image_capable_model(slug: &str) -> Value {
        json!({
            "slug": slug,
            "input_modalities": ["text", "image"],
            "supports_search_tool": true,
            "apply_patch_tool_type": "freeform",
            "supports_reasoning_summaries": true
        })
    }

    fn text_only_model(slug: &str) -> Value {
        json!({
            "slug": slug,
            "input_modalities": ["text"],
            "supports_search_tool": false,
            "apply_patch_tool_type": null,
            "supports_reasoning_summaries": false
        })
    }

    #[test]
    fn codex_capability_profile_official_imagegen_bridge_exposes_compact_and_imagegen() {
        let catalog = codex_models_response(image_capable_model("gpt-5.5"));

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::OfficialImagegenBridge,
            &catalog,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.responses_websocket.support,
            CodexCapabilitySupport::Unsupported
        );
        assert_eq!(
            profile.web_search.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.apply_patch.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[test]
    fn codex_capability_profile_official_relay_does_not_expose_imagegen_without_auth_facade() {
        let catalog = codex_models_response(image_capable_model("gpt-5.5"));

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::OfficialRelayBridge,
            &catalog,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Unsupported
        );
    }

    #[test]
    fn codex_capability_profile_imagegen_bridge_requires_image_capable_model() {
        let catalog = codex_models_response(text_only_model("gpt-5.3-codex-spark"));

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::ImagegenBridge,
            &catalog,
            Some("gpt-5.3-codex-spark"),
        );

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Unsupported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Unsupported
        );
        assert_eq!(
            profile.web_search.support,
            CodexCapabilitySupport::Unsupported
        );
        assert_eq!(
            profile.apply_patch.support,
            CodexCapabilitySupport::Unsupported
        );
    }

    #[test]
    fn codex_capability_profile_chatgpt_bridge_exposes_imagegen_but_not_compact() {
        let catalog = codex_models_response(image_capable_model("gpt-5.5"));

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::ChatGptBridge,
            &catalog,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Unsupported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[test]
    fn codex_capability_profile_allows_auth_shape_to_be_measured_separately_from_patch_mode() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let profile = CodexCapabilityProfile::for_input(CodexCapabilityProfileInput {
            patch_mode: CodexPatchMode::OfficialRelayBridge,
            provider_identity: CodexProviderIdentity::OfficialOpenAi,
            auth_shape: CodexAuthShape::CompleteChatGptLogin,
            provider_supports_websockets: false,
            model_catalog: catalog,
        });

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[test]
    fn codex_capability_profile_reports_websocket_if_provider_advertises_it() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let profile = CodexCapabilityProfile::for_input(CodexCapabilityProfileInput {
            patch_mode: CodexPatchMode::Default,
            provider_identity: CodexProviderIdentity::HelperRelay,
            auth_shape: CodexAuthShape::None,
            provider_supports_websockets: true,
            model_catalog: catalog,
        });

        assert_eq!(
            profile.responses_websocket.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[test]
    fn codex_capability_profile_official_imagegen_bridge_with_transport_exposes_compact_imagegen_and_websocket()
     {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let profile = CodexCapabilityProfile::for_input(
            CodexCapabilityProfileInput::from_patch_mode_with_transport(
                CodexPatchMode::OfficialImagegenBridge,
                true,
                catalog,
            ),
        );

        assert_eq!(
            profile.remote_compaction_v1.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
        assert_eq!(
            profile.responses_websocket.support,
            CodexCapabilitySupport::Supported
        );
    }

    #[test]
    fn codex_capability_profile_openai_data_catalog_requires_translation() {
        let models_response = json!({
            "object": "list",
            "data": [
                { "id": "gpt-5.5", "object": "model" }
            ]
        });

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::OfficialImagegenBridge,
            &models_response,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.model_catalog.shape,
            CodexModelCatalogShape::OpenAiDataList
        );
        assert!(profile.model_catalog.translation_required);
        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Unknown
        );
    }

    #[test]
    fn codex_capability_profile_missing_input_modalities_uses_codex_default() {
        let catalog = codex_models_response(json!({
            "slug": "gpt-5.5",
            "supports_search_tool": true,
            "apply_patch_tool_type": "freeform",
            "supports_reasoning_summaries": true
        }));

        let profile = CodexCapabilityProfile::for_models_response_json(
            CodexPatchMode::ImagegenBridge,
            &catalog,
            Some("gpt-5.5"),
        );

        assert_eq!(
            profile.hosted_image_generation.support,
            CodexCapabilitySupport::Supported
        );
    }

    fn recommendation(
        current_patch_mode: CodexPatchMode,
        model_catalog: CodexModelCatalogProfile,
        responses_compact: CodexCapabilitySupport,
    ) -> CodexPatchModeRecommendation {
        CodexPatchModeRecommendation::for_input(CodexPatchModeRecommendationInput {
            current_patch_mode,
            model_catalog,
            responses: CodexCapabilitySupport::Supported,
            responses_compact,
        })
    }

    #[test]
    fn codex_patch_mode_recommendation_uses_official_imagegen_when_compact_and_image_are_supported()
    {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let recommendation = recommendation(
            CodexPatchMode::Default,
            catalog,
            CodexCapabilitySupport::Supported,
        );

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::OfficialImagegenBridge
        );
        assert!(recommendation.changes_current_mode);
        assert_eq!(
            recommendation.confidence,
            CodexPatchModeRecommendationConfidence::Medium
        );
        assert!(
            recommendation
                .warnings
                .iter()
                .any(|warning| warning.contains("not actively probed"))
        );
    }

    #[test]
    fn codex_patch_mode_recommendation_uses_official_relay_without_image_capable_model() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(text_only_model("gpt-5.3-codex-spark")),
            Some("gpt-5.3-codex-spark"),
        );

        let recommendation = recommendation(
            CodexPatchMode::Default,
            catalog,
            CodexCapabilitySupport::Supported,
        );

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::OfficialRelayBridge
        );
        assert_eq!(
            recommendation.confidence,
            CodexPatchModeRecommendationConfidence::High
        );
    }

    #[test]
    fn codex_patch_mode_recommendation_keeps_imagegen_bridge_when_compact_is_unsupported() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let recommendation = recommendation(
            CodexPatchMode::OfficialImagegenBridge,
            catalog,
            CodexCapabilitySupport::Unsupported,
        );

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::ImagegenBridge
        );
        assert!(recommendation.changes_current_mode);
        assert!(
            recommendation
                .reasons
                .iter()
                .any(|reason| reason.contains("local compaction"))
        );
    }

    #[test]
    fn codex_patch_mode_recommendation_uses_default_when_no_official_gate_is_proven() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(text_only_model("gpt-5.3-codex-spark")),
            Some("gpt-5.3-codex-spark"),
        );

        let recommendation = recommendation(
            CodexPatchMode::OfficialRelayBridge,
            catalog,
            CodexCapabilitySupport::Unsupported,
        );

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::Default
        );
        assert_eq!(
            recommendation.confidence,
            CodexPatchModeRecommendationConfidence::High
        );
    }

    #[test]
    fn codex_patch_mode_recommendation_does_not_upgrade_to_official_when_compact_is_unknown() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let recommendation = recommendation(
            CodexPatchMode::Default,
            catalog,
            CodexCapabilitySupport::Unknown,
        );

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::ImagegenBridge
        );
        assert_eq!(
            recommendation.confidence,
            CodexPatchModeRecommendationConfidence::Low
        );
        assert!(
            recommendation
                .warnings
                .iter()
                .any(|warning| warning.contains("avoid official relay identity"))
        );
    }

    #[test]
    fn codex_patch_mode_recommendation_warns_when_responses_endpoint_is_not_available() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &codex_models_response(image_capable_model("gpt-5.5")),
            Some("gpt-5.5"),
        );

        let recommendation =
            CodexPatchModeRecommendation::for_input(CodexPatchModeRecommendationInput {
                current_patch_mode: CodexPatchMode::OfficialImagegenBridge,
                model_catalog: catalog,
                responses: CodexCapabilitySupport::Unsupported,
                responses_compact: CodexCapabilitySupport::Supported,
            });

        assert_eq!(
            recommendation.recommended_patch_mode,
            CodexPatchMode::Default
        );
        assert!(
            recommendation
                .warnings
                .iter()
                .any(|warning| warning.contains("no Codex patch mode can compensate"))
        );
    }
}

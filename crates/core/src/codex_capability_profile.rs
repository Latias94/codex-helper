use serde::{Deserialize, Serialize};
use serde_json::Value;

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

impl Default for CodexCapabilityDecision {
    fn default() -> Self {
        Self::unknown("capability was not reported by this response")
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
        Self {
            shape: CodexModelCatalogShape::CodexModels,
            selection: CodexModelSelection::Selected,
            translation_required: false,
            selected_model: Some(CodexModelCapabilityProfile::from_codex_model_json(selected)),
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
            slug,
            accepts_image_input: image_input_support(model),
            supports_web_search: web_search_support(model),
            supports_apply_patch: apply_patch_support(model),
            supports_reasoning_summaries: reasoning_summary_support(model),
        }
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

    #[test]
    fn selected_codex_model_exposes_reported_capabilities() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &json!({
                "models": [{
                    "slug": "gpt-5.6-sol",
                    "input_modalities": ["text", "image"],
                    "supports_search_tool": true,
                    "apply_patch_tool_type": "freeform",
                    "supports_reasoning_summaries": true
                }]
            }),
            Some("gpt-5.6-sol"),
        );

        assert_eq!(catalog.shape, CodexModelCatalogShape::CodexModels);
        assert_eq!(catalog.selection, CodexModelSelection::Selected);
        assert!(catalog.selected_image_input_support().is_supported());
        assert!(catalog.selected_web_search_support().is_supported());
        assert!(catalog.selected_apply_patch_support().is_supported());
        assert!(catalog.selected_reasoning_summary_support().is_supported());
    }

    #[test]
    fn openai_data_list_requires_translation() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &json!({
                "object": "list",
                "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
            }),
            Some("gpt-5.6-sol"),
        );

        assert_eq!(catalog.shape, CodexModelCatalogShape::OpenAiDataList);
        assert_eq!(catalog.selection, CodexModelSelection::NotApplicable);
        assert!(catalog.translation_required);
        assert_eq!(
            catalog.selected_image_input_support().support,
            CodexCapabilitySupport::Unknown
        );
    }

    #[test]
    fn missing_selected_model_is_reported_without_inventing_capabilities() {
        let catalog = CodexModelCatalogProfile::from_models_response_json(
            &json!({ "models": [{ "slug": "gpt-5.6-terra" }] }),
            Some("gpt-5.6-sol"),
        );

        assert_eq!(catalog.selection, CodexModelSelection::Missing);
        assert!(catalog.selected_model.is_none());
        assert_eq!(
            catalog.selected_apply_patch_support().support,
            CodexCapabilitySupport::Unknown
        );
    }

    #[test]
    fn missing_input_modalities_uses_codex_default() {
        let model = CodexModelCapabilityProfile::from_codex_model_json(&json!({
            "slug": "gpt-5.6-luna"
        }));

        assert_eq!(
            model.accepts_image_input.support,
            CodexCapabilitySupport::Supported
        );
    }
}

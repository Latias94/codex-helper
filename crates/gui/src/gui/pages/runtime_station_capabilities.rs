use super::*;

fn capability_support_short_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "是",
        (Language::Zh, CapabilitySupport::Unsupported) => "否",
        (Language::Zh, CapabilitySupport::Unknown) => "?",
        (_, CapabilitySupport::Supported) => "yes",
        (_, CapabilitySupport::Unsupported) => "no",
        (_, CapabilitySupport::Unknown) => "?",
    }
}

pub(super) fn capability_support_label(lang: Language, support: CapabilitySupport) -> &'static str {
    match (lang, support) {
        (Language::Zh, CapabilitySupport::Supported) => "支持",
        (Language::Zh, CapabilitySupport::Unsupported) => "不支持",
        (Language::Zh, CapabilitySupport::Unknown) => "未知",
        (_, CapabilitySupport::Supported) => "supported",
        (_, CapabilitySupport::Unsupported) => "unsupported",
        (_, CapabilitySupport::Unknown) => "unknown",
    }
}

pub(super) fn format_runtime_config_capability_label(
    lang: Language,
    capabilities: &StationCapabilitySummary,
) -> String {
    let model_label = match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => {
            format!("{}:any", pick(lang, "模型", "models"))
        }
        ModelCatalogKind::Declared => {
            format!(
                "{}:{}",
                pick(lang, "模型", "models"),
                capabilities.supported_models.len()
            )
        }
    };
    format!(
        "{model_label} | tier:{} | effort:{}",
        capability_support_short_label(lang, capabilities.supports_service_tier),
        capability_support_short_label(lang, capabilities.supports_reasoning_effort),
    )
}

pub(super) fn runtime_config_capability_hover_text(
    lang: Language,
    capabilities: &StationCapabilitySummary,
) -> String {
    let mut lines = Vec::new();
    match capabilities.model_catalog_kind {
        ModelCatalogKind::ImplicitAny => lines.push(
            pick(
                lang,
                "模型能力: 未显式声明，当前按 implicit any 处理",
                "Model support: not declared explicitly; current routing treats this station as implicit-any",
            )
            .to_string(),
        ),
        ModelCatalogKind::Declared => {
            if capabilities.supported_models.is_empty() {
                lines.push(
                    pick(
                        lang,
                        "模型能力: 已声明，但没有正向可用模型模式",
                        "Model support: declared, but no positive model patterns are available",
                    )
                    .to_string(),
                );
            } else {
                lines.push(format!(
                    "{}: {}",
                    pick(lang, "模型列表", "Models"),
                    capabilities.supported_models.join(", ")
                ));
            }
        }
    }
    lines.push(format!(
        "{}: {}",
        pick(lang, "Fast/Service tier", "Fast/Service tier"),
        capability_support_label(lang, capabilities.supports_service_tier)
    ));
    lines.push(format!(
        "{}: {}",
        pick(lang, "思考强度", "Reasoning effort"),
        capability_support_label(lang, capabilities.supports_reasoning_effort)
    ));
    lines.push(
        pick(
            lang,
            "来源: supported_models/model_mapping 与 upstream tags",
            "Source: supported_models/model_mapping plus upstream tags",
        )
        .to_string(),
    );
    lines.join("\n")
}

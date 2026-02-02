#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Zh,
    En,
}

pub fn parse_language(s: &str) -> Option<Language> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    match s.as_str() {
        "zh" | "zh-cn" | "zh_cn" | "zh-hans" | "zh_hans" | "cn" | "chinese" | "中文" => {
            Some(Language::Zh)
        }
        "en" | "en-us" | "en_us" | "english" => Some(Language::En),
        _ => None,
    }
}

pub fn detect_system_language() -> Language {
    // Best-effort: prefer env vars to avoid platform-specific commands.
    // Common values:
    // - LANG=zh_CN.UTF-8
    // - LC_ALL=zh_CN.UTF-8
    // - LANGUAGE=zh_CN:en_US
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_ascii_lowercase();
            if v.starts_with("zh") || v.contains("zh_cn") || v.contains("zh-cn") {
                return Language::Zh;
            }
            if v.starts_with("en") {
                return Language::En;
            }
        }
    }
    Language::En
}

pub(crate) fn pick<'a>(lang: Language, zh: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}

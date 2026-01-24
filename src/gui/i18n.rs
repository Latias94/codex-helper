#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Zh,
    En,
}

pub fn pick<'a>(lang: Language, zh: &'a str, en: &'a str) -> &'a str {
    match lang {
        Language::Zh => zh,
        Language::En => en,
    }
}

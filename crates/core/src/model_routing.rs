use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WildcardPattern {
    prefix: String,
    suffix: String,
    specificity: usize,
}

impl WildcardPattern {
    fn parse(pattern: &str) -> Option<Self> {
        let star = pattern.find('*')?;
        if pattern[star + 1..].contains('*') {
            return None;
        }
        let prefix = pattern[..star].to_string();
        let suffix = pattern[star + 1..].to_string();
        Some(Self {
            specificity: prefix.len() + suffix.len(),
            prefix,
            suffix,
        })
    }

    fn matches(&self, text: &str) -> bool {
        text.len() >= self.specificity
            && text.starts_with(self.prefix.as_str())
            && text.ends_with(self.suffix.as_str())
    }

    fn overlaps(&self, other: &Self) -> bool {
        (self.prefix.starts_with(other.prefix.as_str())
            || other.prefix.starts_with(self.prefix.as_str()))
            && (self.suffix.ends_with(other.suffix.as_str())
                || other.suffix.ends_with(self.suffix.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompiledWildcardRule<T> {
    source: String,
    pattern: WildcardPattern,
    value: T,
}

struct CompiledRuleSet<T> {
    exact: BTreeMap<String, T>,
    wildcard: Vec<CompiledWildcardRule<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRuleCompileError {
    rule_set: &'static str,
    first: String,
    second: String,
    specificity: usize,
}

impl fmt::Display for ModelRuleCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ambiguous {} wildcard model rules '{}' and '{}' overlap with equal specificity {}",
            self.rule_set, self.first, self.second, self.specificity
        )
    }
}

impl Error for ModelRuleCompileError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompiledModelRules {
    exact_supported: BTreeMap<String, bool>,
    wildcard_supported: Vec<CompiledWildcardRule<bool>>,
    exact_mapping: BTreeMap<String, String>,
    wildcard_mapping: Vec<CompiledWildcardRule<String>>,
}

impl CompiledModelRules {
    pub fn compile(
        supported_models: impl IntoIterator<Item = (String, bool)>,
        model_mapping: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ModelRuleCompileError> {
        let supported = compile_rule_set("supported-model", supported_models)?;
        let mapping = compile_rule_set("model-mapping", model_mapping)?;
        Ok(Self {
            exact_supported: supported.exact,
            wildcard_supported: supported.wildcard,
            exact_mapping: mapping.exact,
            wildcard_mapping: mapping.wildcard,
        })
    }

    pub fn effective_model(&self, requested_model: &str) -> String {
        if let Some(mapped) = self.exact_mapping.get(requested_model) {
            return mapped.clone();
        }
        self.matching_mapping(requested_model)
            .map(|rule| {
                apply_wildcard_mapping(rule.source.as_str(), rule.value.as_str(), requested_model)
            })
            .unwrap_or_else(|| requested_model.to_string())
    }

    pub fn is_model_supported(&self, requested_model: &str) -> bool {
        if self.exact_supported.is_empty()
            && self.wildcard_supported.is_empty()
            && self.exact_mapping.is_empty()
            && self.wildcard_mapping.is_empty()
        {
            return true;
        }

        let supported = self
            .exact_supported
            .get(requested_model)
            .copied()
            .or_else(|| {
                self.wildcard_supported
                    .iter()
                    .find(|rule| rule.pattern.matches(requested_model))
                    .map(|rule| rule.value)
            });
        if supported == Some(true) {
            return true;
        }

        self.exact_mapping.contains_key(requested_model)
            || self.matching_mapping(requested_model).is_some()
    }

    fn matching_mapping(&self, requested_model: &str) -> Option<&CompiledWildcardRule<String>> {
        self.wildcard_mapping
            .iter()
            .find(|rule| rule.pattern.matches(requested_model))
    }
}

fn compile_rule_set<T>(
    rule_set: &'static str,
    rules: impl IntoIterator<Item = (String, T)>,
) -> Result<CompiledRuleSet<T>, ModelRuleCompileError> {
    let mut exact = BTreeMap::new();
    let mut wildcard: Vec<CompiledWildcardRule<T>> = Vec::new();
    for (source, value) in rules {
        let Some(pattern) = WildcardPattern::parse(source.as_str()) else {
            exact.insert(source, value);
            continue;
        };
        if let Some(conflict) = wildcard.iter().find(|existing| {
            existing.pattern.specificity == pattern.specificity
                && existing.pattern.overlaps(&pattern)
        }) {
            return Err(ModelRuleCompileError {
                rule_set,
                first: conflict.source.clone(),
                second: source,
                specificity: pattern.specificity,
            });
        }
        wildcard.push(CompiledWildcardRule {
            source,
            pattern,
            value,
        });
    }
    wildcard.sort_by(|left, right| {
        right
            .pattern
            .specificity
            .cmp(&left.pattern.specificity)
            .then_with(|| left.source.cmp(&right.source))
    });
    Ok(CompiledRuleSet { exact, wildcard })
}

pub fn match_wildcard(pattern: &str, text: &str) -> bool {
    WildcardPattern::parse(pattern)
        .map_or_else(|| pattern == text, |wildcard| wildcard.matches(text))
}

fn wildcard_specificity(pattern: &str) -> Option<usize> {
    WildcardPattern::parse(pattern).map(|wildcard| wildcard.specificity)
}

pub fn apply_wildcard_mapping(pattern: &str, replacement: &str, input: &str) -> String {
    if !pattern.contains('*') || !replacement.contains('*') {
        return replacement.to_string();
    }

    let Some(pattern) = WildcardPattern::parse(pattern) else {
        return replacement.to_string();
    };
    if !pattern.matches(input) {
        return replacement.to_string();
    }

    let wildcard_part = &input[pattern.prefix.len()..input.len() - pattern.suffix.len()];
    replacement.replacen('*', wildcard_part, 1)
}

pub fn effective_model(model_mapping: &HashMap<String, String>, requested_model: &str) -> String {
    if model_mapping.is_empty() {
        return requested_model.to_string();
    }

    if let Some(mapped) = model_mapping.get(requested_model) {
        return mapped.clone();
    }

    let best = model_mapping
        .iter()
        .filter(|(pattern, _)| match_wildcard(pattern, requested_model))
        .filter_map(|(pattern, replacement)| {
            wildcard_specificity(pattern)
                .map(|specificity| (pattern.as_str(), replacement.as_str(), specificity))
        })
        .max_by(|left, right| left.2.cmp(&right.2).then_with(|| right.0.cmp(left.0)));
    if let Some((pattern, replacement, _)) = best {
        return apply_wildcard_mapping(pattern, replacement, requested_model);
    }

    requested_model.to_string()
}

pub fn is_model_supported(
    supported_models: &HashMap<String, bool>,
    model_mapping: &HashMap<String, String>,
    requested_model: &str,
) -> bool {
    if supported_models.is_empty() && model_mapping.is_empty() {
        return true;
    }

    if supported_models
        .get(requested_model)
        .copied()
        .unwrap_or(false)
    {
        return true;
    }
    for key in supported_models.keys() {
        if match_wildcard(key, requested_model) {
            return true;
        }
    }

    if model_mapping.contains_key(requested_model) {
        return true;
    }
    for key in model_mapping.keys() {
        if match_wildcard(key, requested_model) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_model_rules_reject_overlapping_wildcards_with_equal_specificity() {
        let error = CompiledModelRules::compile(
            [],
            [
                ("ab*cd".to_string(), "first-*".to_string()),
                ("abc*d".to_string(), "second-*".to_string()),
            ],
        )
        .expect_err("equal-specificity wildcard overlap must be rejected");

        let message = error.to_string();
        assert!(message.contains("ab*cd"), "unexpected error: {message}");
        assert!(message.contains("abc*d"), "unexpected error: {message}");
        assert!(
            message.contains("specificity"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn compiled_model_rules_reject_supported_model_wildcard_ties() {
        let error = CompiledModelRules::compile(
            [("ab*cd".to_string(), true), ("abc*d".to_string(), false)],
            [],
        )
        .expect_err("supported-model wildcard tie must be rejected");

        assert!(error.to_string().contains("supported-model"));
    }

    #[test]
    fn compiled_model_rules_are_independent_of_input_iteration_order() {
        let first = CompiledModelRules::compile(
            [],
            [
                ("gpt-*".to_string(), "fallback-*".to_string()),
                ("gpt-5-*".to_string(), "preferred-*".to_string()),
            ],
        )
        .expect("compile first rule order");
        let second = CompiledModelRules::compile(
            [],
            [
                ("gpt-5-*".to_string(), "preferred-*".to_string()),
                ("gpt-*".to_string(), "fallback-*".to_string()),
            ],
        )
        .expect("compile reverse rule order");

        assert_eq!(first.effective_model("gpt-5-mini"), "preferred-mini");
        assert_eq!(
            first.effective_model("gpt-5-mini"),
            second.effective_model("gpt-5-mini")
        );
    }
}

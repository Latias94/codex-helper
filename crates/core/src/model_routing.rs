use std::collections::HashMap;

pub fn match_wildcard(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() != 2 {
        return false;
    }
    let (prefix, suffix) = (parts[0], parts[1]);
    text.starts_with(prefix) && text.ends_with(suffix)
}

fn wildcard_specificity(pattern: &str) -> Option<usize> {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() != 2 {
        return None;
    }
    Some(parts[0].len() + parts[1].len())
}

pub fn apply_wildcard_mapping(pattern: &str, replacement: &str, input: &str) -> String {
    if !pattern.contains('*') || !replacement.contains('*') {
        return replacement.to_string();
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() != 2 {
        return replacement.to_string();
    }
    let (prefix, suffix) = (parts[0], parts[1]);
    if !input.starts_with(prefix) || !input.ends_with(suffix) {
        return replacement.to_string();
    }

    let wildcard_part = &input[prefix.len()..input.len().saturating_sub(suffix.len())];
    replacement.replacen('*', wildcard_part, 1)
}

pub fn effective_model(model_mapping: &HashMap<String, String>, requested_model: &str) -> String {
    if model_mapping.is_empty() {
        return requested_model.to_string();
    }

    if let Some(mapped) = model_mapping.get(requested_model) {
        return mapped.clone();
    }

    let mut best: Option<(&str, &str, usize)> = None;
    for (pattern, replacement) in model_mapping.iter() {
        if !match_wildcard(pattern, requested_model) {
            continue;
        }
        let Some(spec) = wildcard_specificity(pattern) else {
            continue;
        };
        match best {
            None => best = Some((pattern.as_str(), replacement.as_str(), spec)),
            Some((_, _, best_spec)) if spec > best_spec => {
                best = Some((pattern.as_str(), replacement.as_str(), spec));
            }
            _ => {}
        }
    }
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

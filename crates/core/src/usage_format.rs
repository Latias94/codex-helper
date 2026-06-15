use crate::usage::UsageMetrics;

pub fn tokens_short(value: i64) -> String {
    let value = value.max(0) as f64;
    if value >= 1_000_000_000.0 {
        format!("{:.1}b", value / 1_000_000_000.0)
    } else if value >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

pub fn tokens_per_second(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "-".to_string())
}

pub fn usage_line_with_labels(
    usage: &UsageMetrics,
    usage_label: &str,
    cache_label: &str,
) -> String {
    let mut line = format!(
        "{usage_label}: {}/{}/{}/{}",
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_output_tokens_total()),
        tokens_short(usage.total_tokens)
    );
    if usage.has_cache_tokens() {
        line.push_str(&format!(
            " {cache_label}: {}/{}",
            tokens_short(usage.cache_read_tokens_total()),
            tokens_short(usage.cache_creation_tokens_total())
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_short_scales_counts() {
        assert_eq!(tokens_short(-1), "0");
        assert_eq!(tokens_short(999), "999");
        assert_eq!(tokens_short(1_500), "1.5k");
        assert_eq!(tokens_short(1_500_000), "1.5m");
        assert_eq!(tokens_short(1_500_000_000), "1.5b");
    }

    #[test]
    fn tokens_per_second_formats_positive_finite_values() {
        assert_eq!(tokens_per_second(None), "-");
        assert_eq!(tokens_per_second(Some(0.0)), "-");
        assert_eq!(tokens_per_second(Some(f64::INFINITY)), "-");
        assert_eq!(tokens_per_second(Some(12.34)), "12.3");
    }

    #[test]
    fn usage_line_with_labels_includes_cache_when_present() {
        let usage = UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 20,
            reasoning_output_tokens: 3,
            total_tokens: 1_023,
            cached_input_tokens: 10,
            cache_creation_input_tokens: 5,
            ..UsageMetrics::default()
        };

        assert_eq!(
            usage_line_with_labels(&usage, "tok in/out/rsn/ttl", "cache read/create"),
            "tok in/out/rsn/ttl: 1.0k/20/3/1.0k cache read/create: 10/5"
        );
    }
}

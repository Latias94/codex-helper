use super::*;

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn format_age(now_ms: u64, ts_ms: Option<u64>) -> String {
    let Some(ts) = ts_ms else {
        return "-".to_string();
    };
    if now_ms <= ts {
        return "0s".to_string();
    }
    let mut secs = (now_ms - ts) / 1000;
    let days = secs / 86400;
    secs %= 86400;
    let hours = secs / 3600;
    secs %= 3600;
    let mins = secs / 60;
    secs %= 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins}m")
    } else if mins > 0 {
        format!("{mins}m{secs}s")
    } else {
        format!("{secs}s")
    }
}

pub(super) fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    if duration_ms < 60_000 {
        return format!("{:.1}s", duration_ms as f64 / 1_000.0);
    }
    if duration_ms < 3_600_000 {
        let total_secs = duration_ms / 1_000;
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        return if secs == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m{secs}s")
        };
    }
    if duration_ms < 86_400_000 {
        let total_secs = duration_ms / 1_000;
        let hours = total_secs / 3_600;
        let mins = (total_secs % 3_600) / 60;
        return if mins == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{mins}m")
        };
    }
    let total_hours = duration_ms / 3_600_000;
    let days = total_hours / 24;
    let hours = total_hours % 24;
    if hours == 0 {
        format!("{days}d")
    } else {
        format!("{days}d{hours}h")
    }
}

pub(super) fn format_duration_ms_opt(duration_ms: Option<u64>) -> String {
    duration_ms
        .filter(|value| *value > 0)
        .map(format_duration_ms)
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

pub(super) fn shorten(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

pub(super) fn short_sid(s: &str, max_chars: usize) -> String {
    shorten(s, max_chars)
}

pub(super) fn shorten_middle(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let left = max_chars / 2;
    let right = max_chars.saturating_sub(left).saturating_sub(1);
    let mut out = String::new();
    for ch in chars.iter().take(left) {
        out.push(*ch);
    }
    out.push('…');
    for ch in chars.iter().skip(chars.len().saturating_sub(right)) {
        out.push(*ch);
    }
    out
}

pub(super) fn summarize_upstream_target(raw: &str, max_chars: usize) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "-".to_string();
    }
    let after_scheme = raw.split_once("://").map(|(_, rest)| rest).unwrap_or(raw);
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if host.is_empty() {
        shorten_middle(raw, max_chars)
    } else {
        host.to_string()
    }
}

fn tokens_short(n: i64) -> String {
    let n = n.max(0) as f64;
    if n >= 1_000_000.0 {
        format!("{:.1}m", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}k", n / 1_000.0)
    } else {
        format!("{:.0}", n)
    }
}

pub(super) fn usage_line(usage: &UsageMetrics) -> String {
    let mut line = format!(
        "tok in/out/rsn/ttl: {}/{}/{}/{}",
        tokens_short(usage.input_tokens),
        tokens_short(usage.output_tokens),
        tokens_short(usage.reasoning_output_tokens_total()),
        tokens_short(usage.total_tokens)
    );
    if usage.has_cache_tokens() {
        line.push_str(&format!(
            " cache read/create: {}/{}",
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
    fn format_duration_ms_scales_units() {
        assert_eq!(format_duration_ms(999), "999ms");
        assert_eq!(format_duration_ms(1_500), "1.5s");
        assert_eq!(format_duration_ms(90_000), "1m30s");
        assert_eq!(format_duration_ms(3_600_000), "1h");
    }
}

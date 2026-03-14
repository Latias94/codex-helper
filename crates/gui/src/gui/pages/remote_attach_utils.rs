pub(super) fn merge_info_message<I>(base: String, extras: I) -> String
where
    I: IntoIterator<Item = String>,
{
    let extras = extras
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if extras.is_empty() {
        base
    } else {
        format!("{base} {}", extras.join(" "))
    }
}

pub(super) fn management_base_url_is_loopback(base_url: &str) -> bool {
    let input = base_url.trim();
    if input.is_empty() {
        return false;
    }

    let after_scheme = input
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(input);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme)
        .trim();
    if authority.is_empty() {
        return false;
    }

    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host).unwrap_or(rest)
    } else if let Some((host, _)) = authority.rsplit_once(':') {
        host
    } else {
        authority
    };

    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

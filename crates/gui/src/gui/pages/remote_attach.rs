use super::*;
use crate::gui::proxy_control::{ProxyController, ProxyModeKind};

pub(super) fn remote_safe_surface_status_line(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    let host_only = format_host_local_capability_summary(caps, lang).unwrap_or_else(|| {
        pick(
            lang,
            "会话历史 / transcript read / cwd enrichment",
            "session history / transcript read / cwd enrichment",
        )
        .to_string()
    });

    Some(match lang {
        Language::Zh => format!(
            "当前是远程附着：会话控制台、站点/健康台和共享观测/配置面仍通过控制面访问；{host_only} 这类 host-local 能力仍只在代理主机本地可用。"
        ),
        Language::En => format!(
            "Remote attach: the Session Console, station/health console, and shared observed/config surfaces remain available through the control plane; host-local capabilities remain on the proxy host only: {host_only}."
        ),
    })
}

pub(super) fn remote_attached_proxy_active(proxy: &ProxyController) -> bool {
    matches!(proxy.kind(), ProxyModeKind::Attached) && !host_local_session_features_available(proxy)
}

pub(super) fn attached_host_local_session_features_available(
    admin_base_url: &str,
    host_local_session_history: bool,
) -> bool {
    management_base_url_is_loopback(admin_base_url) && host_local_session_history
}

fn format_host_local_capability_summary(
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
) -> Option<String> {
    let mut parts = Vec::new();
    if caps.session_history {
        parts.push(pick(lang, "会话历史", "session history"));
    }
    if caps.transcript_read {
        parts.push(pick(lang, "对话读取", "transcript read"));
    }
    if caps.cwd_enrichment {
        parts.push(pick(lang, "cwd 补全", "cwd enrichment"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

pub(super) fn remote_local_only_warning_message(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
    requested_features: &[&str],
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    let requested = if requested_features.is_empty() {
        pick(lang, "这些功能", "these features").to_string()
    } else {
        requested_features.join(" / ")
    };

    match (lang, format_host_local_capability_summary(caps, lang)) {
        (Language::Zh, Some(summary)) => Some(format!(
            "当前是远端附着：{requested} 属于 host-local 功能，这台设备不能直接访问。附着目标声明这些能力只在代理主机本地可用：{summary}；如需使用，请在代理主机上运行 codex-helper GUI。"
        )),
        (Language::Zh, None) => Some(format!(
            "当前是远端附着：{requested} 属于 host-local 功能，这台设备不能直接访问。附着目标也没有声明可供主机本地使用的 session/transcript/cwd 能力；如需使用，请切回本机代理或在代理主机上运行 codex-helper GUI。"
        )),
        (Language::En, Some(summary)) => Some(format!(
            "This is a remote attach: {requested} are host-local features and are not directly available from this device. The attached target reports these as host-only capabilities on the proxy machine: {summary}. Run codex-helper GUI on the proxy host to use them."
        )),
        (Language::En, None) => Some(format!(
            "This is a remote attach: {requested} are host-local features and are not directly available from this device. The attached target does not advertise host-local session/transcript/cwd capabilities either. Use a local proxy on this device or run codex-helper GUI on the proxy host."
        )),
    }
}

pub(super) fn remote_admin_token_present() -> bool {
    std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(super) fn remote_admin_access_short_label(
    admin_base_url: &str,
    caps: &RemoteAdminAccessCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }
    if !caps.remote_enabled {
        return Some(
            pick(lang, "远端 admin 未开放", "Remote admin locked to loopback").to_string(),
        );
    }
    if !remote_admin_token_present() {
        return Some(
            pick(
                lang,
                "远端 admin 需 token（本机未设置）",
                "Remote admin needs token (client missing)",
            )
            .to_string(),
        );
    }
    Some(pick(lang, "远端 admin 已启用 token", "Remote admin token ready").to_string())
}

pub(super) fn remote_admin_access_message(
    admin_base_url: &str,
    caps: &RemoteAdminAccessCapabilities,
    lang: Language,
) -> Option<String> {
    if management_base_url_is_loopback(admin_base_url) {
        return None;
    }

    if !caps.remote_enabled {
        return Some(match lang {
            Language::Zh => format!(
                "当前目标的 admin 控制面仍是 loopback-only。要允许 LAN/Tailscale 设备附着，请在代理主机设置环境变量 {}，客户端随后需通过请求头 {} 发送相同 token。",
                caps.token_env_var, caps.token_header
            ),
            Language::En => format!(
                "This target keeps its admin control plane loopback-only. To allow LAN/Tailscale attach, set {} on the proxy host, then clients must send the same token via header {}.",
                caps.token_env_var, caps.token_header
            ),
        });
    }

    if !remote_admin_token_present() {
        return Some(match lang {
            Language::Zh => format!(
                "目标已开放远端 admin，但当前 GUI 进程未设置 {}。若继续远端附着，admin 请求会被拒绝；请在当前设备设置该环境变量，并让其值与代理主机一致，请求头名为 {}。",
                caps.token_env_var, caps.token_header
            ),
            Language::En => format!(
                "The target allows remote admin, but this GUI process has no {} set. Remote attach admin requests will be rejected until this device provides the same token; the required header name is {}.",
                caps.token_env_var, caps.token_header
            ),
        });
    }

    Some(match lang {
        Language::Zh => format!(
            "当前远端 admin 将通过环境变量 {} 注入，并以请求头 {} 发送。请确保客户端与代理主机使用相同 token。",
            caps.token_env_var, caps.token_header
        ),
        Language::En => format!(
            "Remote admin will use the token from env {} and send it via header {}. Ensure the client and proxy host use the same token value.",
            caps.token_env_var, caps.token_header
        ),
    })
}

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

pub(super) fn host_local_session_features_available(proxy: &ProxyController) -> bool {
    match proxy.kind() {
        ProxyModeKind::Attached => proxy.attached().is_some_and(|attached| {
            attached_host_local_session_features_available(
                attached.admin_base_url.as_str(),
                attached.host_local_capabilities.session_history,
            )
        }),
        _ => true,
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

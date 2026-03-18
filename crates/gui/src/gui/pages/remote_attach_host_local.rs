use crate::gui::proxy_control::{ProxyController, ProxyModeKind};

use super::*;

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

pub(super) fn remote_safe_surface_status_line(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
) -> Option<String> {
    if super::management_base_url_is_loopback(admin_base_url) {
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
            "当前是远程附着：会话控制台、站点/健康台和共享观测/控制面仍通过控制面访问；{host_only} 这类 host-local 能力仍只在代理主机本地可用。"
        ),
        Language::En => format!(
            "Remote attach: the Session Console, station/health console, and shared observed/control surfaces remain available through the control plane; host-local capabilities remain on the proxy host only: {host_only}."
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
    super::management_base_url_is_loopback(admin_base_url) && host_local_session_history
}

pub(super) fn remote_local_only_warning_message(
    admin_base_url: &str,
    caps: &HostLocalControlPlaneCapabilities,
    lang: Language,
    requested_features: &[&str],
) -> Option<String> {
    if super::management_base_url_is_loopback(admin_base_url) {
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

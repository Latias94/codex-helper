use super::*;

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
    if super::management_base_url_is_loopback(admin_base_url) {
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
    if super::management_base_url_is_loopback(admin_base_url) {
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

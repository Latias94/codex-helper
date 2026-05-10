use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::config::{
    ProviderConfigV3, ProxyConfig, ProxyConfigV3, RoutingConfigV3, RoutingExhaustedActionV3,
    RoutingPolicyV3, ServiceKind, ServiceViewV3, UpstreamAuth, compile_v3_to_runtime,
};
use crate::state::ProxyState;
use tokio::sync::watch;

use super::helpers::env_lock;
use super::*;

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, content).expect("write test file");
}

fn v3_config(base_url: &str) -> ProxyConfig {
    compile_v3_to_runtime(&ProxyConfigV3 {
        version: 3,
        codex: ServiceViewV3 {
            default_profile: None,
            profiles: BTreeMap::new(),
            providers: BTreeMap::from([(
                "monthly".to_string(),
                ProviderConfigV3 {
                    base_url: Some(base_url.to_string()),
                    auth: UpstreamAuth::default(),
                    inline_auth: UpstreamAuth::default(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    endpoints: BTreeMap::new(),
                    alias: Some("Monthly".to_string()),
                    enabled: true,
                },
            )]),
            routing: Some(RoutingConfigV3 {
                policy: RoutingPolicyV3::OrderedFailover,
                order: vec!["monthly".to_string()],
                target: None,
                prefer_tags: Vec::new(),
                on_exhausted: RoutingExhaustedActionV3::Continue,
            }),
        },
        ..ProxyConfigV3::default()
    })
    .expect("compile v3 config")
}

fn running_controller(cfg: ProxyConfig) -> ProxyController {
    let mut controller = ProxyController::new(3210, ServiceKind::Codex);
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);

    controller.mode = ProxyMode::Running(RunningProxy {
        service_name: "codex",
        port: 3210,
        admin_port: 4321,
        state: ProxyState::new(),
        cfg: Arc::new(cfg),
        last_refresh: None,
        last_error: None,
        active: Vec::new(),
        recent: Vec::new(),
        session_cards: Vec::new(),
        global_station_override: None,
        configured_active_station: Some("routing".to_string()),
        effective_active_station: Some("routing".to_string()),
        configured_default_profile: None,
        default_profile: None,
        profiles: Vec::new(),
        session_model_overrides: HashMap::new(),
        session_station_overrides: HashMap::new(),
        session_effort_overrides: HashMap::new(),
        session_service_tier_overrides: HashMap::new(),
        session_stats: HashMap::new(),
        stations: Vec::new(),
        station_health: HashMap::new(),
        provider_balances: HashMap::new(),
        health_checks: HashMap::new(),
        usage_rollup: Default::default(),
        stats_5m: Default::default(),
        stats_1h: Default::default(),
        configured_retry: None,
        resolved_retry: None,
        lb_view: HashMap::new(),
        shutdown_tx,
        server_handle: None,
    });
    controller
}

#[test]
fn running_config_sync_from_disk_reloads_local_cfg_snapshot() {
    let _lock = env_lock();
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "codex-helper-gui-runtime-sync-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let proxy_home = dir.join(".codex-helper");
    let codex_home = dir.join(".codex");
    let claude_home = dir.join(".claude");
    std::fs::create_dir_all(&proxy_home).expect("create proxy home");
    std::fs::create_dir_all(&codex_home).expect("create codex home");
    std::fs::create_dir_all(&claude_home).expect("create claude home");

    let mut scoped = super::helpers::ScopedEnv::default();
    unsafe {
        scoped.set("CODEX_HELPER_HOME", proxy_home.to_string_lossy().as_ref());
        scoped.set("CODEX_HOME", codex_home.to_string_lossy().as_ref());
        scoped.set("CLAUDE_HOME", claude_home.to_string_lossy().as_ref());
        scoped.set("HOME", dir.to_string_lossy().as_ref());
        scoped.set("USERPROFILE", dir.to_string_lossy().as_ref());
    }

    let config_path = proxy_home.join("config.toml");
    write_file(
        &config_path,
        r#"
version = 3

[codex.providers.monthly]
base_url = "https://old.example.com/v1"

[codex.routing]
policy = "ordered-failover"
order = ["monthly"]
"#,
    );

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut controller = running_controller(v3_config("https://old.example.com/v1"));

    write_file(
        &config_path,
        r#"
version = 3

[codex.providers.monthly]
base_url = "https://new.example.com/v1"

[codex.routing]
policy = "ordered-failover"
order = ["monthly"]
"#,
    );

    controller
        .sync_running_config_from_disk(&rt)
        .expect("sync running config");

    let running = controller.running().expect("running mode");
    assert_eq!(
        running
            .cfg
            .codex
            .station("routing")
            .expect("routing station")
            .upstreams[0]
            .base_url,
        "https://new.example.com/v1"
    );
}

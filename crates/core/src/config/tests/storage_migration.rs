use super::*;
use std::path::PathBuf;
use std::task::Poll;

struct TempConfigDir(PathBuf);

impl TempConfigDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "codex-helper-migration-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("create temporary config directory");
        Self(path)
    }

    fn paths(&self) -> ResolvedConfigDirectory {
        ResolvedConfigDirectory {
            logical_path: self.0.clone(),
            resolved_path: std::fs::canonicalize(&self.0)
                .expect("canonicalize temporary config directory"),
        }
    }
}

impl Drop for TempConfigDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("write temporary config");
}

fn migrated_provider_order(plan: &ConfigMigrationPlan) -> Vec<String> {
    let config = toml::from_str::<HelperConfig>(&plan.rendered)
        .expect("parse migrated helper configuration");
    crate::routing_ir::compile_route_handshake_plan("codex", &config.codex)
        .expect("compile migrated Codex route")
        .candidates
        .into_iter()
        .map(|candidate| candidate.provider_id)
        .collect()
}

#[tokio::test]
async fn migration_dry_run_does_not_modify_source() {
    let temp = TempConfigDir::new();
    let source = r#"
version = 5

[codex.client_patch]
preset = "default"

[codex.providers.primary]
base_url = "https://relay.example/v1"
auth_token = "inline-secret-for-test"
api_key = "inline-api-key-for-test"

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["primary"]
"#;
    let path = temp.0.join("config.toml");
    write(&path, source);
    let before = std::fs::read(&path).expect("read source before dry-run");

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build migration plan");
    assert!(
        plan.notices
            .iter()
            .any(|notice| notice.contains("codex.client_patch"))
    );
    assert!(plan.rendered.contains("version = 5"));
    assert!(!plan.rendered.contains("client_patch"));
    let report = plan.report(false);
    assert!(!report.contains("inline-secret-for-test"));
    assert!(!report.contains("inline-api-key-for-test"));
    assert!(report.contains("<redacted>"));
    assert_eq!(
        std::fs::read(&path).expect("read source after dry-run"),
        before
    );
    assert!(!temp.0.join("config.toml.bak").exists());
}

#[tokio::test]
async fn migration_write_creates_backup_and_v5_output() {
    let temp = TempConfigDir::new();
    let source = r#"
version = 4

[codex.providers.primary]
base_url = "https://relay.example/v1"

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["primary"]
"#;
    let path = temp.0.join("config.toml");
    write(&path, source);

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build migration plan");
    apply_config_migration_plan(&temp.paths(), &plan)
        .await
        .expect("apply migration plan");

    assert_eq!(
        std::fs::read(temp.0.join("config.toml.bak")).expect("read backup"),
        source.as_bytes()
    );
    let migrated = std::fs::read_to_string(&path).expect("read migrated config");
    let parsed = toml::from_str::<HelperConfig>(&migrated).expect("parse migrated config");
    assert_eq!(parsed.version, CURRENT_CONFIG_VERSION);
    assert!(migrated.contains("version = 5"));
}

#[tokio::test]
async fn stale_auto_migration_is_noop_after_another_writer_completed_and_preserves_backup() {
    let temp = TempConfigDir::new();
    let source = r#"
version = 4

[codex.providers.primary]
base_url = "https://relay.example/v1"
"#;
    let path = temp.0.join("config.toml");
    write(&path, source);
    let paths = temp.paths();

    auto_migrate_legacy_config(&paths)
        .await
        .expect("first automatic migration");
    let migrated = std::fs::read(&path).expect("read first migrated config");
    assert_eq!(
        std::fs::read(temp.0.join("config.toml.bak")).expect("read original backup"),
        source.as_bytes()
    );

    auto_migrate_legacy_config(&paths)
        .await
        .expect("stale automatic migration should become a no-op");

    assert_eq!(
        std::fs::read(&path).expect("read config after stale migration"),
        migrated
    );
    assert_eq!(
        std::fs::read(temp.0.join("config.toml.bak")).expect("read preserved backup"),
        source.as_bytes()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_migration_waits_for_lock_then_rechecks_without_rewriting_backup() {
    let temp = TempConfigDir::new();
    let source = "version = 4\n[notify]\nenabled = true\n";
    let path = temp.0.join("config.toml");
    write(&path, source);
    let paths = temp.paths();
    let holder = ConfigMutationLock::try_acquire(&paths).expect("hold config mutation lock");

    let mut waiting = Box::pin(auto_migrate_legacy_config(&paths));
    assert!(matches!(
        futures_util::poll!(waiting.as_mut()),
        Poll::Pending
    ));

    let winner = build_config_migration_plan(&paths)
        .await
        .expect("build winning migration plan");
    apply_config_migration_plan(&paths, &winner)
        .await
        .expect("apply winning migration");
    let migrated = std::fs::read(&path).expect("read winner output");
    let backup = std::fs::read(temp.0.join("config.toml.bak")).expect("read winner backup");

    drop(holder);
    waiting
        .await
        .expect("waiting automatic migration should recheck and become a no-op");

    assert_eq!(std::fs::read(path).expect("read final config"), migrated);
    assert_eq!(
        std::fs::read(temp.0.join("config.toml.bak")).expect("read final backup"),
        backup
    );
    assert_eq!(backup, source.as_bytes());
}

#[tokio::test]
async fn migration_validation_failure_leaves_source_and_backup_untouched() {
    let temp = TempConfigDir::new();
    let source = r#"
version = 4

[codex.providers.primary]
base_url = ""
"#;
    let path = temp.0.join("config.toml");
    write(&path, source);
    let before = std::fs::read(&path).expect("read source before failed migration");

    let error = build_config_migration_plan(&temp.paths())
        .await
        .expect_err("invalid provider should fail validation");
    assert!(
        error
            .to_string()
            .contains("validate migrated configuration")
    );
    assert_eq!(
        std::fs::read(&path).expect("read source after failed migration"),
        before
    );
    assert!(!temp.0.join("config.toml.bak").exists());
}

#[tokio::test]
async fn migration_converts_v3_routing_and_json_sources() {
    let temp = TempConfigDir::new();
    let source = r#"
version = 3

[codex.providers.primary]
base_url = "https://relay.example/v1"

[codex.providers.backup]
base_url = "https://backup.example/v1"

[codex.routing]
policy = "manual-sticky"
target = "backup"
order = ["backup", "primary"]
"#;
    let toml_path = temp.0.join("config.toml");
    write(&toml_path, source);
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("convert v3 routing");
    assert!(plan.rendered.contains("strategy = \"manual-sticky\""));
    assert!(plan.rendered.contains("target = \"backup\""));

    std::fs::remove_file(&toml_path).expect("remove temporary toml source");
    write(
        &temp.0.join("config.json"),
        r#"{"version":5,"codex":{"providers":{"primary":{"base_url":"https://relay.example/v1"}}}}"#,
    );
    let json_plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("convert json source");
    assert_eq!(json_plan.source_name, "config.json");
    apply_config_migration_plan(&temp.paths(), &json_plan)
        .await
        .expect("apply json migration");
    assert!(temp.0.join("config.json.bak").exists());
    assert!(temp.0.join("config.toml").exists());
}

#[tokio::test]
async fn migration_rejects_explicit_invalid_versions_without_shape_inference() {
    let cases = [
        ("config.toml", "version = 0\n"),
        ("config.toml", "version = -1\n"),
        ("config.toml", "version = 4.0\n"),
        ("config.toml", "version = true\n"),
        (
            "config.toml",
            "version = \"1\"\n[codex.configs.primary]\n[[codex.configs.primary.upstreams]]\nbase_url = \"https://relay.example/v1\"\n",
        ),
        ("config.json", r#"{"version":0,"notify":{"enabled":true}}"#),
        (
            "config.json",
            r#"{"version":"1","notify":{"enabled":true}}"#,
        ),
    ];

    for (source_name, source) in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join(source_name);
        write(&path, source);
        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("an explicit invalid version must not be inferred");
        assert!(
            error.to_string().contains("invalid config version"),
            "unexpected invalid-version error for {source_name}: {error}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read source"), source);
        assert!(!temp.0.join(format!("{source_name}.bak")).exists());
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[tokio::test]
async fn migration_rejects_malformed_known_legacy_fields_without_writing() {
    let cases = [
        (
            "v1 station enabled",
            r#"version = 1
[codex.configs.primary]
enabled = "false"
[[codex.configs.primary.upstreams]]
base_url = "https://primary.example/v1"
"#,
            "codex.configs.primary.enabled",
        ),
        (
            "v2 group members",
            r#"version = 2
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.groups.primary]
members = "primary"
"#,
            "codex.groups.primary.members",
        ),
        (
            "v3 routing policy",
            r#"version = 3
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.routing]
policy = 123
order = ["primary"]
"#,
            "codex.routing.policy",
        ),
    ];

    for (label, source, expected_path) in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join("config.toml");
        write(&path, source);

        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err(label);
        let message = format!("{error:#}");
        assert!(
            message.contains(expected_path),
            "{label} did not identify {expected_path}: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read rejected legacy source"),
            source
        );
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[tokio::test]
async fn migration_preserves_nested_retry_as_the_complete_legacy_override() {
    let temp = TempConfigDir::new();
    write(
        &temp.0.join("config.toml"),
        r#"version = 2

[retry]
max_attempts = 7
backoff_ms = 125
backoff_max_ms = 2500
jitter_ms = 40
on_status = "429,500-599"
on_class = ["upstream_transport_error"]
strategy = "same_upstream"

[retry.upstream]
max_attempts = 3
"#,
    );

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("migrate flat retry settings");
    let migrated =
        toml::from_str::<HelperConfig>(&plan.rendered).expect("parse migrated retry configuration");
    let upstream = migrated
        .retry
        .upstream
        .expect("existing retry.upstream should be retained");
    assert_eq!(upstream.max_attempts, Some(3));
    assert_eq!(upstream.backoff_ms, None);
    assert_eq!(upstream.backoff_max_ms, None);
    assert_eq!(upstream.jitter_ms, None);
    assert_eq!(upstream.on_status, None);
    assert_eq!(upstream.on_class, None);
    assert_eq!(upstream.strategy, None);

    let raw = toml::from_str::<TomlValue>(&plan.rendered).expect("parse raw migrated TOML");
    let retry = raw
        .get("retry")
        .and_then(TomlValue::as_table)
        .expect("retry table");
    for field in [
        "max_attempts",
        "backoff_ms",
        "backoff_max_ms",
        "jitter_ms",
        "on_status",
        "on_class",
        "strategy",
    ] {
        assert!(
            !retry.contains_key(field),
            "flat retry field {field} remained"
        );
    }
    assert!(
        plan.notices
            .iter()
            .any(|notice| notice.contains("ignored") && notice.contains("retry.upstream"))
    );
}

#[tokio::test]
async fn migration_retains_v3_routing_extensions_and_rejects_ambiguous_extensions() {
    let temp = TempConfigDir::new();
    let path = temp.0.join("config.toml");
    write(
        &path,
        r#"version = 3
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.routing]
policy = "ordered-failover"
order = ["primary"]
operator_extension = "keep-routing-extension"
"#,
    );
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("retain a v3 routing extension");
    let raw = toml::from_str::<TomlValue>(&plan.rendered).expect("parse migrated TOML");
    assert_eq!(
        nested_toml_value(&raw, &["codex", "routing", "operator_extension"])
            .and_then(TomlValue::as_str),
        Some("keep-routing-extension")
    );

    let ambiguous_cases = [
        (
            r#"version = 1
[codex.configs.primary]
operator_extension = "ambiguous"
[[codex.configs.primary.upstreams]]
base_url = "https://primary.example/v1"
"#,
            "codex.configs.primary.operator_extension",
        ),
        (
            r#"version = 2
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.groups.primary]
members = [{ provider = "primary", weight = 2 }]
"#,
            "codex.groups.primary.members[0].weight",
        ),
        (
            r#"version = 3
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.routing]
policy = "pool-fallback"
chain = ["primary_pool"]
[codex.routing.pools.primary_pool]
providers = ["primary"]
weight = 2
"#,
            "codex.routing.pools.primary_pool.weight",
        ),
    ];

    for (source, expected_path) in ambiguous_cases {
        write(&path, source);
        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("ambiguous legacy extension must fail closed");
        assert!(
            format!("{error:#}").contains(expected_path),
            "ambiguous extension error did not identify {expected_path}: {error:#}"
        );
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn migration_rejects_permission_change_before_creating_backup() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempConfigDir::new();
    let path = temp.0.join("config.toml");
    let source = "version = 4\n[notify]\nenabled = true\n";
    write(&path, source);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("set initial permissions");
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build migration plan");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("tighten permissions after planning");
    let error = apply_config_migration_plan(&temp.paths(), &plan)
        .await
        .expect_err("permission changes must invalidate the plan before backup");
    assert!(
        format!("{error:#}").contains("permissions changed while migration was being prepared")
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read unchanged source"),
        source
    );
    assert!(
        !temp.0.join("config.toml.bak").exists(),
        "permission race must not publish a backup with stale permissions"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn migration_dry_run_rejects_a_file_symlink_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let temp = TempConfigDir::new();
    let target = temp.0.join("legacy-target.toml");
    let logical = temp.0.join("config.toml");
    let source = "version = 4\n[notify]\nenabled = true\n";
    write(&target, source);
    symlink(&target, &logical).expect("create config file symlink");

    let error = build_config_migration_plan(&temp.paths())
        .await
        .expect_err("dry-run must reject a migration that write mode cannot apply");
    assert!(format!("{error:#}").contains("symbolic link"));
    assert_eq!(
        std::fs::read_to_string(&target).expect("read untouched symlink target"),
        source
    );
    assert!(
        std::fs::symlink_metadata(&logical)
            .expect("inspect logical symlink")
            .file_type()
            .is_symlink()
    );
    assert!(!temp.0.join("config.toml.bak").exists());
}

#[tokio::test]
async fn migration_preserves_v1_station_priority_and_stable_disabled_fallback() {
    let temp = TempConfigDir::new();
    let path = temp.0.join("config.toml");
    write(
        &path,
        r#"version = 1

[codex]
active = "zeta"

[codex.configs.low]
enabled = true
level = 1
[[codex.configs.low.upstreams]]
base_url = "https://low.example/v1"

[codex.configs.alpha]
enabled = true
level = 5
[[codex.configs.alpha.upstreams]]
base_url = "https://alpha.example/v1"

[codex.configs.zeta]
enabled = true
level = 5
[[codex.configs.zeta.upstreams]]
base_url = "https://zeta.example/v1"
"#,
    );
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("migrate prioritized v1 stations");
    assert_eq!(
        migrated_provider_order(&plan),
        ["low__u01", "zeta__u01", "alpha__u01"]
    );

    write(
        &path,
        r#"version = 1

[codex.configs.zeta]
enabled = false
[[codex.configs.zeta.upstreams]]
base_url = "https://zeta.example/v1"

[codex.configs.alpha]
enabled = false
[[codex.configs.alpha.upstreams]]
base_url = "https://alpha.example/v1"
"#,
    );
    let fallback = build_config_migration_plan(&temp.paths())
        .await
        .expect("migrate all-disabled v1 stations");
    assert_eq!(migrated_provider_order(&fallback), ["alpha__u01"]);
}

#[tokio::test]
async fn migration_filters_v2_disabled_groups_and_keeps_stable_fallback() {
    let cases = [
        (
            r#"version = 2
[codex.providers.disabled]
base_url = "https://disabled.example/v1"
[codex.providers.enabled]
base_url = "https://enabled.example/v1"
[codex.groups.disabled]
enabled = false
level = 1
members = [{ provider = "disabled" }]
[codex.groups.enabled]
enabled = true
level = 2
members = [{ provider = "enabled" }]
"#,
            vec!["enabled"],
        ),
        (
            r#"version = 2
[codex.providers.alpha]
base_url = "https://alpha.example/v1"
[codex.providers.zeta]
base_url = "https://zeta.example/v1"
[codex.groups.zeta]
enabled = false
members = [{ provider = "zeta" }]
[codex.groups.alpha]
enabled = false
members = [{ provider = "alpha" }]
"#,
            vec!["alpha"],
        ),
        (
            r#"version = 2
[codex]
active_group = "disabled"
[codex.providers.disabled]
base_url = "https://disabled.example/v1"
[codex.providers.enabled]
base_url = "https://enabled.example/v1"
[codex.groups.disabled]
enabled = false
level = 1
members = [{ provider = "disabled" }]
[codex.groups.enabled]
enabled = true
level = 2
members = [{ provider = "enabled" }]
"#,
            vec!["disabled", "enabled"],
        ),
    ];

    for (source, expected) in cases {
        let temp = TempConfigDir::new();
        write(&temp.0.join("config.toml"), source);
        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("migrate v2 groups");
        assert_eq!(migrated_provider_order(&plan), expected);
    }
}

#[tokio::test]
async fn v2_scoped_member_migration_never_routes_unselected_endpoints() {
    for endpoint_field in ["endpoint_names", "endpoints"] {
        let temp = TempConfigDir::new();
        let source = format!(
            r#"version = 2

[codex.providers.relay.endpoints.hk]
base_url = "https://hk.example/v1"
priority = 0

[codex.providers.relay.endpoints.us]
base_url = "https://us.example/v1"
priority = 1

[codex.groups.main]
members = [{{ provider = "relay", {endpoint_field} = ["us"] }}]
"#
        );
        write(&temp.0.join("config.toml"), &source);

        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("migrate endpoint-scoped v2 group");
        let config = toml::from_str::<HelperConfig>(&plan.rendered)
            .expect("parse endpoint-scoped migrated config");
        let endpoint_ids = crate::routing_ir::compile_route_handshake_plan("codex", &config.codex)
            .expect("compile endpoint-scoped migrated route")
            .candidates
            .into_iter()
            .map(|candidate| candidate.endpoint_id)
            .collect::<Vec<_>>();

        assert_eq!(endpoint_ids, vec!["us"]);
        assert!(
            plan.notices
                .iter()
                .any(|notice| notice.contains("explicit endpoint scoping"))
        );
    }
}

#[tokio::test]
async fn migration_rejects_empty_v3_pool_fallback_and_preserves_valid_chain_order() {
    let temp = TempConfigDir::new();
    let path = temp.0.join("config.toml");
    let invalid = r#"version = 3
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.routing]
policy = "pool-fallback"
"#;
    write(&path, invalid);
    let error = build_config_migration_plan(&temp.paths())
        .await
        .expect_err("pool-fallback without pools must fail closed");
    assert!(error.to_string().contains("requires at least one pool"));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read source"),
        invalid
    );
    assert!(!temp.0.join("config.toml.bak").exists());

    write(
        &path,
        r#"version = 3
[codex.providers.input]
base_url = "https://input.example/v1"
[codex.providers.ciii]
base_url = "https://ciii.example/v1"
[codex.routing]
policy = "pool-fallback"
chain = ["input_pool", "ciii_pool"]
[codex.routing.pools.input_pool]
providers = ["input"]
[codex.routing.pools.ciii_pool]
providers = ["ciii"]
"#,
    );
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("migrate valid pool chain");
    assert_eq!(migrated_provider_order(&plan), ["input", "ciii"]);

    let stopped = std::fs::read_to_string(&path)
        .expect("read pool source")
        .replace(
            "policy = \"pool-fallback\"",
            "policy = \"pool-fallback\"\non_exhausted = \"stop\"",
        );
    write(&path, &stopped);
    let stopped_plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("migrate stop-on-exhaustion pool chain");
    assert_eq!(migrated_provider_order(&stopped_plan), ["input"]);
}

#[tokio::test]
async fn migration_preview_redacts_headers_without_changing_written_candidate() {
    let temp = TempConfigDir::new();
    write(
        &temp.0.join("config.toml"),
        r#"version = 5

[codex.client_patch]
preset = "default"

[ui.service_status]
enabled = true

[[ui.service_status.probes]]
id = "private"
url = "https://status.example/health"
headers = { Authorization = "Bearer preview-secret", Cookie = "session=preview-cookie", "x-api-key" = "preview-key" }
"#,
    );
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build current-v5 cleanup preview");
    let report = plan.report(false);
    for secret in ["preview-secret", "preview-cookie", "preview-key"] {
        assert!(!report.contains(secret), "preview leaked {secret}");
        assert!(
            plan.rendered.contains(secret),
            "migration candidate lost {secret}"
        );
    }
    assert!(report.contains("<redacted>"));
}

#[tokio::test]
async fn migration_retains_unknown_nested_fields_after_validating_known_contract() {
    let temp = TempConfigDir::new();
    write(
        &temp.0.join("config.toml"),
        r#"version = 4
operator_extension = "keep-root"

[codex.providers.primary]
base_url = "https://primary.example/v1"
provider_extension = "keep-provider"

[codex.routing]
entry = "main"

[codex.routing.routes.main]
strategy = "ordered-failover"
children = ["primary"]
route_extension = "keep-route"
"#,
    );
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("validate migration while retaining unknown fields");
    for marker in ["keep-root", "keep-provider", "keep-route"] {
        assert!(plan.rendered.contains(marker), "migration dropped {marker}");
    }
    assert!(plan.notices.iter().any(|notice| {
        notice.contains("operator_extension") && notice.contains("retained verbatim")
    }));
}

#[tokio::test]
async fn migration_rechecks_toml_source_immediately_before_replace() {
    let temp = TempConfigDir::new();
    let path = temp.0.join("config.toml");
    let source = "version = 4\n[notify]\nenabled = true\n";
    write(&path, source);
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build migration plan");
    let changed_path = path.clone();

    let error =
        apply_config_migration_plan_with_before_replace(&temp.paths(), &plan, move |_, _| {
            std::fs::write(&changed_path, "version = 5\n# concurrent user edit\n")
        })
        .await
        .expect_err("a concurrent source edit must prevent replacement");
    assert!(
        format!("{error:#}").contains("changed while migration was being prepared"),
        "unexpected source race error: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read preserved concurrent edit"),
        "version = 5\n# concurrent user edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.0.join("config.toml.bak"))
            .expect("read original source backup"),
        source
    );
}

#[tokio::test]
async fn json_migration_rechecks_that_canonical_toml_did_not_appear() {
    let temp = TempConfigDir::new();
    let json_path = temp.0.join("config.json");
    let toml_path = temp.0.join("config.toml");
    let source = r#"{"version":4,"notify":{"enabled":true}}"#;
    write(&json_path, source);
    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("build JSON migration plan");
    let appeared_path = toml_path.clone();

    let error =
        apply_config_migration_plan_with_before_replace(&temp.paths(), &plan, move |_, _| {
            std::fs::write(&appeared_path, "version = 5\n# user-owned TOML\n")
        })
        .await
        .expect_err("an appearing canonical TOML must prevent JSON replacement");
    assert!(
        format!("{error:#}").contains("config.toml appeared while migrating config.json"),
        "unexpected target race error: {error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(&toml_path).expect("read preserved user TOML"),
        "version = 5\n# user-owned TOML\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.0.join("config.json.bak")).expect("read JSON backup"),
        source
    );
}

#[tokio::test]
async fn migration_config_init_imports_json_only_installations() {
    let temp = TempConfigDir::new();
    let source = r#"{"version":4,"notify":{"enabled":true}}"#;
    write(&temp.0.join("config.json"), source);

    let outcome = init_config_toml_at_paths(&temp.paths(), false)
        .await
        .expect("config init should migrate legacy JSON");

    let migrated = std::fs::read_to_string(outcome.path).expect("read migrated config.toml");
    assert!(migrated.contains("version = 5"));
    assert!(migrated.contains("enabled = true"));
    assert!(
        outcome
            .migration_report
            .as_deref()
            .is_some_and(|report| report.contains("Migrated config.json"))
    );
    assert_eq!(
        std::fs::read_to_string(temp.0.join("config.json")).expect("read preserved JSON source"),
        source
    );
    assert_eq!(
        std::fs::read_to_string(temp.0.join("config.json.bak"))
            .expect("read exact JSON migration backup"),
        source
    );
}

#[tokio::test]
async fn migration_config_init_reports_plain_template_creation_without_a_legacy_source() {
    let temp = TempConfigDir::new();

    let outcome = init_config_toml_at_paths(&temp.paths(), false)
        .await
        .expect("config init should create a template for a new installation");

    assert_eq!(outcome.path, temp.0.join("config.toml"));
    assert!(outcome.migration_report.is_none());
    assert!(
        std::fs::read_to_string(outcome.path)
            .expect("read generated config.toml")
            .contains("version = 5")
    );
    assert!(!temp.0.join("config.toml.bak").exists());
    assert!(!temp.0.join("config.json.bak").exists());
}

#[tokio::test]
async fn migration_rejects_ambiguous_v2_scoped_endpoint_references() {
    let cases = [
        r#"version = 2

[codex.providers.relay.endpoints.us]
base_url = "https://relay-us.example/v1"

[codex.providers."relay.us"]
base_url = "https://different-provider.example/v1"

[codex.groups.selected]
members = [{ provider = "relay", endpoint_names = ["us"] }]
"#,
        r#"version = 2

[codex.providers."relay.us".endpoints.west]
base_url = "https://intended.example/v1"

[codex.providers.relay.endpoints."us.west"]
base_url = "https://wrong-provider.example/v1"

[codex.groups.selected]
members = [{ provider = "relay.us", endpoints = ["west"] }]
"#,
        r#"version = 2

[codex.providers." relay ".endpoints.us]
base_url = "https://intended.example/v1"

[codex.providers.relay.endpoints.us]
base_url = "https://wrong-provider.example/v1"

[codex.groups.selected]
members = [{ provider = " relay ", endpoint_names = ["us"] }]
"#,
        r#"version = 2

[codex.providers.relay.endpoints." us "]
base_url = "https://intended.example/v1"

[codex.providers.relay.endpoints.us]
base_url = "https://wrong-endpoint.example/v1"

[codex.groups.selected]
members = [{ provider = "relay", endpoint_names = [" us "] }]
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join("config.toml");
        write(&path, source);

        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("ambiguous provider.endpoint reference must fail closed");
        let message = format!("{error:#}");
        assert!(message.contains("ambiguous"), "unexpected error: {message}");
        assert_eq!(std::fs::read_to_string(path).expect("read source"), source);
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[tokio::test]
async fn migration_rejects_v2_services_without_any_routable_group_members() {
    let cases = [
        r#"version = 2
[codex.providers.primary]
base_url = "https://primary.example/v1"
"#,
        r#"version = 2
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.groups.selected]
members = []
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join("config.toml");
        write(&path, source);

        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("v2 migration must not fall back to every provider");
        assert!(
            format!("{error:#}").contains("no routable group members"),
            "unexpected empty-group error: {error:#}"
        );
        assert_eq!(std::fs::read_to_string(path).expect("read source"), source);
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[tokio::test]
async fn migration_rejects_mixed_legacy_and_current_service_ownership() {
    let cases = [
        (
            r#"version = 1
[codex.providers.current]
base_url = "https://current.example/v1"
[codex.configs.legacy]
[[codex.configs.legacy.upstreams]]
base_url = "https://legacy.example/v1"
"#,
            "providers",
        ),
        (
            r#"version = 2
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.groups.selected]
members = [{ provider = "primary" }]
[codex.routing]
entry = "existing"
[codex.routing.routes.existing]
strategy = "ordered-failover"
children = ["primary"]
"#,
            "routing",
        ),
    ];

    for (source, conflicting_field) in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join("config.toml");
        write(&path, source);

        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("mixed ownership must fail before transformation");
        let message = format!("{error:#}");
        assert!(message.contains("mixes") && message.contains(conflicting_field));
        assert_eq!(std::fs::read_to_string(path).expect("read source"), source);
        assert!(!temp.0.join("config.toml.bak").exists());
    }
}

#[tokio::test]
async fn migration_uses_a_non_conflicting_route_entry_for_provider_named_main() {
    let cases = [
        r#"version = 2
[codex.providers.main]
base_url = "https://main.example/v1"
[codex.groups.selected]
members = [{ provider = "main" }]
"#,
        r#"version = 3
[codex.providers.main]
base_url = "https://main.example/v1"
[codex.routing]
policy = "ordered-failover"
order = ["main"]
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        write(&temp.0.join("config.toml"), source);
        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("provider named main should remain routable");
        let config = toml::from_str::<HelperConfig>(&plan.rendered)
            .expect("parse migrated provider named main");
        assert_ne!(
            config.codex.routing.expect("migrated route graph").entry,
            "main"
        );
        assert_eq!(migrated_provider_order(&plan), ["main"]);
    }
}

#[tokio::test]
async fn migration_write_is_a_noop_for_clean_v5_and_preserves_legacy_backup() {
    let temp = TempConfigDir::new();
    let source = "version = 5\n[notify]\nenabled = true\n";
    let backup = "version = 2\n# only surviving legacy backup\n";
    let path = temp.0.join("config.toml");
    let backup_path = temp.0.join("config.toml.bak");
    write(&path, source);
    write(&backup_path, backup);

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("validate clean v5 config");
    assert!(!plan.requires_write);
    apply_config_migration_plan(&temp.paths(), &plan)
        .await
        .expect("clean v5 migration should be a no-op");

    assert_eq!(std::fs::read_to_string(path).expect("read config"), source);
    assert_eq!(
        std::fs::read_to_string(backup_path).expect("read preserved backup"),
        backup
    );
    assert!(plan.report(true).contains("no files were written"));
}

#[tokio::test]
async fn migration_rejects_missing_active_station_or_group_references() {
    let cases = [
        r#"version = 1
[codex]
active = "missing"
[codex.configs.present]
[[codex.configs.present.upstreams]]
base_url = "https://present.example/v1"
"#,
        r#"version = 2
[codex]
active_group = "missing"
[codex.providers.present]
base_url = "https://present.example/v1"
[codex.groups.present]
members = [{ provider = "present" }]
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        let path = temp.0.join("config.toml");
        write(&path, source);
        let error = build_config_migration_plan(&temp.paths())
            .await
            .expect_err("missing active reference must fail closed");
        assert!(
            format!("{error:#}").contains("references missing"),
            "unexpected active-reference error: {error:#}"
        );
        assert_eq!(std::fs::read_to_string(path).expect("read source"), source);
    }
}

#[tokio::test]
async fn migration_clamps_legacy_levels_before_ordering() {
    let cases = [
        (
            r#"version = 1
[codex.configs.zeta]
level = 0
[[codex.configs.zeta.upstreams]]
base_url = "https://zeta.example/v1"
[codex.configs.alpha]
level = 1
[[codex.configs.alpha.upstreams]]
base_url = "https://alpha.example/v1"
"#,
            vec!["alpha__u01", "zeta__u01"],
        ),
        (
            r#"version = 2
[codex.providers.zeta]
base_url = "https://zeta.example/v1"
[codex.providers.alpha]
base_url = "https://alpha.example/v1"
[codex.groups.zeta]
level = 10
members = [{ provider = "zeta" }]
[codex.groups.alpha]
level = 255
members = [{ provider = "alpha" }]
"#,
            vec!["alpha", "zeta"],
        ),
    ];

    for (source, expected) in cases {
        let temp = TempConfigDir::new();
        write(&temp.0.join("config.toml"), source);
        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("migrate clamped legacy levels");
        assert_eq!(migrated_provider_order(&plan), expected);
    }
}

#[tokio::test]
async fn migration_infers_legacy_shapes_per_service_without_rejecting_current_peers() {
    let temp = TempConfigDir::new();
    write(
        &temp.0.join("config.toml"),
        r#"[codex.providers.legacy]
base_url = "https://legacy.example/v1"
[codex.groups.selected]
members = [{ provider = "legacy" }]

[claude.providers.current]
base_url = "https://current.example/v1"
[claude.routing]
entry = "current_route"
[claude.routing.routes.current_route]
strategy = "ordered-failover"
children = ["current"]
"#,
    );

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("service-local v2 shape should not claim current peer ownership");
    let migrated = toml::from_str::<HelperConfig>(&plan.rendered)
        .expect("parse service-local inferred migration");
    assert_eq!(migrated_provider_order(&plan), ["legacy"]);
    assert_eq!(
        migrated
            .claude
            .routing
            .expect("preserved current Claude route graph")
            .entry,
        "current_route"
    );
}

#[tokio::test]
async fn migration_detects_current_version_files_that_still_contain_legacy_shapes() {
    let temp = TempConfigDir::new();
    let source = r#"version = 5
[codex.providers.primary]
base_url = "https://primary.example/v1"
[codex.groups.selected]
members = [{ provider = "primary" }]
"#;
    let path = temp.0.join("config.toml");
    write(&path, source);

    assert!(
        automatic_config_migration_required(&temp.paths())
            .await
            .expect("inspect current-version legacy shape")
    );
    auto_migrate_legacy_config(&temp.paths())
        .await
        .expect("auto-migrate current-version legacy shape");

    let migrated = std::fs::read_to_string(&path).expect("read migrated config");
    assert!(!migrated.contains("[codex.groups."));
    assert_eq!(
        std::fs::read_to_string(temp.0.join("config.toml.bak"))
            .expect("read exact legacy-shape backup"),
        source
    );
    assert!(
        existing_backup_is_legacy_migration_source(&temp.paths())
            .await
            .expect("classify current-version legacy backup")
    );
}

#[tokio::test]
async fn migration_preserves_active_station_and_group_names_exactly() {
    let cases = [
        r#"version = 1
[codex]
active = " team "
[codex.configs." team "]
[[codex.configs." team ".upstreams]]
base_url = "https://spaced.example/v1"
[codex.configs.team]
[[codex.configs.team.upstreams]]
base_url = "https://trimmed.example/v1"
"#,
        r#"version = 2
[codex]
active_group = " team "
[codex.providers.spaced]
base_url = "https://spaced.example/v1"
[codex.providers.trimmed]
base_url = "https://trimmed.example/v1"
[codex.groups." team "]
members = [{ provider = "spaced" }]
[codex.groups.team]
members = [{ provider = "trimmed" }]
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        write(&temp.0.join("config.toml"), source);
        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("active identity should match an exact legacy key");
        let config =
            toml::from_str::<HelperConfig>(&plan.rendered).expect("parse exact-active migration");
        let base_urls = crate::routing_ir::compile_route_handshake_plan("codex", &config.codex)
            .expect("compile exact-active route")
            .candidates
            .into_iter()
            .map(|candidate| candidate.base_url)
            .collect::<Vec<_>>();
        assert_eq!(
            base_urls,
            [
                "https://spaced.example/v1".to_string(),
                "https://trimmed.example/v1".to_string(),
            ]
        );
    }
}

#[tokio::test]
async fn migration_treats_omitted_v1_upstreams_as_the_historical_empty_default() {
    let cases = [
        r#"version = 1
[codex.configs.empty]
[codex.configs.full]
[[codex.configs.full.upstreams]]
base_url = "https://full.example/v1"
"#,
        r#"[codex.configs.empty]
"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        write(&temp.0.join("config.toml"), source);
        let plan = build_config_migration_plan(&temp.paths())
            .await
            .expect("omitted v1 upstreams should migrate as an empty list");
        assert!(!plan.rendered.contains("[codex.configs"));
        toml::from_str::<HelperConfig>(&plan.rendered)
            .expect("parse migration with an empty legacy station");
    }
}

#[tokio::test]
async fn migration_accepts_historically_nullable_json_fields() {
    let temp = TempConfigDir::new();
    let source = r#"{
  "version": null,
  "default_service": null,
  "codex": {
    "active": null,
    "default_profile": null,
    "profiles": {
      "deep": {
        "extends": null,
        "station": null,
        "model": null,
        "reasoning_effort": null,
        "service_tier": null
      }
    },
    "stations": {
      "primary": {
        "name": "primary",
        "alias": null,
        "enabled": true,
        "level": 1,
        "upstreams": [{
          "base_url": "https://relay.example/v1",
          "auth": {
            "auth_token": null,
            "auth_token_env": null,
            "api_key": null,
            "api_key_env": null
          }
        }]
      }
    }
  },
  "retry": {
    "max_attempts": null,
    "upstream": null,
    "provider": null,
    "reasoning_guard": null
  }
}"#;
    write(&temp.0.join("config.json"), source);

    let plan = build_config_migration_plan(&temp.paths())
        .await
        .expect("historically nullable JSON should migrate");
    assert_eq!(plan.source_version, Some(1));
    let migrated = toml::from_str::<HelperConfig>(&plan.rendered)
        .expect("parse migrated historically nullable JSON");
    assert_eq!(migrated.version, CURRENT_CONFIG_VERSION);
    assert_eq!(migrated_provider_order(&plan), ["primary__u01"]);
}

#[tokio::test]
async fn migration_rejects_json_nulls_that_the_published_schema_rejected() {
    let cases = [
        r#"{"codex":null}"#,
        r#"{"codex":{"stations":{"primary":{"upstreams":[{"base_url":"https://relay.example/v1","auth":null}]}}}}"#,
        r#"{"codex":{"stations":{"primary":{"upstreams":[null]}}}}"#,
        r#"{"codex":{"stations":{"primary":{"upstreams":[{"base_url":"https://relay.example/v1","tags":{"region":null}}]}}}}"#,
        r#"{"version":5,"codex":{"providers":{"relay":{"base_url":"https://relay.example/v1","alias":null}}}}"#,
    ];

    for source in cases {
        let temp = TempConfigDir::new();
        let source_path = temp.0.join("config.json");
        write(&source_path, source);

        build_config_migration_plan(&temp.paths())
            .await
            .expect_err("non-nullable JSON field must fail before migration");
        assert_eq!(
            std::fs::read_to_string(&source_path).expect("read rejected JSON source"),
            source
        );
        assert!(!temp.0.join("config.json.bak").exists());
        assert!(!temp.0.join("config.toml").exists());
    }
}

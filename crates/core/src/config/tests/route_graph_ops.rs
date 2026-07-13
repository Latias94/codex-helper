use super::*;

#[test]
fn rename_route_node_updates_references_and_entry_name() {
    let mut routing = RouteGraphConfig {
        entry: "main".to_string(),
        routes: BTreeMap::from([
            (
                "main".to_string(),
                RouteNodeConfig {
                    strategy: RouteStrategy::OrderedFailover,
                    children: vec!["pool".to_string(), "paygo".to_string()],
                    ..RouteNodeConfig::default()
                },
            ),
            (
                "pool".to_string(),
                RouteNodeConfig {
                    strategy: RouteStrategy::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RouteNodeConfig::default()
                },
            ),
        ]),
        ..RouteGraphConfig::default()
    };

    routing
        .rename_route_node("pool", "monthly_pool".to_string())
        .expect("rename should succeed");

    assert_eq!(routing.entry, "main");
    assert!(routing.routes.contains_key("monthly_pool"));
    assert!(!routing.routes.contains_key("pool"));
    assert_eq!(
        routing.entry_node().map(|node| node.children.as_slice()),
        Some(&["monthly_pool".to_string(), "paygo".to_string()][..])
    );
}

#[test]
fn delete_route_node_rejects_entry_and_referenced_nodes() {
    let mut routing = RouteGraphConfig {
        entry: "main".to_string(),
        routes: BTreeMap::from([
            (
                "main".to_string(),
                RouteNodeConfig {
                    strategy: RouteStrategy::OrderedFailover,
                    children: vec!["pool".to_string()],
                    ..RouteNodeConfig::default()
                },
            ),
            (
                "pool".to_string(),
                RouteNodeConfig {
                    strategy: RouteStrategy::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RouteNodeConfig::default()
                },
            ),
            (
                "unused".to_string(),
                RouteNodeConfig {
                    strategy: RouteStrategy::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RouteNodeConfig::default()
                },
            ),
        ]),
        ..RouteGraphConfig::default()
    };

    let err = routing
        .delete_route_node("pool")
        .expect_err("referenced node should not delete");
    assert!(err.to_string().contains("referenced"));

    let err = routing
        .delete_route_node("main")
        .expect_err("entry node should not delete");
    assert!(err.to_string().contains("entry route node"));

    routing
        .delete_route_node("unused")
        .expect("unreferenced node should delete");
    assert!(!routing.routes.contains_key("unused"));
}

#[test]
fn entry_routing_authoring_updates_entry_node() {
    let mut routing = RouteGraphConfig::default();

    routing.set_entry_routing(
        RouteStrategy::ManualSticky,
        Some("monthly".to_string()),
        vec!["monthly".to_string(), "paygo".to_string()],
        Vec::new(),
        RouteExhaustedAction::Continue,
    );

    let entry = routing.entry_node().expect("entry route should exist");
    assert_eq!(entry.strategy, RouteStrategy::ManualSticky);
    assert_eq!(entry.target.as_deref(), Some("monthly"));
    assert_eq!(
        entry.children,
        vec!["monthly".to_string(), "paygo".to_string()]
    );
}

#[test]
fn provider_reference_authoring_updates_entry_node() {
    let mut routing = RouteGraphConfig::manual_sticky(
        "monthly".to_string(),
        vec!["monthly".to_string(), "paygo".to_string()],
    );

    assert!(routing.clear_manual_target_for("monthly"));
    routing.remove_provider_references("monthly");

    let entry = routing.entry_node().expect("entry route should exist");
    assert_eq!(entry.strategy, RouteStrategy::OrderedFailover);
    assert_eq!(entry.target, None);
    assert_eq!(entry.children, vec!["paygo".to_string()]);
}

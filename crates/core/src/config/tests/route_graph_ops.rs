use super::*;

#[test]
fn rename_route_node_updates_references_and_entry_name() {
    let mut routing = RoutingConfigV4 {
        entry: "main".to_string(),
        routes: BTreeMap::from([
            (
                "main".to_string(),
                RoutingNodeV4 {
                    strategy: RoutingPolicyV4::OrderedFailover,
                    children: vec!["pool".to_string(), "paygo".to_string()],
                    ..RoutingNodeV4::default()
                },
            ),
            (
                "pool".to_string(),
                RoutingNodeV4 {
                    strategy: RoutingPolicyV4::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RoutingNodeV4::default()
                },
            ),
        ]),
        ..RoutingConfigV4::default()
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
    let mut routing = RoutingConfigV4 {
        entry: "main".to_string(),
        routes: BTreeMap::from([
            (
                "main".to_string(),
                RoutingNodeV4 {
                    strategy: RoutingPolicyV4::OrderedFailover,
                    children: vec!["pool".to_string()],
                    ..RoutingNodeV4::default()
                },
            ),
            (
                "pool".to_string(),
                RoutingNodeV4 {
                    strategy: RoutingPolicyV4::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RoutingNodeV4::default()
                },
            ),
            (
                "unused".to_string(),
                RoutingNodeV4 {
                    strategy: RoutingPolicyV4::OrderedFailover,
                    children: vec!["alpha".to_string()],
                    ..RoutingNodeV4::default()
                },
            ),
        ]),
        ..RoutingConfigV4::default()
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
fn sync_graph_from_compat_ignores_default_compat_fields_for_existing_graph() {
    let mut routing = RoutingConfigV4 {
        entry: "main".to_string(),
        routes: BTreeMap::from([(
            "main".to_string(),
            RoutingNodeV4 {
                strategy: RoutingPolicyV4::TagPreferred,
                children: vec!["monthly".to_string(), "paygo".to_string()],
                prefer_tags: vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])],
                on_exhausted: RoutingExhaustedActionV4::Stop,
                ..RoutingNodeV4::default()
            },
        )]),
        ..RoutingConfigV4::default()
    };

    routing.sync_graph_from_compat();

    let entry = routing.entry_node().expect("entry node should remain");
    assert_eq!(entry.strategy, RoutingPolicyV4::TagPreferred);
    assert_eq!(
        entry.children,
        vec!["monthly".to_string(), "paygo".to_string()]
    );
    assert_eq!(entry.on_exhausted, RoutingExhaustedActionV4::Stop);
    assert_eq!(
        entry.prefer_tags,
        vec![BTreeMap::from([(
            "billing".to_string(),
            "monthly".to_string()
        )])]
    );
}

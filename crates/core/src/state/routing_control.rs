use std::collections::BTreeMap;

use crate::runtime_identity::ProviderEndpointKey;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoutingOperatorControlError {
    #[error("routing operator service name is empty")]
    EmptyServiceName,
    #[error("routing operator route graph key is empty")]
    EmptyRouteGraphKey,
    #[error("routing operator target belongs to service '{actual}', expected service '{expected}'")]
    ServiceMismatch { expected: String, actual: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionPreference {
    pub route_graph_key: String,
    pub target: ProviderEndpointKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutingOperatorServiceControl {
    route_graph_key: String,
    new_session_preference: Option<ProviderEndpointKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingOperatorControlSnapshot {
    revision: u64,
    services: BTreeMap<String, RoutingOperatorServiceControl>,
}

impl RoutingOperatorControlSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn route_graph_key(&self, service_name: &str) -> Option<&str> {
        self.services
            .get(service_name)
            .map(|control| control.route_graph_key.as_str())
    }

    pub fn new_session_preference(
        &self,
        service_name: &str,
        route_graph_key: &str,
    ) -> Option<&ProviderEndpointKey> {
        self.services.get(service_name).and_then(|control| {
            (control.route_graph_key == route_graph_key)
                .then_some(control.new_session_preference.as_ref())
                .flatten()
        })
    }

    pub fn configured_new_session_preference(
        &self,
        service_name: &str,
    ) -> Option<NewSessionPreference> {
        let control = self.services.get(service_name)?;
        Some(NewSessionPreference {
            route_graph_key: control.route_graph_key.clone(),
            target: control.new_session_preference.clone()?,
        })
    }

    pub(super) fn reconcile_route_graph(
        &mut self,
        service_name: &str,
        route_graph_key: &str,
    ) -> RoutingOperatorControlUpdate {
        if self
            .services
            .get(service_name)
            .is_some_and(|control| control.route_graph_key == route_graph_key)
        {
            return RoutingOperatorControlUpdate::Unchanged;
        }

        self.services.insert(
            service_name.to_string(),
            RoutingOperatorServiceControl {
                route_graph_key: route_graph_key.to_string(),
                new_session_preference: None,
            },
        );
        self.revision = self.revision.wrapping_add(1);
        RoutingOperatorControlUpdate::Applied
    }

    pub(super) fn apply_new_session_preference(
        &mut self,
        service_name: &str,
        route_graph_key: &str,
        target: Option<ProviderEndpointKey>,
    ) -> RoutingOperatorControlUpdate {
        let control = self
            .services
            .entry(service_name.to_string())
            .or_insert_with(|| RoutingOperatorServiceControl {
                route_graph_key: route_graph_key.to_string(),
                new_session_preference: None,
            });
        if control.route_graph_key != route_graph_key {
            return RoutingOperatorControlUpdate::Conflict;
        }
        if control.new_session_preference == target {
            return RoutingOperatorControlUpdate::Unchanged;
        }

        control.new_session_preference = target;
        self.revision = self.revision.wrapping_add(1);
        RoutingOperatorControlUpdate::Applied
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutingOperatorControlUpdate {
    Applied,
    Unchanged,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingOperatorControlCommit {
    pub status: RoutingOperatorControlUpdate,
    pub snapshot: RoutingOperatorControlSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_updates_are_revisioned_and_idempotent() {
        let mut snapshot = RoutingOperatorControlSnapshot::default();
        let target = ProviderEndpointKey::new("codex", "input", "default");

        assert_eq!(
            snapshot.apply_new_session_preference(
                "codex",
                "route:v1:initial",
                Some(target.clone())
            ),
            RoutingOperatorControlUpdate::Applied
        );
        assert_eq!(snapshot.revision(), 1);
        assert_eq!(
            snapshot.new_session_preference("codex", "route:v1:initial"),
            Some(&target)
        );

        assert_eq!(
            snapshot.apply_new_session_preference("codex", "route:v1:initial", Some(target)),
            RoutingOperatorControlUpdate::Unchanged
        );
        assert_eq!(snapshot.revision(), 1);

        assert_eq!(
            snapshot.apply_new_session_preference("codex", "route:v1:initial", None),
            RoutingOperatorControlUpdate::Applied
        );
        assert_eq!(snapshot.revision(), 2);
        assert!(
            snapshot
                .new_session_preference("codex", "route:v1:initial")
                .is_none()
        );
    }

    #[test]
    fn route_graph_reconciliation_clears_old_preference_permanently() {
        let mut snapshot = RoutingOperatorControlSnapshot::default();
        let target = ProviderEndpointKey::new("codex", "input", "default");
        assert_eq!(
            snapshot.apply_new_session_preference(
                "codex",
                "route:v1:initial",
                Some(target.clone())
            ),
            RoutingOperatorControlUpdate::Applied
        );

        assert_eq!(
            snapshot.reconcile_route_graph("codex", "route:v1:replacement"),
            RoutingOperatorControlUpdate::Applied
        );
        assert!(
            snapshot
                .new_session_preference("codex", "route:v1:initial")
                .is_none()
        );
        assert!(
            snapshot
                .new_session_preference("codex", "route:v1:replacement")
                .is_none()
        );

        assert_eq!(
            snapshot.reconcile_route_graph("codex", "route:v1:initial"),
            RoutingOperatorControlUpdate::Applied
        );
        assert!(
            snapshot
                .new_session_preference("codex", "route:v1:initial")
                .is_none()
        );
    }

    #[test]
    fn stale_topology_mutation_conflicts_without_changing_revision() {
        let mut snapshot = RoutingOperatorControlSnapshot::default();
        assert_eq!(
            snapshot.reconcile_route_graph("codex", "route:v1:current"),
            RoutingOperatorControlUpdate::Applied
        );
        let revision = snapshot.revision();

        assert_eq!(
            snapshot.apply_new_session_preference(
                "codex",
                "route:v1:stale",
                Some(ProviderEndpointKey::new("codex", "input", "default"))
            ),
            RoutingOperatorControlUpdate::Conflict
        );
        assert_eq!(snapshot.revision(), revision);
    }
}

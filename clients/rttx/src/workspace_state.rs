use crate::daemon_bridge::EndpointEvent;
use crate::runtime::{ConnectionStatus, RuntimeEndpoint, WorkspacePolicy, reconcile_bindings};
use crate::session::{LayoutNode, PaneRecovery, SessionState, SplitOrientation, WindowState};
use rttx_proto::proto;
use std::collections::{BTreeMap, BTreeSet};

/// Routing metadata for a daemon-managed terminal pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalBinding {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub runtime_pane_id: String,
}

/// Mapping from a daemon pane snapshot to a layout terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaneRestore {
    pub layout_terminal_uuid: String,
    pub title: String,
    pub cwd: String,
    pub scrollback: Vec<u8>,
}

/// Pure result of reconciling a workspace against a runtime snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorkspaceOpenResult {
    pub session_state: SessionState,
    pub panes_to_create: Vec<String>,
    pub snapshot_restores: Vec<WorkspacePaneRestore>,
    pub skipped_runtime_panes: Vec<String>,
}

/// Pure connection-status update derived from an endpoint event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatusUpdate {
    pub workspace_id: String,
    pub status: ConnectionStatus,
}

/// Pure request to rebuild one workspace from mutated state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorkspaceRebuild {
    pub workspace_id: String,
    pub session_state: SessionState,
}

/// Pure request to create a missing daemon pane for a layout terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPaneCreateRequest {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub layout_terminal_uuid: String,
}

/// Pure outcome of reconciling a daemon endpoint event against app state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EndpointEventTransition {
    pub recovered_workspaces: Vec<SessionState>,
    pub rebuilt_workspaces: Vec<ManagedWorkspaceRebuild>,
    pub pane_create_requests: Vec<ManagedPaneCreateRequest>,
    pub pane_snapshot_restores: Vec<WorkspacePaneRestore>,
    pub connected_layout_terminals: Vec<String>,
    pub layout_terminals_to_recover: Vec<String>,
    pub removed_layout_terminals: Vec<String>,
    pub connection_status_updates: Vec<ConnectionStatusUpdate>,
    pub skipped_runtime_panes: Vec<String>,
    pub persist_window_state: bool,
}

impl WindowState {
    /// True when startup should query an endpoint for inventory bootstrap.
    #[must_use]
    pub fn needs_inventory_bootstrap(&self, endpoint: &RuntimeEndpoint) -> bool {
        !self
            .sessions
            .iter()
            .any(|session| session.uses_managed_runtime() && &session.runtime.endpoint == endpoint)
    }

    /// Reconcile a daemon endpoint event into pure state updates plus follow-on effects.
    #[must_use]
    pub fn reconcile_endpoint_event(&mut self, event: &EndpointEvent) -> EndpointEventTransition {
        let mut transition = EndpointEventTransition::default();

        match event {
            EndpointEvent::WorkspaceConnectionChanged { workspace_id, status } => {
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: status.clone(),
                });
            }
            EndpointEvent::WorkspaceOpened { workspace_id, runtime_id, snapshot } => {
                let Some(opened) =
                    self.apply_managed_workspace_opened(workspace_id, runtime_id, snapshot)
                else {
                    return transition;
                };
                let ManagedWorkspaceOpenResult {
                    session_state,
                    panes_to_create,
                    snapshot_restores,
                    skipped_runtime_panes,
                } = opened;

                transition.rebuilt_workspaces.push(ManagedWorkspaceRebuild {
                    workspace_id: workspace_id.clone(),
                    session_state: session_state.clone(),
                });
                transition.skipped_runtime_panes = skipped_runtime_panes;
                transition.pane_snapshot_restores = snapshot_restores;

                for layout_terminal_uuid in panes_to_create {
                    transition.pane_create_requests.push(ManagedPaneCreateRequest {
                        workspace_id: workspace_id.clone(),
                        endpoint: session_state.runtime.endpoint.clone(),
                        runtime_id: runtime_id.clone(),
                        layout_terminal_uuid,
                    });
                }
            }
            EndpointEvent::PaneCreated {
                workspace_id,
                layout_terminal_uuid,
                runtime_id,
                runtime_pane_id,
            } => {
                let applied = self.apply_managed_pane_created(
                    workspace_id,
                    layout_terminal_uuid,
                    runtime_id,
                    runtime_pane_id,
                );
                if !applied {
                    return transition;
                }

                transition.connected_layout_terminals.push(layout_terminal_uuid.clone());
                transition.layout_terminals_to_recover.push(layout_terminal_uuid.clone());
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Connected,
                });
            }
            EndpointEvent::PaneClosed { workspace_id, layout_terminal_uuid, .. } => {
                let session_state =
                    self.apply_managed_pane_closed(workspace_id, layout_terminal_uuid);
                transition.removed_layout_terminals.push(layout_terminal_uuid.clone());
                if let Some(session_state) = session_state {
                    transition.rebuilt_workspaces.push(ManagedWorkspaceRebuild {
                        workspace_id: workspace_id.clone(),
                        session_state,
                    });
                }
            }
            EndpointEvent::WorkspaceDetached { workspace_id, .. } => {
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Disconnected,
                });
            }
            EndpointEvent::RuntimeTerminated { workspace_id, .. } => {
                if let Some(session) =
                    self.sessions.iter_mut().find(|session| session.uuid == *workspace_id)
                {
                    session.runtime.runtime_id = None;
                    session.sync_legacy_mode_from_runtime();
                }
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Disconnected,
                });
            }
            EndpointEvent::InventoryLoaded { endpoint, sessions } => {
                let recovered = self.recover_managed_workspaces_from_inventory(endpoint, sessions);
                if recovered.is_empty() {
                    return transition;
                }

                for session_state in &recovered {
                    transition.recovered_workspaces.push(session_state.clone());
                    transition.connection_status_updates.push(ConnectionStatusUpdate {
                        workspace_id: session_state.uuid.clone(),
                        status: ConnectionStatus::Connecting,
                    });
                }
                transition.persist_window_state = true;
            }
            EndpointEvent::RuntimeMessage { .. } | EndpointEvent::WorkspaceError { .. } => {}
        }

        transition
    }

    /// Recover daemon-managed workspaces that exist in inventory but not in the GUI state.
    pub fn recover_managed_workspaces_from_inventory(
        &mut self,
        endpoint: &RuntimeEndpoint,
        sessions: &[proto::SessionInfo],
    ) -> Vec<SessionState> {
        let mut known_runtime_ids = self
            .sessions
            .iter()
            .filter(|session| {
                session.uses_managed_runtime() && &session.runtime.endpoint == endpoint
            })
            .filter_map(|session| session.runtime.runtime_id.clone())
            .collect::<BTreeSet<_>>();
        let mut recovered = Vec::new();

        for session_info in sessions {
            let Some(session) = recovered_managed_workspace(endpoint, session_info) else {
                continue;
            };
            let Some(runtime_id) = session.runtime.runtime_id.clone() else {
                continue;
            };
            if !known_runtime_ids.insert(runtime_id) {
                continue;
            }
            self.sessions.push(session.clone());
            recovered.push(session);
        }

        recovered
    }

    /// Resolve a layout terminal to its managed runtime binding.
    #[must_use]
    pub fn managed_terminal_binding(&self, terminal_uuid: &str) -> Option<ManagedTerminalBinding> {
        let session = self.sessions.iter().find(|session| {
            session.uses_managed_runtime() && session.layout.contains_terminal(terminal_uuid)
        })?;
        let runtime_id = session.runtime.runtime_id.clone()?;
        let runtime_pane_id = session.runtime.pane_bindings.get(terminal_uuid)?.clone();
        if session.runtime.is_layout_pane_pending(terminal_uuid) {
            return None;
        }
        Some(ManagedTerminalBinding {
            workspace_id: session.uuid.clone(),
            endpoint: session.runtime.endpoint.clone(),
            runtime_id,
            runtime_pane_id,
        })
    }

    /// Resolve a runtime ID back to its workspace.
    #[must_use]
    pub fn workspace_for_runtime(
        &self,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
    ) -> Option<String> {
        self.sessions
            .iter()
            .find(|session| {
                session.uses_managed_runtime()
                    && &session.runtime.endpoint == endpoint
                    && session.runtime.runtime_id.as_deref() == Some(runtime_id)
            })
            .map(|session| session.uuid.clone())
    }

    /// Resolve a runtime pane back to its workspace/layout terminal.
    #[must_use]
    pub fn runtime_pane_target(
        &self,
        endpoint: &RuntimeEndpoint,
        runtime_pane_id: &str,
    ) -> Option<(String, String)> {
        let session = self.sessions.iter().find(|session| {
            session.uses_managed_runtime()
                && &session.runtime.endpoint == endpoint
                && session
                    .runtime
                    .pane_bindings
                    .values()
                    .any(|bound_runtime_pane_id| bound_runtime_pane_id == runtime_pane_id)
        })?;
        session
            .runtime
            .pane_bindings
            .iter()
            .find(|(_, bound_runtime_pane_id)| *bound_runtime_pane_id == runtime_pane_id)
            .map(|(layout_terminal_uuid, _)| (session.uuid.clone(), layout_terminal_uuid.clone()))
    }

    /// Apply the state mutation for a daemon pane-create acknowledgement.
    pub fn apply_managed_pane_created(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
        runtime_id: &str,
        runtime_pane_id: &str,
    ) -> bool {
        let Some(session) = self.sessions.iter_mut().find(|session| session.uuid == workspace_id)
        else {
            return false;
        };
        if !session.layout.contains_terminal(layout_terminal_uuid) {
            return false;
        }

        session.runtime.runtime_id = Some(runtime_id.to_string());
        session.runtime.bind_runtime_pane(layout_terminal_uuid, runtime_pane_id);
        session.sync_legacy_mode_from_runtime();
        true
    }

    /// Apply the state mutation for a daemon-acked managed pane close.
    pub fn apply_managed_pane_closed(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
    ) -> Option<SessionState> {
        let session = self.sessions.iter_mut().find(|session| session.uuid == workspace_id)?;
        session.runtime.pane_bindings.remove(layout_terminal_uuid);
        let new_layout = session.layout.remove_terminal(layout_terminal_uuid)?;
        session.layout = new_layout;
        let layout_terminal_uuids = session.layout.terminal_uuids();
        session.runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
        session.prune_recovery();
        session.normalize_active_terminal();
        Some(session.clone())
    }

    /// Apply the state mutation for attaching/reconciling a managed runtime snapshot.
    pub fn apply_managed_workspace_opened(
        &mut self,
        workspace_id: &str,
        runtime_id: &str,
        snapshot: &proto::Snapshot,
    ) -> Option<ManagedWorkspaceOpenResult> {
        let session = self.sessions.iter_mut().find(|session| session.uuid == workspace_id)?;

        let had_runtime_id = session.runtime.runtime_id.is_some();
        let had_only_placeholder_bindings =
            session.layout.terminal_uuids().iter().all(|layout_terminal_uuid| {
                session
                    .runtime
                    .pane_bindings
                    .get(layout_terminal_uuid)
                    .is_some_and(|runtime_pane_id| runtime_pane_id == layout_terminal_uuid)
            });
        session.runtime.runtime_id = Some(runtime_id.to_string());
        session.sync_legacy_mode_from_runtime();

        let layout_terminal_uuids = session.layout.terminal_uuids();
        let runtime_pane_uuids =
            snapshot.panes.iter().filter_map(snapshot_pane_id).collect::<Vec<_>>();
        let reconciliation = reconcile_bindings(
            &layout_terminal_uuids,
            &session.runtime.pane_bindings,
            &runtime_pane_uuids,
        );

        session.runtime.pane_bindings = reconciliation.bindings;
        session.runtime.pending_layout_panes =
            reconciliation.disconnected_layout_panes.iter().cloned().collect();

        let panes_to_create = if !had_runtime_id {
            let mut placeholders = reconciliation.disconnected_layout_panes.clone();
            if let Some(initial_terminal_uuid) = layout_terminal_uuids.first() {
                placeholders
                    .retain(|layout_terminal_uuid| layout_terminal_uuid != initial_terminal_uuid);
            }
            placeholders
        } else if snapshot.panes.is_empty() && had_only_placeholder_bindings {
            reconciliation.disconnected_layout_panes.clone()
        } else {
            Vec::new()
        };

        let mut skipped_runtime_panes = Vec::new();
        for runtime_pane_id in reconciliation.recovered_runtime_panes {
            let Some(anchor_uuid) = session.layout.terminal_uuids().last().cloned() else {
                skipped_runtime_panes.push(runtime_pane_id);
                continue;
            };
            let anchor_cwd = session.layout.terminal_cwd(&anchor_uuid);
            let Some((mut new_layout, new_terminal_uuid)) = session
                .layout
                .split_terminal_with_new_uuid(&anchor_uuid, SplitOrientation::Horizontal)
            else {
                skipped_runtime_panes.push(runtime_pane_id);
                continue;
            };

            if let Some(cwd) = anchor_cwd {
                new_layout.set_terminal_cwd(&new_terminal_uuid, Some(cwd));
            }
            session.layout = new_layout;
            session.set_recovery(&new_terminal_uuid, PaneRecovery::empty_shell());
            session.runtime.bind_runtime_pane(&new_terminal_uuid, &runtime_pane_id);
            session.normalize_active_terminal();
        }

        let layout_by_runtime_pane = session
            .runtime
            .pane_bindings
            .iter()
            .map(|(layout_terminal_uuid, runtime_pane_id)| {
                (runtime_pane_id.clone(), layout_terminal_uuid.clone())
            })
            .collect::<BTreeMap<_, _>>();
        let snapshot_restores = snapshot
            .panes
            .iter()
            .filter_map(|pane_snapshot| {
                let runtime_pane_id = snapshot_pane_id(pane_snapshot)?;
                let layout_terminal_uuid = layout_by_runtime_pane.get(&runtime_pane_id)?.clone();
                Some(WorkspacePaneRestore {
                    layout_terminal_uuid,
                    title: pane_snapshot.title.clone(),
                    cwd: pane_snapshot.cwd.clone(),
                    scrollback: pane_snapshot.scrollback.clone(),
                })
            })
            .collect::<Vec<_>>();

        Some(ManagedWorkspaceOpenResult {
            session_state: session.clone(),
            panes_to_create,
            snapshot_restores,
            skipped_runtime_panes,
        })
    }
}

fn recovered_managed_workspace(
    endpoint: &RuntimeEndpoint,
    session_info: &proto::SessionInfo,
) -> Option<SessionState> {
    let runtime_id = rttx_proto::bytes_to_uuid(&session_info.id).ok()?.to_string();
    let policy = match proto::RuntimePolicy::try_from(session_info.policy).ok() {
        Some(proto::RuntimePolicy::Ephemeral) => WorkspacePolicy::Ephemeral,
        _ => WorkspacePolicy::Persistent,
    };

    let mut session = SessionState::new_managed_local(session_info.name.clone(), policy, None);
    session.uuid = inventory_workspace_id(endpoint, &runtime_id);
    session.runtime.endpoint = endpoint.clone();
    session.runtime.runtime_id = Some(runtime_id);

    if !session_info.panes.is_empty() {
        session.layout = layout_from_inventory_panes(&session_info.panes);
        let pane_ids = session.layout.terminal_uuids();
        session.terminal_recovery = pane_ids
            .iter()
            .cloned()
            .map(|pane_id| (pane_id, PaneRecovery::empty_shell()))
            .collect();
        session.active_terminal_uuid = session_info
            .active_pane_id
            .as_ref()
            .and_then(|pane_id| rttx_proto::bytes_to_uuid(pane_id).ok().map(|id| id.to_string()))
            .or_else(|| pane_ids.first().cloned());
        session.runtime.pane_bindings =
            pane_ids.iter().cloned().map(|pane_id| (pane_id.clone(), pane_id)).collect();
        session.runtime.pending_layout_panes.clear();
    }

    session.sync_legacy_mode_from_runtime();
    Some(session)
}

fn inventory_workspace_id(endpoint: &RuntimeEndpoint, runtime_id: &str) -> String {
    format!("inventory:{}:{runtime_id}", endpoint.key())
}

fn layout_from_inventory_panes(panes: &[proto::PaneInfo]) -> LayoutNode {
    let mut panes = panes.iter();
    let Some(first_pane) = panes.next() else {
        return LayoutNode::new_terminal();
    };

    panes.fold(layout_terminal_from_inventory(first_pane), |layout, pane| LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(layout),
        second: Box::new(layout_terminal_from_inventory(pane)),
    })
}

fn layout_terminal_from_inventory(pane: &proto::PaneInfo) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: inventory_pane_id(pane).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        profile: None,
        cwd: (!pane.cwd.is_empty()).then(|| pane.cwd.clone()),
        custom_title: (!pane.title.is_empty()).then(|| pane.title.clone()),
    }
}

fn inventory_pane_id(pane: &proto::PaneInfo) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane.id).ok().map(|uuid| uuid.to_string())
}

fn snapshot_pane_id(pane_snapshot: &proto::PaneSnapshot) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane_snapshot.pane_id).ok().map(|uuid| uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_bridge::EndpointEvent;
    use crate::runtime::ConnectionStatus;
    use crate::runtime::WorkspacePolicy;
    use crate::test_helpers::{
        hsplit, managed_session, managed_session_with_runtime, session, term, term_full,
        window_state,
    };

    fn pane_snapshot(
        pane_id: &str,
        title: &str,
        cwd: &str,
        scrollback: &[u8],
    ) -> proto::PaneSnapshot {
        proto::PaneSnapshot {
            pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            scrollback: scrollback.to_vec(),
            exit_status: None,
        }
    }

    fn snapshot(runtime_id: &str, panes: Vec<proto::PaneSnapshot>) -> proto::Snapshot {
        proto::Snapshot {
            session_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            panes,
            revision: 7,
            current_client_role: rttx_proto::proto::RuntimeClientRole::Writer as i32,
        }
    }

    fn pane_info(pane_id: &str, title: &str, cwd: &str) -> proto::PaneInfo {
        proto::PaneInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            exit_status: None,
            reconstructed: true,
        }
    }

    fn session_info(
        runtime_id: &str,
        name: &str,
        policy: proto::RuntimePolicy,
        panes: Vec<proto::PaneInfo>,
        active_pane_id: Option<&str>,
    ) -> proto::SessionInfo {
        proto::SessionInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            name: name.to_string(),
            pane_count: panes.len() as u32,
            has_attached_client: false,
            active_pane_id: active_pane_id
                .map(|pane_id| rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap())),
            panes,
            policy: policy as i32,
            attached_client_count: 0,
            reconstructed: true,
            revision: 7,
            current_client_role: proto::RuntimeClientRole::Unattached as i32,
            has_write_owner: false,
            read_only_client_count: 0,
        }
    }

    #[test]
    fn managed_terminal_binding_ignores_placeholders_and_uses_explicit_runtime_bindings() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let runtime_pane_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        let mut state = window_state(vec![session.clone()]);

        assert!(
            state.managed_terminal_binding("pane-1").is_none(),
            "self-bindings stay unroutable until the daemon assigns a pane id",
        );

        session.runtime.bind_runtime_pane("pane-1", &runtime_pane_id);
        state.sessions[0] = session;

        let binding = state
            .managed_terminal_binding("pane-1")
            .expect("explicit daemon binding should resolve");
        assert_eq!(binding.workspace_id, "workspace-1");
        assert_eq!(binding.endpoint, RuntimeEndpoint::Local);
        assert_eq!(binding.runtime_id, runtime_id);
        assert_eq!(binding.runtime_pane_id, runtime_pane_id);
    }

    #[test]
    fn runtime_lookup_helpers_resolve_workspace_and_pane_targets() {
        let endpoint = RuntimeEndpoint::Remote { host: "builder.example".into() };
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("pane-1", "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26");
        let state = window_state(vec![session]);

        assert_eq!(
            state.workspace_for_runtime(&endpoint, "d7d04564-b2bf-4302-9495-e65c4df12ac6"),
            Some("workspace-1".into()),
        );
        assert_eq!(
            state.runtime_pane_target(&endpoint, "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            Some(("workspace-1".into(), "pane-1".into())),
        );
    }

    #[test]
    fn apply_managed_pane_created_binds_runtime_and_clears_pending_placeholder() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        assert!(state.apply_managed_pane_created(
            "workspace-1",
            "pane-1",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        ));

        let session = &state.sessions[0];
        assert_eq!(
            session.runtime.runtime_id.as_deref(),
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        assert_eq!(
            session.runtime.pane_bindings.get("pane-1").map(String::as_str),
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );
        assert!(!session.runtime.is_layout_pane_pending("pane-1"));
        assert_eq!(session.mode.daemon_session_id(), Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),);
    }

    #[test]
    fn apply_managed_pane_created_rejects_unknown_layout_terminal() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);
        let before = state.clone();

        assert!(!state.apply_managed_pane_created(
            "workspace-1",
            "missing-pane",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn apply_managed_pane_closed_prunes_state_and_preserves_remaining_binding() {
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("left", "07fa83b4-9ae3-4354-a1c5-1f685ffab370");
        session.runtime.bind_runtime_pane("right", "0d88f17f-626d-40b8-a1d3-6a42af628ac9");
        session.active_terminal_uuid = Some("left".into());
        let mut state = window_state(vec![session]);

        let updated = state
            .apply_managed_pane_closed("workspace-1", "left")
            .expect("removing one branch of a managed split should preserve the workspace");

        assert_eq!(updated.layout.terminal_uuids(), vec!["right".to_string()]);
        assert_eq!(updated.active_terminal_uuid.as_deref(), Some("right"));
        assert_eq!(
            updated.runtime.pane_bindings.get("right").map(String::as_str),
            Some("0d88f17f-626d-40b8-a1d3-6a42af628ac9"),
        );
        assert!(updated.recovery_for("left").is_none());
        assert!(updated.recovery_for("right").is_some());
    }

    #[test]
    fn apply_managed_workspace_opened_recovers_extra_runtime_panes_in_pure_state() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_runtime_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let second_runtime_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term_full("left", "/srv/project", "Left"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        session.runtime.bind_runtime_pane("left", first_runtime_pane);
        let mut state = window_state(vec![session]);
        let snapshot = snapshot(
            runtime_id,
            vec![
                pane_snapshot(first_runtime_pane, "Shell", "/srv/project", b"shell"),
                pane_snapshot(second_runtime_pane, "Logs", "/srv/project", b"logs"),
            ],
        );

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snapshot)
            .expect("managed workspace open should reconcile state");

        assert!(opened.panes_to_create.is_empty());
        assert!(opened.skipped_runtime_panes.is_empty());
        assert_eq!(opened.snapshot_restores.len(), 2);
        assert_eq!(opened.session_state.layout.terminal_count(), 2);

        let recovered_terminal_uuid = opened
            .session_state
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != "left")
            .expect("extra runtime pane should synthesize a recovered layout pane");
        assert_eq!(
            opened
                .session_state
                .runtime
                .pane_bindings
                .get(&recovered_terminal_uuid)
                .map(String::as_str),
            Some(second_runtime_pane),
        );
        assert_eq!(
            opened.session_state.layout.terminal_cwd(&recovered_terminal_uuid).as_deref(),
            Some("/srv/project"),
            "recovered pane should inherit the anchor cwd in pure state",
        );
        assert_eq!(
            opened
                .session_state
                .recovery_for(&recovered_terminal_uuid)
                .expect("recovered pane should have default recovery"),
            &PaneRecovery::empty_shell(),
        );
        assert!(opened.snapshot_restores.iter().any(|restore| {
            restore.layout_terminal_uuid == "left"
                && restore.title == "Shell"
                && restore.cwd == "/srv/project"
                && restore.scrollback == b"shell"
        }));
        assert!(opened.snapshot_restores.iter().any(|restore| {
            restore.layout_terminal_uuid == recovered_terminal_uuid
                && restore.title == "Logs"
                && restore.cwd == "/srv/project"
                && restore.scrollback == b"logs"
        }));
    }

    #[test]
    fn recover_managed_workspaces_from_inventory_creates_recoverable_layouts() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let second_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[session_info(
                runtime_id,
                "Recovered Workspace",
                proto::RuntimePolicy::Persistent,
                vec![
                    pane_info(first_pane, "Shell", "/srv/project"),
                    pane_info(second_pane, "Logs", "/srv/project"),
                ],
                Some(second_pane),
            )],
        );

        assert_eq!(recovered.len(), 1);
        assert_eq!(state.sessions.len(), 1);
        let session = &recovered[0];
        assert_eq!(session.uuid, "inventory:local:d7d04564-b2bf-4302-9495-e65c4df12ac6");
        assert_eq!(session.name, "Recovered Workspace");
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(session.runtime.runtime_id.as_deref(), Some(runtime_id));
        assert_eq!(session.active_terminal_uuid.as_deref(), Some(second_pane));
        assert_eq!(
            session.layout.terminal_uuids(),
            vec![first_pane.to_string(), second_pane.to_string()]
        );
        assert_eq!(
            session.runtime.pane_bindings.get(first_pane).map(String::as_str),
            Some(first_pane)
        );
        assert_eq!(
            session.runtime.pane_bindings.get(second_pane).map(String::as_str),
            Some(second_pane)
        );
        assert!(session.runtime.pending_layout_panes.is_empty());

        let snapshot = snapshot(
            runtime_id,
            vec![
                pane_snapshot(first_pane, "Shell", "/srv/project", b"shell"),
                pane_snapshot(second_pane, "Logs", "/srv/project", b"logs"),
            ],
        );
        let opened = state
            .apply_managed_workspace_opened(&session.uuid, runtime_id, &snapshot)
            .expect("inventory-recovered workspace should reattach cleanly");
        assert!(opened.panes_to_create.is_empty());
        assert!(opened.skipped_runtime_panes.is_empty());
        assert_eq!(opened.session_state.layout.terminal_uuids(), session.layout.terminal_uuids());
        assert_eq!(opened.snapshot_restores.len(), 2);
    }

    #[test]
    fn recover_managed_workspaces_from_inventory_skips_known_runtime_ids() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let existing = managed_session_with_runtime(
            "workspace-1",
            "Existing Workspace",
            term("pane-1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![existing]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[session_info(
                runtime_id,
                "Recovered Workspace",
                proto::RuntimePolicy::Persistent,
                vec![pane_info("07fa83b4-9ae3-4354-a1c5-1f685ffab370", "Shell", "/srv/project")],
                None,
            )],
        );

        assert!(recovered.is_empty());
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].uuid, "workspace-1");
    }

    #[test]
    fn apply_managed_workspace_opened_creates_initial_pane_for_empty_inventory_runtime() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[session_info(
                runtime_id,
                "Recovered Workspace",
                proto::RuntimePolicy::Persistent,
                vec![],
                None,
            )],
        );

        let session =
            recovered.first().expect("inventory should synthesize a placeholder workspace");
        let placeholder_uuid = session.layout.terminal_uuids()[0].clone();
        let opened = state
            .apply_managed_workspace_opened(
                &session.uuid,
                runtime_id,
                &snapshot(runtime_id, vec![]),
            )
            .expect("empty runtime should still reconcile");

        assert_eq!(opened.panes_to_create, vec![placeholder_uuid]);
        assert!(opened.snapshot_restores.is_empty());
    }

    #[test]
    fn reconcile_endpoint_event_status_change_emits_workspace_status_update() {
        let mut state = WindowState::default_for_test();

        let transition =
            state.reconcile_endpoint_event(&EndpointEvent::WorkspaceConnectionChanged {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
            });

        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
            }],
        );
    }

    #[test]
    fn reconcile_endpoint_event_inventory_loaded_recovers_workspace_and_persists() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            sessions: vec![session_info(
                &runtime_id,
                "Recovered Workspace",
                proto::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/srv/project")],
                Some(&pane_id),
            )],
        });

        assert_eq!(transition.recovered_workspaces.len(), 1);
        assert!(transition.persist_window_state);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: format!("inventory:local:{runtime_id}"),
                status: ConnectionStatus::Connecting,
            }],
        );
        assert!(
            state
                .sessions
                .iter()
                .any(|session| session.uuid == format!("inventory:local:{runtime_id}"))
        );
    }

    #[test]
    fn needs_inventory_bootstrap_when_no_managed_workspace_uses_endpoint() {
        let state = window_state(vec![session("session-1", "Session 1", term("pane-1"))]);

        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::Remote {
            host: "builder.example".into(),
        }));
    }

    #[test]
    fn needs_inventory_bootstrap_skips_endpoints_already_present_in_state() {
        let state = window_state(vec![
            managed_session_with_runtime(
                "workspace-1",
                "Local Workspace",
                term("local-pane"),
                RuntimeEndpoint::Local,
                WorkspacePolicy::Persistent,
                Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
            ),
            managed_session_with_runtime(
                "workspace-2",
                "Remote Workspace",
                term("remote-pane"),
                RuntimeEndpoint::Remote { host: "builder.example".into() },
                WorkspacePolicy::Persistent,
                Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            ),
        ]);

        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::Remote {
            host: "builder.example".into(),
        }));
        assert!(
            state.needs_inventory_bootstrap(&RuntimeEndpoint::Remote {
                host: "other.example".into(),
            })
        );
    }

    #[test]
    fn reconcile_endpoint_event_workspace_opened_rebuilds_restores_and_requests_missing_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let first_terminal_uuid = uuid::Uuid::new_v4().to_string();
        let second_terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session(
            "workspace-1",
            "Workspace",
            hsplit(term(&first_terminal_uuid), term(&second_terminal_uuid)),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(
                    &first_terminal_uuid,
                    "Shell",
                    "/srv/project",
                    b"restored output",
                )],
            ),
        });

        assert_eq!(
            transition.rebuilt_workspaces,
            vec![ManagedWorkspaceRebuild {
                workspace_id: "workspace-1".into(),
                session_state: state.sessions[0].clone(),
            }],
        );
        assert_eq!(
            transition.pane_create_requests,
            vec![ManagedPaneCreateRequest {
                workspace_id: "workspace-1".into(),
                endpoint: RuntimeEndpoint::Local,
                runtime_id: runtime_id.clone(),
                layout_terminal_uuid: second_terminal_uuid,
            }],
        );
        assert_eq!(
            transition.pane_snapshot_restores,
            vec![WorkspacePaneRestore {
                layout_terminal_uuid: first_terminal_uuid,
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                scrollback: b"restored output".to_vec(),
            }],
        );
        assert_eq!(state.sessions[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    }

    #[test]
    fn reconcile_endpoint_event_pane_created_updates_binding_and_requests_recovery() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneCreated {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: "pane-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".into(),
        });

        assert_eq!(transition.connected_layout_terminals, vec!["pane-1".to_string()]);
        assert_eq!(transition.layout_terminals_to_recover, vec!["pane-1".to_string()]);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Connected,
            }],
        );
        assert_eq!(
            state.sessions[0].runtime.pane_bindings.get("pane-1").map(String::as_str),
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );
    }

    #[test]
    fn reconcile_endpoint_event_pane_closed_removes_terminal_and_rebuilds_workspace() {
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("left", "07fa83b4-9ae3-4354-a1c5-1f685ffab370");
        session.runtime.bind_runtime_pane("right", "0d88f17f-626d-40b8-a1d3-6a42af628ac9");
        let mut state = window_state(vec![session]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneClosed {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: "right".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: "0d88f17f-626d-40b8-a1d3-6a42af628ac9".into(),
        });

        assert_eq!(transition.removed_layout_terminals, vec!["right".to_string()]);
        assert_eq!(transition.rebuilt_workspaces.len(), 1);
        assert_eq!(state.sessions[0].layout.terminal_uuids(), vec!["left".to_string()]);
    }

    #[test]
    fn reconcile_endpoint_event_workspace_detached_preserves_runtime_id_and_marks_disconnected() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::Remote { host: "builder.example".into() },
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceDetached {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
        });

        assert_eq!(state.sessions[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Disconnected,
            }],
        );
    }

    #[test]
    fn reconcile_endpoint_event_runtime_terminated_clears_runtime_id_and_marks_disconnected() {
        let mut state = window_state(vec![managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::Remote { host: "builder.example".into() },
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::RuntimeTerminated {
            workspace_id: "workspace-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            reason: proto::RuntimeTerminationReason::Explicit,
        });

        assert_eq!(state.sessions[0].runtime.runtime_id, None);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Disconnected,
            }],
        );
    }
}

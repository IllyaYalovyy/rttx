use crate::daemon_bridge::EndpointEvent;
use crate::runtime::{ConnectionStatus, RuntimeEndpoint, WorkspacePolicy};
use crate::workspace::{
    LayoutNode, PaneRecovery, SplitOrientation, WindowState, WorkspaceState, layout_from_pane_tree,
};
use rttx_proto::v3;
use std::collections::BTreeSet;

/// Routing metadata for a daemon-managed terminal pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalBinding {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub runtime_pane_id: String,
}

/// Mapping from a daemon pane snapshot to a layout terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspacePaneRestore {
    pub layout_terminal_uuid: String,
    pub title: String,
    pub cwd: String,
    pub pane_output_seq: u64,
    pub scrollback_tail: bytes::Bytes,
    pub scrollback_complete: bool,
    pub cols: u16,
    pub rows: u16,
    pub terminal_modes: Option<v3::TerminalModeState>,
}

/// Pure result of reconciling a workspace against a runtime snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorkspaceOpenResult {
    pub session_state: WorkspaceState,
    pub panes_to_create: Vec<String>,
    pub snapshot_restores: Vec<WorkspacePaneRestore>,
    pub skipped_runtime_panes: Vec<String>,
    pub previous_layout_terminals: Vec<String>,
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
    pub session_state: WorkspaceState,
}

/// Records a layout-terminal re-key so the window layer can rename the widget
/// maps keyed by uuid when a client-minted uuid becomes its server pane id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRekey {
    pub workspace_id: String,
    pub old_uuid: String,
    pub new_uuid: String,
}

/// Pure request to create a missing daemon pane for a layout terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPaneCreateRequest {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub layout_terminal_uuid: String,
    pub cwd: Option<String>,
}

/// Pure outcome of reconciling a daemon endpoint event against app state.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EndpointEventTransition {
    pub recovered_workspaces: Vec<WorkspaceState>,
    pub rebuilt_workspaces: Vec<ManagedWorkspaceRebuild>,
    pub pane_create_requests: Vec<ManagedPaneCreateRequest>,
    pub pane_snapshot_restores: Vec<WorkspacePaneRestore>,
    pub connected_layout_terminals: Vec<String>,
    pub layout_terminals_to_recover: Vec<String>,
    pub removed_layout_terminals: Vec<String>,
    /// Layout-terminal re-keys (client uuid -> server pane id) that the window
    /// layer must apply to its widget maps.
    pub pane_rekeys: Vec<PaneRekey>,
    pub connection_status_updates: Vec<ConnectionStatusUpdate>,
    pub skipped_runtime_panes: Vec<String>,
    pub persist_window_state: bool,
}

impl WindowState {
    /// Record a runtime ID as dismissed so inventory refresh won't resurrect it.
    pub fn dismiss_runtime(&mut self, _endpoint: &RuntimeEndpoint, runtime_id: &str) {
        self.dismissed_runtime_ids.insert(runtime_id.to_string());
    }

    /// True when startup should query an endpoint for inventory bootstrap.
    #[must_use]
    pub fn needs_inventory_bootstrap(&self, endpoint: &RuntimeEndpoint) -> bool {
        !self
            .workspaces
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
                    previous_layout_terminals,
                } = opened;

                let new_terminal_set: BTreeSet<_> =
                    session_state.layout.terminal_uuids().into_iter().collect();
                for old_uuid in &previous_layout_terminals {
                    if !new_terminal_set.contains(old_uuid) {
                        transition.removed_layout_terminals.push(old_uuid.clone());
                    }
                }

                transition.rebuilt_workspaces.push(ManagedWorkspaceRebuild {
                    workspace_id: workspace_id.clone(),
                    session_state: session_state.clone(),
                });
                transition.skipped_runtime_panes = skipped_runtime_panes;
                transition.pane_snapshot_restores = snapshot_restores;

                for layout_terminal_uuid in panes_to_create {
                    let cwd = session_state.layout.terminal_cwd(&layout_terminal_uuid);
                    transition.pane_create_requests.push(ManagedPaneCreateRequest {
                        workspace_id: workspace_id.clone(),
                        endpoint: session_state.runtime.endpoint.clone(),
                        runtime_id: runtime_id.clone(),
                        layout_terminal_uuid,
                        cwd,
                    });
                }
            }
            EndpointEvent::PaneCreated {
                workspace_id,
                layout_terminal_uuid,
                runtime_id,
                runtime_pane_id,
            } => {
                let Some(rekey) = self.apply_managed_pane_created(
                    workspace_id,
                    layout_terminal_uuid,
                    runtime_id,
                    runtime_pane_id,
                ) else {
                    return transition;
                };

                // After the re-key the layout terminal *is* the server pane id,
                // so downstream recovery/connect steps key on the new uuid.
                let new_uuid = rekey.new_uuid.clone();
                if rekey.old_uuid != rekey.new_uuid {
                    transition.pane_rekeys.push(rekey);
                }
                transition.connected_layout_terminals.push(new_uuid.clone());
                transition.layout_terminals_to_recover.push(new_uuid);
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
            EndpointEvent::WorkspaceTerminated { workspace_id, .. } => {
                if let Some(session) =
                    self.workspaces.iter_mut().find(|session| session.uuid == *workspace_id)
                {
                    session.runtime.runtime_id = None;
                }
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Disconnected,
                });
            }
            EndpointEvent::InventoryLoaded { endpoint, workspaces } => {
                let inventory_ids: std::collections::BTreeSet<String> = workspaces
                    .iter()
                    .filter_map(|s| rttx_proto::bytes_to_uuid(&s.id).ok().map(|u| u.to_string()))
                    .collect();
                self.dismissed_runtime_ids.retain(|id| inventory_ids.contains(id));

                let recovered =
                    self.recover_managed_workspaces_from_inventory(endpoint, workspaces);
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
            EndpointEvent::WorkspaceResynced { workspace_id, snapshot, .. } => {
                let restores = self.build_resync_restores(workspace_id, snapshot);
                transition.pane_snapshot_restores = restores;
            }
            EndpointEvent::WorkspaceMessage { .. } | EndpointEvent::WorkspaceError { .. } => {}
        }

        transition
    }

    /// Recover daemon-managed workspaces that exist in inventory but not in the GUI state.
    pub fn recover_managed_workspaces_from_inventory(
        &mut self,
        endpoint: &RuntimeEndpoint,
        workspaces: &[v3::WorkspaceInfo],
    ) -> Vec<WorkspaceState> {
        let mut known_runtime_ids = self
            .workspaces
            .iter()
            .filter(|session| {
                session.uses_managed_runtime() && &session.runtime.endpoint == endpoint
            })
            .filter_map(|session| session.runtime.runtime_id.clone())
            .collect::<BTreeSet<_>>();
        let mut recovered = Vec::new();

        for rt_info in workspaces {
            let Some(session) = recovered_managed_workspace(endpoint, rt_info) else {
                continue;
            };
            let Some(runtime_id) = session.runtime.runtime_id.clone() else {
                continue;
            };
            if self.dismissed_runtime_ids.contains(&runtime_id) {
                continue;
            }
            if !known_runtime_ids.insert(runtime_id) {
                continue;
            }
            self.workspaces.push(session.clone());
            recovered.push(session);
        }

        if !recovered.is_empty() {
            self.rebuild_pane_reverse_index();
        }

        recovered
    }

    /// Resolve a layout terminal to its managed runtime binding.
    ///
    /// Under the identity invariant the runtime pane id *is* the layout
    /// terminal uuid. Returns `None` when the workspace is unmanaged, lacks a
    /// live runtime, or does not contain the terminal.
    #[must_use]
    pub fn managed_terminal_binding(&self, terminal_uuid: &str) -> Option<ManagedTerminalBinding> {
        let session = self.workspaces.iter().find(|session| {
            session.uses_managed_runtime() && session.layout.contains_terminal(terminal_uuid)
        })?;
        let runtime_id = session.runtime.runtime_id.clone()?;
        Some(ManagedTerminalBinding {
            workspace_id: session.uuid.clone(),
            endpoint: session.runtime.endpoint.clone(),
            runtime_id,
            runtime_pane_id: terminal_uuid.to_string(),
        })
    }

    /// Return managed bindings for all sibling panes when input sync is active.
    ///
    /// Returns an empty vec when input sync is off, the workspace is not
    /// managed, or the source terminal has no routable binding.
    #[must_use]
    pub fn input_sync_targets(&self, source_uuid: &str) -> Vec<ManagedTerminalBinding> {
        let session = self.workspaces.iter().find(|s| {
            s.input_sync && s.uses_managed_runtime() && s.layout.contains_terminal(source_uuid)
        });
        let Some(session) = session else {
            return Vec::new();
        };
        let Some(runtime_id) = session.runtime.runtime_id.as_deref() else {
            return Vec::new();
        };
        session
            .layout
            .terminal_uuids()
            .into_iter()
            .filter(|uuid| uuid != source_uuid)
            .map(|uuid| ManagedTerminalBinding {
                workspace_id: session.uuid.clone(),
                endpoint: session.runtime.endpoint.clone(),
                runtime_id: runtime_id.to_string(),
                runtime_pane_id: uuid,
            })
            .collect()
    }

    /// Resolve a runtime ID back to its workspace.
    #[must_use]
    pub fn workspace_for_runtime(
        &self,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
    ) -> Option<String> {
        self.workspaces
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
        let key = Self::pane_index_key(&endpoint.key(), runtime_pane_id);
        self.pane_reverse_index.get(&key).cloned()
    }

    /// Apply the state mutation for a daemon pane-create acknowledgement.
    ///
    /// Re-keys the layout terminal to the durable server pane id so the
    /// identity invariant (uuid == pane id) holds. Returns the [`PaneRekey`] so
    /// the window layer can rename its widget maps, or `None` when the
    /// workspace or layout terminal is unknown.
    pub fn apply_managed_pane_created(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
        runtime_id: &str,
        runtime_pane_id: &str,
    ) -> Option<PaneRekey> {
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;
        if !session.layout.contains_terminal(layout_terminal_uuid) {
            return None;
        }

        session.runtime.runtime_id = Some(runtime_id.to_string());
        if layout_terminal_uuid != runtime_pane_id {
            // Re-key layout, recovery and active-terminal state onto the
            // server pane id (identity invariant).
            session.replace_terminal_uuid(layout_terminal_uuid, runtime_pane_id);
        }
        self.rebuild_pane_reverse_index();
        Some(PaneRekey {
            workspace_id: workspace_id.to_string(),
            old_uuid: layout_terminal_uuid.to_string(),
            new_uuid: runtime_pane_id.to_string(),
        })
    }

    /// Apply the state mutation for a daemon-acked managed pane close.
    pub fn apply_managed_pane_closed(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
    ) -> Option<WorkspaceState> {
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;
        let new_layout = session.layout.remove_terminal(layout_terminal_uuid)?;
        session.layout = new_layout;
        session.prune_recovery();
        session.normalize_active_terminal();
        let result = session.clone();
        self.rebuild_pane_reverse_index();
        Some(result)
    }

    /// Adopt the daemon's authoritative pane tree as the client's render
    /// layout (RFC-031 §3, Step 4). The render leaf uuids are the durable
    /// server pane ids, so the layout and the daemon share one identity and the
    /// client neither mints structure nor creates panes to match the daemon.
    fn adopt_server_tree(
        &mut self,
        workspace_id: &str,
        runtime_id: &str,
        layout: LayoutNode,
        snapshot: &v3::WorkspaceSnapshot,
    ) -> Option<ManagedWorkspaceOpenResult> {
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;
        session.runtime.runtime_id = Some(runtime_id.to_string());

        let previous_layout_terminals = session.layout.terminal_uuids();
        session.layout = layout;
        let layout_terminal_uuids = session.layout.terminal_uuids();

        // The server names the fallback focus pane (RFC-031 §2).
        if let Some(active) = rttx_proto::bytes_to_uuid(&snapshot.default_active_pane_id)
            .ok()
            .map(|uuid| uuid.to_string())
            .filter(|id| layout_terminal_uuids.contains(id))
        {
            session.active_terminal_uuid = Some(active);
        }
        session.normalize_active_terminal();

        // Pane content restores map 1:1 to layout terminals by pane id.
        let snapshot_restores = snapshot
            .panes
            .iter()
            .filter_map(|pane_snapshot| {
                let pane_id = snapshot_pane_id(pane_snapshot)?;
                layout_terminal_uuids.contains(&pane_id).then(|| WorkspacePaneRestore {
                    layout_terminal_uuid: pane_id,
                    title: pane_snapshot.title.clone(),
                    cwd: pane_snapshot.cwd.clone(),
                    pane_output_seq: pane_snapshot.pane_output_seq,
                    scrollback_tail: pane_snapshot.scrollback_tail.clone(),
                    scrollback_complete: pane_snapshot.scrollback_complete,
                    cols: pane_snapshot.cols as u16,
                    rows: pane_snapshot.rows as u16,
                    terminal_modes: pane_snapshot.terminal_modes,
                })
            })
            .collect::<Vec<_>>();

        // Carry the daemon's current CWD into the adopted layout.
        for restore in &snapshot_restores {
            if !restore.cwd.is_empty() {
                session
                    .layout
                    .set_terminal_cwd(&restore.layout_terminal_uuid, Some(restore.cwd.clone()));
            }
        }

        let session_state = session.clone();
        self.rebuild_pane_reverse_index();

        Some(ManagedWorkspaceOpenResult {
            session_state,
            panes_to_create: Vec::new(),
            snapshot_restores,
            skipped_runtime_panes: Vec::new(),
            previous_layout_terminals,
        })
    }

    /// Apply the state mutation for attaching/reconciling a managed runtime snapshot.
    pub fn apply_managed_workspace_opened(
        &mut self,
        workspace_id: &str,
        runtime_id: &str,
        snapshot: &v3::WorkspaceSnapshot,
    ) -> Option<ManagedWorkspaceOpenResult> {
        // RFC-031 §3 (Step 4): when the daemon provides its authoritative pane
        // tree, the client adopts it wholesale and renders as a pure view. The
        // tree's leaf uuids are the durable server pane ids, so layout terminal
        // uuid == pane id with no client-side binding translation, and the
        // client never mints structure nor creates panes to "match" the daemon.
        if let Some(layout) = render_layout_from_snapshot(snapshot) {
            return self.adopt_server_tree(workspace_id, runtime_id, layout, snapshot);
        }

        // Tree-less snapshot: an empty workspace whose daemon runtime has no
        // panes yet. There is nothing to
        // adopt — keep the session's existing placeholder layout and record the
        // runtime id. The daemon-bridge CreatePane bootstrap creates the first
        // pane and the subsequent PaneCreated re-key assigns identity.
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;
        session.runtime.runtime_id = Some(runtime_id.to_string());

        let layout_terminal_uuids = session.layout.terminal_uuids();
        let session_state = session.clone();
        self.rebuild_pane_reverse_index();

        Some(ManagedWorkspaceOpenResult {
            session_state,
            panes_to_create: Vec::new(),
            snapshot_restores: Vec::new(),
            skipped_runtime_panes: Vec::new(),
            previous_layout_terminals: layout_terminal_uuids,
        })
    }

    /// Build snapshot restores for a resync without rebuilding the layout.
    fn build_resync_restores(
        &self,
        workspace_id: &str,
        snapshot: &v3::WorkspaceSnapshot,
    ) -> Vec<WorkspacePaneRestore> {
        let Some(session) = self.workspaces.iter().find(|s| s.uuid == workspace_id) else {
            return Vec::new();
        };
        snapshot
            .panes
            .iter()
            .filter_map(|pane_snapshot| {
                // Identity invariant: the snapshot pane id *is* the layout
                // terminal uuid. Only restore panes the layout still contains.
                let layout_terminal_uuid = snapshot_pane_id(pane_snapshot)?;
                if !session.layout.contains_terminal(&layout_terminal_uuid) {
                    return None;
                }
                Some(WorkspacePaneRestore {
                    layout_terminal_uuid,
                    title: pane_snapshot.title.clone(),
                    cwd: pane_snapshot.cwd.clone(),
                    pane_output_seq: pane_snapshot.pane_output_seq,
                    scrollback_tail: pane_snapshot.scrollback_tail.clone(),
                    scrollback_complete: pane_snapshot.scrollback_complete,
                    cols: pane_snapshot.cols as u16,
                    rows: pane_snapshot.rows as u16,
                    terminal_modes: pane_snapshot.terminal_modes,
                })
            })
            .collect()
    }
}

fn recovered_managed_workspace(
    endpoint: &RuntimeEndpoint,
    rt_info: &v3::WorkspaceInfo,
) -> Option<WorkspaceState> {
    let runtime_id = rttx_proto::bytes_to_uuid(&rt_info.id).ok()?.to_string();
    let policy = match v3::WorkspacePolicy::try_from(rt_info.policy).ok() {
        Some(v3::WorkspacePolicy::Ephemeral) => WorkspacePolicy::Ephemeral,
        _ => WorkspacePolicy::Persistent,
    };

    let mut session = WorkspaceState::new_managed_local(rt_info.name.clone(), policy, None);
    session.uuid = inventory_workspace_id(endpoint, &runtime_id);
    session.runtime.endpoint = endpoint.clone();
    session.runtime.runtime_id = Some(runtime_id);

    if !rt_info.panes.is_empty() {
        session.layout = layout_from_inventory_panes(&rt_info.panes);
        let pane_ids = session.layout.terminal_uuids();
        session.terminal_recovery = pane_ids
            .iter()
            .cloned()
            .map(|pane_id| (pane_id, PaneRecovery::empty_shell()))
            .collect();
        session.active_terminal_uuid = pane_ids.first().cloned();
    }

    Some(session)
}

fn inventory_workspace_id(endpoint: &RuntimeEndpoint, runtime_id: &str) -> String {
    format!("inventory:{}:{runtime_id}", endpoint.key())
}

fn layout_from_inventory_panes(panes: &[v3::PaneInfo]) -> LayoutNode {
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

fn layout_terminal_from_inventory(pane: &v3::PaneInfo) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: inventory_pane_id(pane).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        profile: None,
        cwd: (!pane.cwd.is_empty()).then(|| pane.cwd.clone()),
        custom_title: (!pane.title.is_empty()).then(|| pane.title.clone()),
    }
}

fn inventory_pane_id(pane: &v3::PaneInfo) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane.id).ok().map(|uuid| uuid.to_string())
}

fn snapshot_pane_id(pane_snapshot: &v3::PaneSnapshot) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane_snapshot.pane_id).ok().map(|uuid| uuid.to_string())
}

/// Build the client render layout from a workspace snapshot's authoritative
/// server tree (RFC-031 §3, Step 4).
///
/// Returns `None` when the snapshot carries no tree (an empty workspace whose
/// daemon runtime has no panes yet); the caller then keeps its placeholder
/// layout until the daemon mints the first pane. The returned [`LayoutNode`]
/// keys every leaf by the durable server pane id, so the render tree and the
/// daemon share one
/// identity with no client-side binding indirection.
#[must_use]
pub fn render_layout_from_snapshot(snapshot: &v3::WorkspaceSnapshot) -> Option<LayoutNode> {
    snapshot.tree.as_ref().and_then(layout_from_pane_tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_bridge::EndpointEvent;
    use crate::runtime::ConnectionStatus;
    use crate::runtime::WorkspacePolicy;
    use crate::test_helpers::{
        hsplit, managed_session, managed_session_with_runtime, term, window_state, workspace,
    };

    fn pane_snapshot(pane_id: &str, title: &str, cwd: &str, scrollback: &[u8]) -> v3::PaneSnapshot {
        v3::PaneSnapshot {
            pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            pane_output_seq: 0,
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            exit_status: None,
            terminal_modes: None,
            scrollback_tail: bytes::Bytes::copy_from_slice(scrollback),
            total_scrollback_bytes: scrollback.len() as u64,
            scrollback_complete: true,
        }
    }

    fn snapshot(runtime_id: &str, panes: Vec<v3::PaneSnapshot>) -> v3::WorkspaceSnapshot {
        v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            panes,
            workspace_revision: 7,
            client_role: v3::WorkspaceClientRole::Writer as i32,
        }
    }

    /// A single-pane snapshot carrying the daemon's authoritative single-leaf
    /// tree, so the client adopts a layout whose terminal uuid equals the
    /// server pane id (identity invariant).
    fn single_leaf_snapshot(
        runtime_id: &str,
        pane_id: &str,
        pane: v3::PaneSnapshot,
    ) -> v3::WorkspaceSnapshot {
        let mut snap = snapshot(runtime_id, vec![pane]);
        snap.tree =
            Some(rttx_proto::v3_tree::pane_tree_leaf(uuid::Uuid::parse_str(pane_id).unwrap()));
        snap
    }

    #[test]
    fn render_layout_from_snapshot_returns_none_without_tree() {
        let snap = snapshot("11111111-1111-1111-1111-111111111111", Vec::new());
        assert!(render_layout_from_snapshot(&snap).is_none());
    }

    #[test]
    fn render_layout_from_snapshot_builds_split_keyed_by_server_pane_ids() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        let left = uuid::Uuid::new_v4();
        let right = uuid::Uuid::new_v4();
        let mut snap = snapshot("22222222-2222-2222-2222-222222222222", Vec::new());
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(left),
            pane_tree_leaf(right),
        ));

        let layout = render_layout_from_snapshot(&snap).expect("snapshot with tree renders");
        let LayoutNode::Split { orientation, first, second, .. } = layout else {
            panic!("expected a split");
        };
        assert_eq!(orientation, SplitOrientation::Horizontal);
        assert_eq!(first.terminal_uuids(), vec![left.to_string()]);
        assert_eq!(second.terminal_uuids(), vec![right.to_string()]);
    }

    #[test]
    fn apply_managed_workspace_opened_adopts_server_tree_as_pure_view() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        // The client starts with a stale single-pane layout that does not match
        // the server tree.
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("stale-old-pane"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);

        // Server tree: a horizontal split of two server-minted pane ids.
        let left = uuid::Uuid::new_v4();
        let right = uuid::Uuid::new_v4();
        let mut snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(&left.to_string(), "left", "/work/left", b"L"),
                pane_snapshot(&right.to_string(), "right", "/work/right", b"R"),
            ],
        );
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(left),
            pane_tree_leaf(right),
        ));
        snap.default_active_pane_id = rttx_proto::uuid_to_bytes(right);

        let result = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
            .expect("adopts the server tree");

        // The client discards its stale layout and renders the server tree.
        let uuids = result.session_state.layout.terminal_uuids();
        assert_eq!(uuids.len(), 2, "layout is rebuilt from the two server panes");
        assert!(uuids.contains(&left.to_string()), "left server pane is rendered");
        assert!(uuids.contains(&right.to_string()), "right server pane is rendered");
        assert!(
            !uuids.contains(&"stale-old-pane".to_string()),
            "the stale client-owned pane is discarded",
        );

        // A pure view never mints structure or creates panes to match the daemon.
        assert!(result.panes_to_create.is_empty(), "pure view creates no panes");
        assert!(result.skipped_runtime_panes.is_empty());
        assert!(
            result.previous_layout_terminals.contains(&"stale-old-pane".to_string()),
            "the discarded layout terminal is reported so its widget is torn down",
        );

        // Restores map 1:1 to layout terminals by pane id (identity).
        assert_eq!(result.snapshot_restores.len(), 2);
        assert!(result.snapshot_restores.iter().all(|r| {
            r.layout_terminal_uuid == left.to_string()
                || r.layout_terminal_uuid == right.to_string()
        }));

        let session = &state.workspaces[0];
        // Layout terminals ARE the server pane ids (identity invariant).
        assert!(session.layout.contains_terminal(&left.to_string()));
        assert!(session.layout.contains_terminal(&right.to_string()));
        // Active pane follows the server's fallback focus.
        assert_eq!(session.active_terminal_uuid.as_deref(), Some(right.to_string().as_str()));
    }

    #[test]
    fn adopted_pane_close_targets_the_daemon_not_local_only() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        // Regression for the close/reconnect drift: after adopting the server
        // tree, layout uuids equal their server pane ids (identity bindings).
        // Closing such a pane must still be sent to the daemon, otherwise the
        // daemon keeps the pane and it resurrects on the next restart.
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            term("stale"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);

        let left = uuid::Uuid::new_v4();
        let right = uuid::Uuid::new_v4();
        let mut snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(&left.to_string(), "l", "", b""),
                pane_snapshot(&right.to_string(), "r", "", b""),
            ],
        );
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(left),
            pane_tree_leaf(right),
        ));
        state.apply_managed_workspace_opened("ws-1", runtime_id, &snap).expect("adopts tree");

        let session = &state.workspaces[0];
        assert_eq!(
            session.managed_close_target(&left.to_string()),
            Some((runtime_id.to_string(), left.to_string())),
            "an adopted (identity-bound) pane must close on the daemon",
        );
    }

    #[test]
    fn pending_or_single_pane_close_stays_local() {
        // A fresh managed workspace has placeholder (pending) bindings and no
        // daemon runtime id yet: closes stay client-local.
        let state =
            window_state(vec![managed_session("ws-1", "Workspace", hsplit(term("a"), term("b")))]);
        assert_eq!(
            state.workspaces[0].managed_close_target("a"),
            None,
            "a pending/unbound pane closes locally",
        );

        // A single-pane workspace closes the whole workspace, not one pane.
        let single = window_state(vec![managed_session("ws-2", "Workspace", term("only"))]);
        assert_eq!(single.workspaces[0].managed_close_target("only"), None);
    }

    fn pane_info(pane_id: &str, title: &str, cwd: &str) -> v3::PaneInfo {
        v3::PaneInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            exit_status: None,
            reconstructed: true,
            no_persist: false,
        }
    }

    fn rt_info(
        runtime_id: &str,
        name: &str,
        policy: v3::WorkspacePolicy,
        panes: Vec<v3::PaneInfo>,
        _active_pane_id: Option<&str>,
    ) -> v3::WorkspaceInfo {
        v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            name: name.to_string(),
            pane_count: panes.len() as u32,
            panes,
            policy: policy as i32,
            reconstructed: true,
            workspace_revision: 7,
            current_client_role: v3::WorkspaceClientRole::Unattached as i32,
            has_write_owner: false,
            read_only_client_count: 0,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
        }
    }

    /// A serialized workspace carries its current fields.
    #[test]
    fn serialized_workspace_state_has_expected_shape() {
        let state = window_state(vec![managed_session("ws-1", "Workspace", term("pane-1"))]);
        let json = serde_json::to_string(&state.workspaces[0]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("runtime").is_some(), "runtime must be present");
        assert!(value.get("layout").is_some(), "layout must be present");
    }

    #[test]
    fn managed_terminal_binding_returns_identity_once_runtime_present() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let session = managed_session("workspace-1", "Workspace", term("pane-1"));
        let mut state = window_state(vec![session]);

        assert!(
            state.managed_terminal_binding("pane-1").is_none(),
            "a terminal stays unroutable until the workspace has a live runtime",
        );

        state.workspaces[0].runtime.runtime_id = Some(runtime_id.clone());
        state.rebuild_pane_reverse_index();

        let binding = state
            .managed_terminal_binding("pane-1")
            .expect("identity binding should resolve once a runtime exists");
        assert_eq!(binding.workspace_id, "workspace-1");
        assert_eq!(binding.endpoint, RuntimeEndpoint::Local);
        assert_eq!(binding.runtime_id, runtime_id);
        assert_eq!(binding.runtime_pane_id, "pane-1");
    }

    #[test]
    fn runtime_lookup_helpers_resolve_workspace_and_pane_targets() {
        let endpoint = RuntimeEndpoint::remote("builder.example");
        let pane_id = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term(pane_id),
            endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        let state = window_state(vec![session]);

        assert_eq!(
            state.workspace_for_runtime(&endpoint, "d7d04564-b2bf-4302-9495-e65c4df12ac6"),
            Some("workspace-1".into()),
        );
        assert_eq!(
            state.runtime_pane_target(&endpoint, pane_id),
            Some(("workspace-1".into(), pane_id.into())),
        );
    }

    #[test]
    fn apply_managed_pane_created_rekeys_layout_to_server_pane_id() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        let rekey = state
            .apply_managed_pane_created(
                "workspace-1",
                "pane-1",
                "d7d04564-b2bf-4302-9495-e65c4df12ac6",
                "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
            )
            .expect("pane creation for a known layout terminal should apply");

        assert_eq!(rekey.workspace_id, "workspace-1");
        assert_eq!(rekey.old_uuid, "pane-1");
        assert_eq!(rekey.new_uuid, "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26");

        let session = &state.workspaces[0];
        assert_eq!(
            session.runtime.runtime_id.as_deref(),
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        // The layout terminal now IS the server pane id (identity invariant).
        assert!(session.layout.contains_terminal("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"));
        assert!(!session.layout.contains_terminal("pane-1"));
    }

    #[test]
    fn apply_managed_pane_created_rejects_unknown_layout_terminal() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);
        let before = state.clone();

        assert!(
            state
                .apply_managed_pane_created(
                    "workspace-1",
                    "missing-pane",
                    "d7d04564-b2bf-4302-9495-e65c4df12ac6",
                    "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
                )
                .is_none()
        );
        assert_eq!(state, before);
    }

    #[test]
    fn managed_terminal_binding_uses_identity_after_pane_ack() {
        // After a pane ack re-keys the layout terminal to the server pane id,
        // the routable binding is the identity of that id — there is no
        // separate binding table (RFC-031).
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let server_pane = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";

        state
            .apply_managed_pane_created("workspace-1", "pane-1", runtime_id, server_pane)
            .expect("pane ack applies");

        // The old client-minted id no longer resolves; the server pane id does,
        // and resolves to itself (identity).
        assert!(state.managed_terminal_binding("pane-1").is_none());
        let binding = state.managed_terminal_binding(server_pane).expect("server pane id resolves");
        assert_eq!(binding.runtime_pane_id, server_pane);
        assert_eq!(binding.runtime_id, runtime_id);
    }

    #[test]
    fn apply_managed_pane_closed_prunes_state_and_preserves_remaining_terminal() {
        let left = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let right = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term(left), term(right)),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.active_terminal_uuid = Some(left.into());
        let mut state = window_state(vec![session]);

        let updated = state
            .apply_managed_pane_closed("workspace-1", left)
            .expect("removing one branch of a managed split should preserve the workspace");

        assert_eq!(updated.layout.terminal_uuids(), vec![right.to_string()]);
        assert_eq!(updated.active_terminal_uuid.as_deref(), Some(right));
        assert!(updated.recovery_for(left).is_none());
        assert!(updated.recovery_for(right).is_some());
    }

    #[test]
    fn recover_managed_workspaces_from_inventory_creates_recoverable_layouts() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let second_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::WorkspacePolicy::Persistent,
                vec![
                    pane_info(first_pane, "Shell", "/srv/project"),
                    pane_info(second_pane, "Logs", "/srv/project"),
                ],
                Some(second_pane),
            )],
        );

        assert_eq!(recovered.len(), 1);
        assert_eq!(state.workspaces.len(), 1);
        let session = &recovered[0];
        assert_eq!(session.uuid, "inventory:local:d7d04564-b2bf-4302-9495-e65c4df12ac6");
        assert_eq!(session.name, "Recovered Workspace");
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(session.runtime.runtime_id.as_deref(), Some(runtime_id));
        assert_eq!(session.active_terminal_uuid.as_deref(), Some(first_pane));
        // The inventory pane ids ARE the layout terminal uuids (identity).
        assert_eq!(
            session.layout.terminal_uuids(),
            vec![first_pane.to_string(), second_pane.to_string()]
        );

        // The daemon sends its authoritative tree on open; the client adopts it.
        let mut snapshot = snapshot(
            runtime_id,
            vec![
                pane_snapshot(first_pane, "Shell", "/srv/project", b"shell"),
                pane_snapshot(second_pane, "Logs", "/srv/project", b"logs"),
            ],
        );
        snapshot.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(uuid::Uuid::parse_str(first_pane).unwrap()),
            pane_tree_leaf(uuid::Uuid::parse_str(second_pane).unwrap()),
        ));
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
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::WorkspacePolicy::Persistent,
                vec![pane_info("07fa83b4-9ae3-4354-a1c5-1f685ffab370", "Shell", "/srv/project")],
                None,
            )],
        );

        assert!(recovered.is_empty());
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].uuid, "workspace-1");
    }

    #[test]
    fn apply_managed_workspace_opened_empty_runtime_keeps_placeholder_layout() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::WorkspacePolicy::Persistent,
                vec![],
                None,
            )],
        );

        let session =
            recovered.first().expect("inventory should synthesize a placeholder workspace");
        let placeholder_uuids = session.layout.terminal_uuids();
        let opened = state
            .apply_managed_workspace_opened(
                &session.uuid,
                runtime_id,
                &snapshot(runtime_id, vec![]),
            )
            .expect("empty runtime should still reconcile");

        // A tree-less (empty) snapshot yields a minimal result: the client keeps
        // its placeholder layout and the daemon-bridge bootstrap creates panes.
        assert!(opened.panes_to_create.is_empty());
        assert!(opened.snapshot_restores.is_empty());
        assert_eq!(opened.session_state.layout.terminal_uuids(), placeholder_uuids);
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
            workspaces: vec![rt_info(
                &runtime_id,
                "Recovered Workspace",
                v3::WorkspacePolicy::Persistent,
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
                .workspaces
                .iter()
                .any(|session| session.uuid == format!("inventory:local:{runtime_id}"))
        );
    }

    #[test]
    fn needs_inventory_bootstrap_when_no_managed_workspace_uses_endpoint() {
        let state = window_state(vec![workspace("session-1", "Session 1", term("pane-1"))]);

        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("builder.example")));
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
                RuntimeEndpoint::remote("builder.example"),
                WorkspacePolicy::Persistent,
                Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            ),
        ]);

        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("builder.example")));
        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("other.example")));
    }

    #[test]
    fn reconcile_endpoint_event_workspace_opened_adopts_tree_and_restores() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        let runtime_id = uuid::Uuid::new_v4().to_string();
        let left = uuid::Uuid::new_v4();
        let right = uuid::Uuid::new_v4();
        let mut state = window_state(vec![managed_session(
            "workspace-1",
            "Workspace",
            hsplit(term("client-a"), term("client-b")),
        )]);

        let mut snap = snapshot(
            &runtime_id,
            vec![
                pane_snapshot(&left.to_string(), "Shell", "/srv/project", b"restored output"),
                pane_snapshot(&right.to_string(), "Logs", "/var/log", b"logs"),
            ],
        );
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(left),
            pane_tree_leaf(right),
        ));

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snap,
        });

        assert_eq!(
            transition.rebuilt_workspaces,
            vec![ManagedWorkspaceRebuild {
                workspace_id: "workspace-1".into(),
                session_state: state.workspaces[0].clone(),
            }],
        );
        // Pure view: adopting the tree never mints panes to match the daemon.
        assert!(transition.pane_create_requests.is_empty());
        // Restores map 1:1 to the adopted (identity) layout terminals.
        assert_eq!(transition.pane_snapshot_restores.len(), 2);
        assert!(
            transition
                .pane_snapshot_restores
                .iter()
                .any(|r| r.layout_terminal_uuid == left.to_string() && r.title == "Shell")
        );
        // The stale client terminals are torn down.
        assert!(transition.removed_layout_terminals.contains(&"client-a".to_string()));
        assert!(transition.removed_layout_terminals.contains(&"client-b".to_string()));
        assert_eq!(state.workspaces[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    }

    #[test]
    fn reconcile_snapshot_carries_interaction_modes_to_restore() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "vim", "/home", b"");
        snap.terminal_modes = Some(v3::TerminalModeState {
            bracketed_paste: true,
            focus_reporting: false,
            application_cursor_keys: true,
            application_keypad: true,
            alternate_screen: false,
            cursor_hidden: false,
            mouse_mode: v3::MouseMode::Any as i32,
            sgr_mouse: true,
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: single_leaf_snapshot(&runtime_id, &terminal_uuid, snap),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        let modes = restore.terminal_modes.as_ref().expect("terminal_modes should be present");
        assert!(modes.application_cursor_keys);
        assert!(modes.application_keypad);
        assert_eq!(modes.mouse_mode, v3::MouseMode::Any as i32);
        assert!(modes.sgr_mouse);
        assert!(modes.bracketed_paste);
    }

    /// Snapshot with `focus_reporting` and `cursor_hidden` propagates through
    /// reconciliation so the restore path can re-apply them. #765.
    #[test]
    fn reconcile_snapshot_carries_focus_and_cursor_modes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "htop", "/home", b"");
        snap.terminal_modes = Some(v3::TerminalModeState {
            focus_reporting: true,
            cursor_hidden: true,
            alternate_screen: true,
            ..Default::default()
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: single_leaf_snapshot(&runtime_id, &terminal_uuid, snap),
        });

        let modes =
            transition.pane_snapshot_restores[0].terminal_modes.as_ref().expect("modes present");
        assert!(modes.focus_reporting);
        assert!(modes.cursor_hidden);
        assert!(modes.alternate_screen);
    }

    #[test]
    fn reconcile_endpoint_event_pane_created_rekeys_and_requests_recovery() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneCreated {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: "pane-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".into(),
        });

        // After the re-key the layout terminal IS the server pane id.
        assert_eq!(
            transition.connected_layout_terminals,
            vec!["598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".to_string()]
        );
        assert_eq!(
            transition.layout_terminals_to_recover,
            vec!["598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".to_string()]
        );
        assert_eq!(
            transition.pane_rekeys,
            vec![PaneRekey {
                workspace_id: "workspace-1".into(),
                old_uuid: "pane-1".into(),
                new_uuid: "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".into(),
            }]
        );
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Connected,
            }],
        );
        assert!(
            state.workspaces[0].layout.contains_terminal("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26")
        );
    }

    #[test]
    fn reconcile_endpoint_event_pane_closed_removes_terminal_and_rebuilds_workspace() {
        let left = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let right = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term(left), term(right)),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        let mut state = window_state(vec![session]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneClosed {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: right.into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: right.into(),
        });

        assert_eq!(transition.removed_layout_terminals, vec![right.to_string()]);
        assert_eq!(transition.rebuilt_workspaces.len(), 1);
        assert_eq!(state.workspaces[0].layout.terminal_uuids(), vec![left.to_string()]);
    }

    #[test]
    fn reconcile_endpoint_event_workspace_detached_preserves_runtime_id_and_marks_disconnected() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::remote("builder.example"),
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceDetached {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
        });

        assert_eq!(state.workspaces[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
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
            RuntimeEndpoint::remote("builder.example"),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceTerminated {
            workspace_id: "workspace-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            reason: v3::WorkspaceTerminationReason::Explicit,
        });

        assert_eq!(state.workspaces[0].runtime.runtime_id, None);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Disconnected,
            }],
        );
    }

    /// After the layout/state/recovery module split, session types imported
    /// through `crate::workspace::*` must still compose correctly in workspace
    /// state operations.
    #[test]
    fn session_types_compose_after_module_split() {
        let mut state = window_state(vec![workspace("s1", "Work", term("t1"))]);
        let ws = &mut state.workspaces[0];
        ws.set_recovery("t1", PaneRecovery::empty_shell());
        assert!(ws.recovery_for("t1").is_some());
        ws.prune_recovery();
        assert!(ws.recovery_for("t1").is_some());
    }

    #[test]
    fn dismissed_runtime_is_not_resurrected_by_inventory() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        // Dismiss the runtime (simulates user closing the workspace).
        state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);

        // Inventory refresh reports the runtime still exists on the daemon.
        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            workspaces: vec![rt_info(
                &runtime_id,
                "Should Not Resurrect",
                v3::WorkspacePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/tmp")],
                Some(&pane_id),
            )],
        });

        assert!(
            transition.recovered_workspaces.is_empty(),
            "dismissed runtime must not be resurrected by inventory"
        );
        assert!(
            !state.workspaces.iter().any(|s| s.runtime.runtime_id.as_deref() == Some(&runtime_id)),
            "dismissed runtime must not appear in session state"
        );
    }

    /// Closing a remote workspace must prevent resurrection from the remote
    /// daemon's inventory. Regression test for #248.
    #[test]
    fn dismissed_remote_runtime_is_not_resurrected_by_inventory() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let endpoint = RuntimeEndpoint::remote("build-host");
        let mut state = WindowState::default_for_test();

        state.dismiss_runtime(&endpoint, &runtime_id);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: endpoint.clone(),
            workspaces: vec![rt_info(
                &runtime_id,
                "Remote Work",
                v3::WorkspacePolicy::Persistent,
                vec![pane_info(&pane_id, "bash", "/home/user")],
                Some(&pane_id),
            )],
        });

        assert!(
            transition.recovered_workspaces.is_empty(),
            "dismissed remote runtime must not be resurrected"
        );
    }

    #[test]
    fn dismissed_runtime_ids_survive_serde_roundtrip() {
        let mut state = WindowState::default_for_test();
        state.dismiss_runtime(&RuntimeEndpoint::Local, "runtime-abc");
        state.dismiss_runtime(&RuntimeEndpoint::Local, "runtime-def");

        let json = serde_json::to_string(&state).unwrap();
        let restored: WindowState = serde_json::from_str(&json).unwrap();

        assert!(restored.dismissed_runtime_ids.contains("runtime-abc"));
        assert!(restored.dismissed_runtime_ids.contains("runtime-def"));
    }

    #[test]
    fn repeated_close_and_inventory_cycles_stay_clean() {
        let mut state = WindowState::default_for_test();

        for i in 0..5 {
            let runtime_id = uuid::Uuid::new_v4().to_string();
            let pane_id = uuid::Uuid::new_v4().to_string();

            state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);

            let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
                endpoint: RuntimeEndpoint::Local,
                workspaces: vec![rt_info(
                    &runtime_id,
                    &format!("Dismissed {i}"),
                    v3::WorkspacePolicy::Persistent,
                    vec![pane_info(&pane_id, "bash", "/tmp")],
                    Some(&pane_id),
                )],
            });

            assert!(
                transition.recovered_workspaces.is_empty(),
                "cycle {i}: dismissed runtime must not be recovered"
            );
        }

        assert_eq!(
            state.dismissed_runtime_ids.len(),
            1,
            "only the last dismissed ID should survive — earlier ones were pruned by inventory reconciliation"
        );
    }

    #[test]
    fn close_workspace_without_runtime_id_removes_cleanly() {
        let mut state = window_state(vec![managed_session("ws-1", "Disconnected", term("t1"))]);
        // No runtime_id — workspace was never connected.
        state.workspaces[0].runtime.runtime_id = None;

        // Dismiss with empty string (no runtime to track).
        assert_eq!(state.workspaces.len(), 1);
        state.workspaces.retain(|s| s.uuid != "ws-1");
        assert!(state.workspaces.is_empty());
    }

    #[test]
    fn workspace_opened_preserves_layout_cwd_from_snapshot() {
        use rttx_proto::v3_tree::pane_tree_leaf;

        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        // The layout terminal IS the server pane id (identity).
        let mut state = window_state(vec![managed_session_with_runtime(
            "ws-1",
            "Work",
            term(&pane_id),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        )]);
        state.workspaces[0].layout.set_terminal_cwd(&pane_id, Some("/old/path".into()));

        let mut snap = pane_snapshot(&pane_id, "bash", "/new/project", b"");
        snap.cols = 80;
        snap.rows = 24;
        let mut ws_snap = snapshot(&runtime_id, vec![snap]);
        ws_snap.workspace_revision = 2;
        ws_snap.tree = Some(pane_tree_leaf(uuid::Uuid::parse_str(&pane_id).unwrap()));

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id,
            snapshot: ws_snap,
        });

        assert!(!transition.pane_snapshot_restores.is_empty());
        assert_eq!(transition.pane_snapshot_restores[0].cwd, "/new/project");

        // The layout CWD must also be updated from the snapshot.
        let session = state.workspaces.iter().find(|s| s.uuid == "ws-1").unwrap();
        let layout_uuid = &transition.pane_snapshot_restores[0].layout_terminal_uuid;
        assert_eq!(
            session.layout.terminal_cwd(layout_uuid).as_deref(),
            Some("/new/project"),
            "layout CWD must be updated from snapshot during workspace opened"
        );
    }

    #[test]
    fn snapshot_restore_carries_daemon_pane_dimensions() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("ws-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "Shell", "/home", b"$ ");
        snap.cols = 200;
        snap.rows = 50;

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: single_leaf_snapshot(&runtime_id, &terminal_uuid, snap),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        assert_eq!(restore.cols, 200);
        assert_eq!(restore.rows, 50);
    }

    #[test]
    fn input_sync_targets_returns_siblings_when_enabled() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            hsplit(term("pane-1"), term("pane-2")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        session.input_sync = true;
        let state = window_state(vec![session]);

        let targets = state.input_sync_targets("pane-1");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].runtime_pane_id, "pane-2");
    }

    #[test]
    fn input_sync_targets_empty_when_disabled() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            hsplit(term("pane-1"), term("pane-2")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        let state = window_state(vec![session]);

        assert!(state.input_sync_targets("pane-1").is_empty());
    }

    #[test]
    fn input_sync_targets_empty_for_direct_workspace() {
        let mut session = workspace("s1", "Direct", hsplit(term("pane-1"), term("pane-2")));
        session.input_sync = true;
        let state = window_state(vec![session]);

        assert!(state.input_sync_targets("pane-1").is_empty());
    }

    #[test]
    fn reconcile_session_missing_emits_status_update() {
        let mut state = WindowState::default_for_test();

        let transition =
            state.reconcile_endpoint_event(&EndpointEvent::WorkspaceConnectionChanged {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::SessionMissing,
            });

        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::SessionMissing,
            }],
        );
    }

    #[test]
    fn workspace_opened_returns_previous_layout_terminals() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);

        let server_left = uuid::Uuid::new_v4();
        let server_right = uuid::Uuid::new_v4();
        let mut snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(&server_left.to_string(), "Shell", "/srv", b""),
                pane_snapshot(&server_right.to_string(), "Logs", "/srv", b""),
            ],
        );
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(server_left),
            pane_tree_leaf(server_right),
        ));

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
            .expect("should adopt the server tree");

        assert_eq!(
            opened.previous_layout_terminals,
            vec!["left".to_string(), "right".to_string()],
            "previous_layout_terminals should capture the pre-adoption UUIDs",
        );
    }

    #[test]
    fn reconcile_workspace_opened_keeps_layout_for_treeless_snapshot() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.into(),
            // A tree-less snapshot: the client keeps its existing layout and
            // removes nothing (the daemon-bridge bootstrap creates panes).
            snapshot: snapshot(runtime_id, vec![]),
        });

        assert!(
            !transition.removed_layout_terminals.contains(&"left".to_string()),
            "matched terminal should not be marked as removed",
        );
        assert!(
            !transition.removed_layout_terminals.contains(&"right".to_string()),
            "existing terminal stays in layout and should not be removed",
        );
        assert_eq!(
            transition.rebuilt_workspaces.len(),
            1,
            "workspace should be rebuilt after opening",
        );
        assert_eq!(
            transition.rebuilt_workspaces[0].session_state.layout.terminal_uuids(),
            vec!["left".to_string(), "right".to_string()],
            "both terminals should remain in the layout",
        );
    }

    #[test]
    fn dismissed_runtime_ids_pruned_after_inventory_reconciliation() {
        let mut state = WindowState::default_for_test();
        let stale_id = uuid::Uuid::new_v4().to_string();
        let live_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();

        state.dismiss_runtime(&RuntimeEndpoint::Local, &stale_id);
        state.dismiss_runtime(&RuntimeEndpoint::Local, &live_id);
        assert_eq!(state.dismissed_runtime_ids.len(), 2);

        // Inventory only contains live_id — stale_id was already removed by daemon.
        let _transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            workspaces: vec![rt_info(
                &live_id,
                "Still Running",
                v3::WorkspacePolicy::Persistent,
                vec![pane_info(&pane_id, "bash", "/tmp")],
                Some(&pane_id),
            )],
        });

        assert!(
            !state.dismissed_runtime_ids.contains(&stale_id),
            "stale dismissed ID should be pruned after inventory reconciliation"
        );
        assert!(
            state.dismissed_runtime_ids.contains(&live_id),
            "live dismissed ID should be retained"
        );
    }

    // ── Resync (StreamOverflow recovery) ──

    #[test]
    fn resync_produces_snapshot_restores_for_bound_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        // Simulate an opened workspace so bindings exist.
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home", b"initial")],
            ),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home/project", b"resynced output")],
            ),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        assert_eq!(restore.layout_terminal_uuid, pane_uuid);
        assert_eq!(restore.cwd, "/home/project");
        assert_eq!(restore.scrollback_tail, bytes::Bytes::from_static(b"resynced output"));
    }

    #[test]
    fn resync_ignores_unknown_workspace() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "nonexistent".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![]),
        });

        assert!(transition.pane_snapshot_restores.is_empty());
    }

    #[test]
    fn resync_skips_unbound_runtime_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let extra_pane = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![
                    pane_snapshot(&pane_uuid, "bash", "/home", b"resynced"),
                    pane_snapshot(&extra_pane, "zsh", "/tmp", b"extra"),
                ],
            ),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        assert_eq!(transition.pane_snapshot_restores[0].layout_terminal_uuid, pane_uuid);
    }

    #[test]
    fn resync_does_not_rebuild_layout() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home", b"resynced")],
            ),
        });

        assert!(transition.rebuilt_workspaces.is_empty());
        assert!(transition.recovered_workspaces.is_empty());
        assert!(transition.pane_create_requests.is_empty());
    }

    #[test]
    fn resync_carries_terminal_modes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let mut snap = pane_snapshot(&pane_uuid, "vim", "/home", b"vim content");
        snap.terminal_modes = Some(v3::TerminalModeState {
            bracketed_paste: true,
            application_cursor_keys: true,
            ..Default::default()
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![snap]),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let modes = transition.pane_snapshot_restores[0]
            .terminal_modes
            .as_ref()
            .expect("modes should be present");
        assert!(modes.bracketed_paste);
        assert!(modes.application_cursor_keys);
    }

    #[test]
    fn store_preferences_conversion_preserves_all_fields() {
        use crate::store::models::preferences::PreferencesV1;

        let prefs = crate::preferences::Preferences {
            font: "Hack 11".into(),
            scrollback_lines: 5000,
            smart_clipboard: true,
            paste_guard_threshold: 512,
            ..Default::default()
        };
        let v1: PreferencesV1 = (&prefs).into();
        let round_tripped: crate::preferences::Preferences = v1.into();
        assert_eq!(round_tripped.font, "Hack 11");
        assert_eq!(round_tripped.scrollback_lines, 5000);
        assert!(round_tripped.smart_clipboard);
        assert_eq!(round_tripped.paste_guard_threshold, 512);
    }

    // ── Reverse index tests ──────────────────────────────────────

    #[test]
    fn reverse_index_matches_linear_scan_for_bound_pane() {
        let endpoint = RuntimeEndpoint::remote("builder.example");
        let pane_id = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term(pane_id),
            endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        let state = window_state(vec![session]);

        assert_eq!(
            state.runtime_pane_target(&endpoint, pane_id),
            Some(("workspace-1".into(), pane_id.into())),
        );
    }

    #[test]
    fn reverse_index_maps_layout_terminals_by_identity() {
        let state = window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        // Under the identity invariant every managed layout terminal maps to
        // itself in the reverse index.
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, "pane-1"),
            Some(("workspace-1".into(), "pane-1".into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_pane_create() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        state.apply_managed_pane_created(
            "workspace-1",
            "pane-1",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        );

        // After the re-key the layout terminal IS the server pane id.
        assert_eq!(
            state.runtime_pane_target(
                &RuntimeEndpoint::Local,
                "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"
            ),
            Some(("workspace-1".into(), "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_pane_close() {
        let left = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let right = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term(left), term(right)),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        let mut state = window_state(vec![session]);

        state.apply_managed_pane_closed("workspace-1", left);

        assert!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, left).is_none(),
            "closed pane must be removed from the reverse index",
        );
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, right),
            Some(("workspace-1".into(), right.into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_workspace_opened_adoption() {
        use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};

        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let runtime_pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let runtime_pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut state = window_state(vec![managed_session(
            "workspace-1",
            "Workspace",
            hsplit(term("client-a"), term("client-b")),
        )]);

        let mut snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(runtime_pane_a, "Shell", "/home", b""),
                pane_snapshot(runtime_pane_b, "Logs", "/var", b""),
            ],
        );
        snap.tree = Some(pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(uuid::Uuid::parse_str(runtime_pane_a).unwrap()),
            pane_tree_leaf(uuid::Uuid::parse_str(runtime_pane_b).unwrap()),
        ));
        state.apply_managed_workspace_opened("workspace-1", runtime_id, &snap);

        // Layout terminals ARE the server pane ids (identity).
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, runtime_pane_a),
            Some(("workspace-1".into(), runtime_pane_a.into())),
        );
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, runtime_pane_b),
            Some(("workspace-1".into(), runtime_pane_b.into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_inventory_recovery() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                &runtime_id,
                "Recovered",
                v3::WorkspacePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/home")],
                Some(&pane_id),
            )],
        );

        // Inventory recovery creates self-bindings that are NOT pending
        // (the pane IDs come from the daemon), so they should be in the index.
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, &pane_id),
            Some((format!("inventory:local:{runtime_id}"), pane_id.clone())),
        );
    }

    #[test]
    fn reverse_index_not_serialized() {
        let session = managed_session_with_runtime(
            "ws-1",
            "Work",
            term("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        let state = window_state(vec![session]);

        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("pane_reverse_index"),
            "reverse index must not appear in serialized JSON",
        );

        let deserialized: WindowState = serde_json::from_str(&json).unwrap();
        assert!(
            deserialized.pane_reverse_index.is_empty(),
            "deserialized state must have empty reverse index",
        );
    }

    #[test]
    fn reverse_index_multi_workspace_multi_endpoint() {
        let local_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let remote_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let remote_endpoint = RuntimeEndpoint::remote("builder.example");

        // Layout terminal uuids ARE their server pane ids (identity).
        let local_session = managed_session_with_runtime(
            "ws-local",
            "Local",
            term(local_pane),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );

        let remote_session = managed_session_with_runtime(
            "ws-remote",
            "Remote",
            term(remote_pane),
            remote_endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );

        let state = window_state(vec![local_session, remote_session]);

        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, local_pane),
            Some(("ws-local".into(), local_pane.into())),
        );
        assert_eq!(
            state.runtime_pane_target(&remote_endpoint, remote_pane),
            Some(("ws-remote".into(), remote_pane.into())),
        );
        // Cross-endpoint lookup must not match.
        assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, remote_pane).is_none());
        assert!(state.runtime_pane_target(&remote_endpoint, local_pane).is_none());
    }

    #[test]
    fn pane_ack_establishes_identity_routing_without_a_binding_table() {
        // Net-new pure-state coverage (RFC-031): a daemon pane ack re-keys the
        // client layout terminal onto the server pane id, so routing becomes
        // the identity of that id and no binding table exists.
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("client-pane"))]);
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let server_pane = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";

        let rekey = state
            .apply_managed_pane_created("workspace-1", "client-pane", runtime_id, server_pane)
            .expect("pane ack applies to a known layout terminal");

        assert_eq!(rekey.old_uuid, "client-pane");
        assert_eq!(rekey.new_uuid, server_pane);
        assert!(state.workspaces[0].layout.contains_terminal(server_pane));
        assert!(state.managed_terminal_binding("client-pane").is_none());
        assert_eq!(
            state.managed_terminal_binding(server_pane).map(|binding| binding.runtime_pane_id),
            Some(server_pane.to_string()),
        );
    }
}

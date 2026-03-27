use crate::runtime::{RuntimeEndpoint, reconcile_bindings};
use crate::session::layout::{PaneRecovery, SessionState, SplitOrientation, WindowState};
use rttx_proto::proto;
use std::collections::BTreeMap;

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

impl WindowState {
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

        let panes_to_create = if had_runtime_id {
            Vec::new()
        } else {
            let mut placeholders = reconciliation.disconnected_layout_panes.clone();
            if let Some(initial_terminal_uuid) = layout_terminal_uuids.first() {
                placeholders.retain(|layout_terminal_uuid| {
                    layout_terminal_uuid != initial_terminal_uuid
                });
            }
            placeholders
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

fn snapshot_pane_id(pane_snapshot: &proto::PaneSnapshot) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane_snapshot.pane_id)
        .ok()
        .map(|uuid| uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use crate::test_helpers::{
        hsplit, managed_session, managed_session_with_runtime, term, term_full, window_state,
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
        let mut state = window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

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
        assert_eq!(
            session.mode.daemon_session_id(),
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
    }

    #[test]
    fn apply_managed_pane_created_rejects_unknown_layout_terminal() {
        let mut state = window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);
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
}

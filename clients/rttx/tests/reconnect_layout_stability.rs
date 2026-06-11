//! Integration test for #547: workspace layout must not grow across
//! repeated reconnect cycles with changing daemon pane IDs.

use rttx::daemon_bridge::EndpointEvent;
use rttx::runtime::{WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;

fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.to_string(), profile: None, cwd: None, custom_title: None }
}

fn hsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn pane_snapshot(pane_id: &str, title: &str, cwd: &str) -> rttx_proto::v3::PaneSnapshot {
    rttx_proto::v3::PaneSnapshot {
        pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
        pane_output_seq: 0,
        title: title.to_string(),
        cwd: cwd.to_string(),
        cols: 120,
        rows: 40,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::new(),
        total_scrollback_bytes: 0,
        scrollback_complete: true,
    }
}

fn snapshot(
    runtime_id: &str,
    panes: Vec<rttx_proto::v3::PaneSnapshot>,
) -> rttx_proto::v3::WorkspaceSnapshot {
    rttx_proto::v3::WorkspaceSnapshot {
        tree: None,
        default_active_pane_id: Vec::new(),
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
        panes,
        workspace_revision: 1,
        client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
    }
}

/// Regression test for #547: simulates 5 disconnect/reconnect cycles where
/// the daemon restarts each time (new pane IDs). The layout must stay at
/// exactly 2 terminals throughout.
#[test]
fn layout_stays_stable_across_reconnect_cycles_with_fresh_pane_ids() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";

    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = hsplit(term("left"), term("right"));
    session.runtime = WorkspaceRuntime::managed_local(
        WorkspacePolicy::Persistent,
        &session.layout.terminal_uuids(),
    );
    session.runtime.runtime_id = Some(runtime_id.into());

    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    for cycle in 0..5 {
        let pane_a = uuid::Uuid::new_v4().to_string();
        let pane_b = uuid::Uuid::new_v4().to_string();

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.into(),
            snapshot: snapshot(
                runtime_id,
                vec![
                    pane_snapshot(&pane_a, "Shell", "/home"),
                    pane_snapshot(&pane_b, "Logs", "/var/log"),
                ],
            ),
        });

        assert_eq!(
            transition.rebuilt_workspaces.len(),
            1,
            "cycle {cycle}: workspace must be rebuilt",
        );
        let rebuilt = &transition.rebuilt_workspaces[0].session_state;
        assert_eq!(
            rebuilt.layout.terminal_count(),
            2,
            "cycle {cycle}: layout must stay at 2 terminals, not grow",
        );
        assert!(
            transition.skipped_runtime_panes.is_empty(),
            "cycle {cycle}: all runtime panes must be claimed",
        );
    }
}

//! Server-side protocol helpers.
//!
//! Convenience functions for constructing server response messages.

use crate::pane_tree::{PaneId, PaneTree, Side, SplitAxis, WorkspaceTree};
use crate::workspace::{ClientRole, Workspace, TerminationReason};
use rttx_proto::{uuid_to_bytes, v3};
use uuid::Uuid;

// ── V3 protocol helpers ─────────────────────────────────────────

/// Build a v3 `OutputDelta` push envelope.
#[must_use]
pub fn v3_delta(
    runtime_id: Uuid,
    pane_id: Uuid,
    data: bytes::Bytes,
    pane_output_seq: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::OutputDelta(
        v3::OutputDelta {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            data,
            pane_output_seq,
        },
    ))
}

/// Build a v3 `CwdChanged` push envelope.
#[must_use]
pub fn v3_cwd_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    cwd: String,
    workspace_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::CwdChanged(
        v3::CwdChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            cwd,
            workspace_revision,
        },
    ))
}

/// Build a v3 `TitleChanged` push envelope.
#[must_use]
pub fn v3_title_changed(
    runtime_id: Uuid,
    pane_id: Uuid,
    title: String,
    workspace_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::TitleChanged(
        v3::TitleChanged {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            title,
            workspace_revision,
        },
    ))
}

/// Build a v3 `PaneExited` push envelope.
#[must_use]
pub fn v3_pane_exited(
    runtime_id: Uuid,
    pane_id: Uuid,
    status: i32,
    workspace_revision: u64,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::PaneExited(
        v3::PaneExited {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            status,
            workspace_revision,
        },
    ))
}

/// Build a v3 `WorkspaceTerminated` push envelope.
#[must_use]
pub fn v3_workspace_terminated(
    runtime_id: Uuid,
    final_revision: u64,
    reason: TerminationReason,
) -> v3::ServerEnvelope {
    rttx_proto::v3_envelope::build_push_envelope(v3::server_envelope::Payload::WorkspaceTerminated(
        v3::WorkspaceTerminated {
            runtime_id: uuid_to_bytes(runtime_id),
            final_revision,
            reason: reason.as_v3_proto() as i32,
        },
    ))
}

// ── Workspace tree conversions (RFC-031 §5) ─────────────────────

/// Map a server split axis to its wire enum.
#[must_use]
pub const fn split_axis_to_proto(axis: SplitAxis) -> v3::PaneSplitAxis {
    match axis {
        SplitAxis::Horizontal => v3::PaneSplitAxis::Horizontal,
        SplitAxis::Vertical => v3::PaneSplitAxis::Vertical,
    }
}

/// Map a wire split axis to the server axis. An unspecified axis defaults to
/// horizontal (the daemon's even-split convention).
#[must_use]
pub const fn proto_axis_to_split(axis: v3::PaneSplitAxis) -> SplitAxis {
    match axis {
        v3::PaneSplitAxis::Vertical => SplitAxis::Vertical,
        v3::PaneSplitAxis::Unspecified | v3::PaneSplitAxis::Horizontal => SplitAxis::Horizontal,
    }
}

/// Map a wire tree side to the server `Side`, dropping the unspecified value
/// (which never addresses a real split branch).
#[must_use]
pub const fn proto_side_to_side(side: v3::PaneTreeSide) -> Option<Side> {
    match side {
        v3::PaneTreeSide::First => Some(Side::First),
        v3::PaneTreeSide::Second => Some(Side::Second),
        v3::PaneTreeSide::Unspecified => None,
    }
}

/// Decode a wire side path into a server path. Any unspecified or out-of-range
/// step makes the whole path unaddressable and yields `None`.
#[must_use]
pub fn decode_side_path(raw: &[i32]) -> Option<Vec<Side>> {
    raw.iter().map(|&v| v3::PaneTreeSide::try_from(v).ok().and_then(proto_side_to_side)).collect()
}

fn pane_tree_to_proto(node: &PaneTree) -> v3::PaneTreeNode {
    match node {
        PaneTree::Leaf { pane } => rttx_proto::v3_tree::pane_tree_leaf(pane.uuid()),
        PaneTree::Split { axis, ratio, first, second } => rttx_proto::v3_tree::pane_tree_split(
            split_axis_to_proto(*axis),
            *ratio,
            pane_tree_to_proto(first),
            pane_tree_to_proto(second),
        ),
    }
}

/// Build the wire representation of a workspace tree, or `None` for an empty
/// workspace.
#[must_use]
pub fn workspace_tree_to_proto(tree: &WorkspaceTree) -> Option<v3::PaneTreeNode> {
    tree.root().map(pane_tree_to_proto)
}

/// The wire bytes of the tree's default-active pane, or empty when there is
/// none.
#[must_use]
pub fn default_active_bytes(tree: &WorkspaceTree) -> Vec<u8> {
    tree.default_active().map(PaneId::uuid).map(uuid_to_bytes).unwrap_or_default()
}

/// The PTY size for a pane shared by multiple clients: the per-axis minimum of
/// every client's reported render size, so no client sees truncated output
/// (RFC-031 §4). Zero dimensions are ignored. Returns `None` when no client has
/// reported a usable size on the corresponding axis.
#[must_use]
pub fn min_client_pane_size<I>(sizes: I) -> Option<(u16, u16)>
where
    I: IntoIterator<Item = (u16, u16)>,
{
    let mut min_cols: Option<u16> = None;
    let mut min_rows: Option<u16> = None;
    for (cols, rows) in sizes {
        if cols > 0 {
            min_cols = Some(min_cols.map_or(cols, |m| m.min(cols)));
        }
        if rows > 0 {
            min_rows = Some(min_rows.map_or(rows, |m| m.min(rows)));
        }
    }
    Some((min_cols?, min_rows?))
}

/// Build a v3 `WorkspaceSnapshot` from a workspace's current state.
#[must_use]
pub fn build_v3_workspace_snapshot(
    rt: &Workspace,
    runtime_id: Uuid,
    client_role: v3::WorkspaceClientRole,
) -> v3::WorkspaceSnapshot {
    let panes: Vec<v3::PaneSnapshot> = rt
        .panes
        .values()
        .map(|pane| {
            let scrollback_data = crate::screen::strip_client_queries(
                pane.screen.snapshot_bytes(crate::pane::MAX_SNAPSHOT_BYTES),
            );
            let total_scrollback_bytes = pane.screen.raw_bytes().len() as u64;
            rttx_proto::v3_snapshot::build_pane_snapshot(
                rttx_proto::v3_snapshot::PaneSnapshotParams {
                    pane_id: pane.id,
                    pane_output_seq: pane.output_seq,
                    title: pane.title.clone().unwrap_or_default(),
                    cwd: pane.effective_cwd().unwrap_or_default(),
                    cols: u32::from(pane.cols),
                    rows: u32::from(pane.rows),
                    exit_status: pane.exit_status,
                    terminal_modes: pane.screen.terminal_mode_state(),
                    scrollback_tail: bytes::Bytes::from(scrollback_data),
                    total_scrollback_bytes,
                },
            )
        })
        .collect();
    rttx_proto::v3_snapshot::build_workspace_snapshot_with_tree(
        runtime_id,
        rt.revision(),
        client_role,
        panes,
        workspace_tree_to_proto(&rt.tree),
        default_active_bytes(&rt.tree),
    )
}

/// Build a v3 workspace inventory for `ListWorkspaces`.
#[must_use]
pub fn v3_workspace_inventory_for<'a, I>(
    client_id: Uuid,
    workspaces: I,
    has_inventory_v2: bool,
) -> Vec<v3::WorkspaceInfo>
where
    I: IntoIterator<Item = &'a Workspace>,
{
    let mut inventory: Vec<_> = workspaces
        .into_iter()
        .map(|rt| v3_workspace_info_for(client_id, rt, has_inventory_v2))
        .collect();
    inventory.sort_by(|left, right| left.id.cmp(&right.id));
    inventory
}

/// Build a v3 `WorkspaceInfo` for a single workspace.
///
/// Public so callers with per-workspace locks can build inventory entries
/// one at a time.
#[must_use]
pub fn v3_workspace_info_for(
    client_id: Uuid,
    rt: &Workspace,
    has_inventory_v2: bool,
) -> v3::WorkspaceInfo {
    let policy = match rt.policy {
        crate::workspace::WorkspacePolicy::Persistent => v3::WorkspacePolicy::Persistent,
        crate::workspace::WorkspacePolicy::Ephemeral => v3::WorkspacePolicy::Ephemeral,
    };
    let current_role = rt
        .client_role(client_id)
        .map_or(v3::WorkspaceClientRole::Unattached, ClientRole::as_v3_proto);

    let params = rttx_proto::v3_inventory::WorkspaceInfoParams {
        id: rt.id,
        name: rt.name.clone(),
        policy,
        pane_count: u32::try_from(rt.panes.len()).unwrap_or(u32::MAX),
        has_write_owner: rt.has_write_owner(),
        read_only_client_count: u32::try_from(rt.read_only_client_count()).unwrap_or(u32::MAX),
        current_client_role: current_role,
        workspace_revision: rt.revision(),
        reconstructed: rt.reconstructed,
    };

    if has_inventory_v2 {
        let mut panes: Vec<_> = rt
            .panes
            .values()
            .map(|pane| {
                rttx_proto::v3_inventory::build_pane_info(
                    rttx_proto::v3_inventory::PaneInfoParams {
                        pane_id: pane.id,
                        title: pane.title.clone().unwrap_or_default(),
                        cwd: pane.effective_cwd().unwrap_or_default(),
                        cols: u32::from(pane.cols),
                        rows: u32::from(pane.rows),
                        exit_status: pane.exit_status,
                        reconstructed: pane.reconstructed,
                        no_persist: pane.no_persist,
                    },
                )
            })
            .collect();
        panes.sort_by(|left, right| left.id.cmp(&right.id));
        rttx_proto::v3_inventory::build_workspace_info_v2(
            params,
            rttx_proto::v3_inventory::WorkspaceInfoV2Fields {
                active_pane_summary: String::new(),
                takeover_eligible: !rt.has_write_owner(),
                disabled_reason: String::new(),
                panes,
            },
        )
    } else {
        rttx_proto::v3_inventory::build_workspace_info(params)
    }
}

/// Build a v3 `DiagnosticsReport` from the current server state.
#[must_use]
pub fn v3_diagnostics_report(server: &crate::server::Server) -> v3::DiagnosticsReport {
    let report = server.diagnostics();
    let workspaces = report
        .workspaces
        .iter()
        .map(|s| {
            let panes = s
                .panes
                .iter()
                .map(|p| {
                    let id = uuid::Uuid::parse_str(&p.id).map(uuid_to_bytes).unwrap_or_default();
                    v3::PaneDiagnosticsInfo {
                        id,
                        raw_bytes_len: p.raw_bytes_len as u64,
                        pending_flush_len: p.pending_flush_len as u64,
                        is_exited: p.is_exited,
                    }
                })
                .collect();
            let id = uuid::Uuid::parse_str(&s.id).map(uuid_to_bytes).unwrap_or_default();
            v3::WorkspaceDiagnosticsInfo {
                id,
                name: s.name.clone(),
                active_pane_count: s.active_pane_count as u32,
                exited_pane_count: s.exited_pane_count as u32,
                command_history_len: 0,
                attached_client_count: s.attached_client_count as u32,
                panes,
            }
        })
        .collect();
    v3::DiagnosticsReport {
        workspace_count: report.workspace_count as u32,
        total_pane_count: report.total_pane_count as u32,
        total_active_panes: report.total_active_panes as u32,
        total_exited_panes: report.total_exited_panes as u32,
        client_count: report.client_count as u32,
        pty_writer_count: report.pty_writer_count as u32,
        total_raw_bytes: report.total_raw_bytes as u64,
        total_pending_flush: report.total_pending_flush as u64,
        total_command_history: 0,
        workspaces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v3_cwd_changed_message_contains_correct_fields() {
        let sid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let env = v3_cwd_changed(sid, pid, "/home/user".into(), 42);
        let Some(v3::server_envelope::Payload::CwdChanged(inner)) = env.payload else {
            panic!("expected CwdChanged");
        };
        assert_eq!(inner.cwd, "/home/user");
        assert_eq!(inner.workspace_revision, 42);
        assert_eq!(inner.runtime_id, sid.as_bytes().to_vec());
        assert_eq!(inner.pane_id, pid.as_bytes().to_vec());
    }

    #[test]
    fn v3_delta_builds_output_delta_push_envelope() {
        let rid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let env = v3_delta(rid, pid, bytes::Bytes::from_static(b"payload"), 7);
        // Push events carry request_id 0.
        assert_eq!(env.request_id, 0);
        let Some(v3::server_envelope::Payload::OutputDelta(d)) = env.payload else {
            panic!("expected OutputDelta");
        };
        assert_eq!(d.runtime_id, rid.as_bytes().to_vec());
        assert_eq!(d.pane_id, pid.as_bytes().to_vec());
        assert_eq!(d.data.as_ref(), b"payload");
        assert_eq!(d.pane_output_seq, 7);
    }

    #[test]
    fn v3_pane_exited_carries_status_and_revision() {
        let env = v3_pane_exited(Uuid::new_v4(), Uuid::new_v4(), 137, 9);
        let Some(v3::server_envelope::Payload::PaneExited(p)) = env.payload else {
            panic!("expected PaneExited");
        };
        assert_eq!(p.status, 137);
        assert_eq!(p.workspace_revision, 9);
    }

    #[test]
    fn v3_workspace_terminated_maps_reason() {
        let env = v3_workspace_terminated(Uuid::new_v4(), 3, TerminationReason::EphemeralLastDetach);
        let Some(v3::server_envelope::Payload::WorkspaceTerminated(t)) = env.payload else {
            panic!("expected WorkspaceTerminated");
        };
        assert_eq!(t.final_revision, 3);
        assert_eq!(t.reason, v3::WorkspaceTerminationReason::EphemeralDetach as i32);
    }

    // ── Workspace tree conversions (RFC-031 §5) ──

    #[test]
    fn split_axis_round_trips_through_proto() {
        for axis in [SplitAxis::Horizontal, SplitAxis::Vertical] {
            assert_eq!(proto_axis_to_split(split_axis_to_proto(axis)), axis);
        }
        // An unspecified wire axis defaults to horizontal.
        assert_eq!(proto_axis_to_split(v3::PaneSplitAxis::Unspecified), SplitAxis::Horizontal);
    }

    #[test]
    fn decode_side_path_rejects_unaddressable_steps() {
        assert_eq!(
            decode_side_path(&[v3::PaneTreeSide::First as i32, v3::PaneTreeSide::Second as i32,]),
            Some(vec![Side::First, Side::Second])
        );
        // An unspecified step makes the whole path unaddressable.
        assert_eq!(decode_side_path(&[v3::PaneTreeSide::Unspecified as i32]), None);
        assert_eq!(decode_side_path(&[42]), None);
        // The empty path addresses the root split and is valid.
        assert_eq!(decode_side_path(&[]), Some(vec![]));
    }

    #[test]
    fn empty_workspace_tree_has_no_proto_node() {
        let tree = WorkspaceTree::new();
        assert!(workspace_tree_to_proto(&tree).is_none());
        assert!(default_active_bytes(&tree).is_empty());
    }

    #[test]
    fn workspace_tree_converts_to_matching_proto_structure() {
        let mut tree = WorkspaceTree::new();
        let a = PaneId::new();
        let b = PaneId::new();
        tree.insert_root(a);
        assert!(tree.split(a, b, SplitAxis::Vertical, 0.25));

        let node = workspace_tree_to_proto(&tree).expect("non-empty tree");
        let Some(v3::pane_tree_node::Node::Split(split)) = node.node else {
            panic!("expected a split at the root");
        };
        assert_eq!(split.axis, v3::PaneSplitAxis::Vertical as i32);
        assert!((split.ratio - 0.25).abs() < f32::EPSILON);
        // default-active is the first pane, encoded as its raw uuid bytes.
        assert_eq!(default_active_bytes(&tree), uuid_to_bytes(a.uuid()));
    }

    // ── Multi-client PTY min-size policy (RFC-031 §4) ──

    #[test]
    fn min_pane_size_is_per_axis_minimum() {
        assert_eq!(min_client_pane_size([(100, 40), (80, 50), (120, 24)]), Some((80, 24)));
    }

    #[test]
    fn min_pane_size_ignores_zero_dimensions() {
        // A client reporting 0 on an axis (not yet rendered) is ignored on
        // that axis but can still constrain the other.
        assert_eq!(min_client_pane_size([(100, 0), (0, 30)]), Some((100, 30)));
    }

    #[test]
    fn min_pane_size_single_client_tracks_that_client() {
        assert_eq!(min_client_pane_size([(90, 30)]), Some((90, 30)));
    }

    #[test]
    fn min_pane_size_none_when_no_usable_dimensions() {
        assert_eq!(min_client_pane_size(std::iter::empty()), None);
        assert_eq!(min_client_pane_size([(0, 0)]), None);
    }
}

//! V3 workspace-tree messages: builders for the server-authoritative pane
//! tree, its structural mutations, and viewport messages (RFC-031 §5).
//!
//! The wire tree ([`v3::PaneTreeNode`]) mirrors the server's authoritative
//! `WorkspaceTree`: a binary tree whose leaves name panes and whose internal
//! nodes are splits carrying a logical ratio. Pane identity is always
//! server-assigned (RFC-031 G1); these builders never mint identity.

use crate::v3;

/// Build a leaf node naming a single pane.
#[must_use]
pub fn pane_tree_leaf(pane_id: uuid::Uuid) -> v3::PaneTreeNode {
    v3::PaneTreeNode {
        node: Some(v3::pane_tree_node::Node::Leaf(v3::PaneTreeLeaf {
            pane_id: crate::uuid_to_bytes(pane_id),
        })),
    }
}

/// Build a split node joining two subtrees with a logical `ratio`.
#[must_use]
pub fn pane_tree_split(
    axis: v3::PaneSplitAxis,
    ratio: f32,
    first: v3::PaneTreeNode,
    second: v3::PaneTreeNode,
) -> v3::PaneTreeNode {
    v3::PaneTreeNode {
        node: Some(v3::pane_tree_node::Node::Split(Box::new(v3::PaneTreeSplit {
            axis: axis as i32,
            ratio,
            first: Some(Box::new(first)),
            second: Some(Box::new(second)),
        }))),
    }
}

/// Build a `PaneSplit` tree-delta event.
#[must_use]
pub fn build_pane_split(
    runtime_id: uuid::Uuid,
    target_pane_id: uuid::Uuid,
    new_pane_id: uuid::Uuid,
    axis: v3::PaneSplitAxis,
    ratio: f32,
    runtime_revision: u64,
) -> v3::PaneSplit {
    v3::PaneSplit {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        target_pane_id: crate::uuid_to_bytes(target_pane_id),
        new_pane_id: crate::uuid_to_bytes(new_pane_id),
        axis: axis as i32,
        ratio,
        runtime_revision,
    }
}

/// Build a `ServerEnvelope` response carrying a `PaneSplit`.
#[must_use]
pub fn build_pane_split_response(request_id: u64, split: v3::PaneSplit) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::PaneSplit(split),
    )
}

/// Build a `ServerEnvelope` push event carrying a `PaneSplit`.
#[must_use]
pub fn build_pane_split_push(split: v3::PaneSplit) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::PaneSplit(split))
}

/// Build a `SplitResized` event.
#[must_use]
pub fn build_split_resized(
    runtime_id: uuid::Uuid,
    path: Vec<v3::PaneTreeSide>,
    ratio: f32,
    runtime_revision: u64,
) -> v3::SplitResized {
    v3::SplitResized {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        path: path.into_iter().map(|s| s as i32).collect(),
        ratio,
        runtime_revision,
    }
}

/// Build a `ServerEnvelope` response carrying a `SplitResized`.
#[must_use]
pub fn build_split_resized_response(
    request_id: u64,
    resized: v3::SplitResized,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::SplitResized(resized),
    )
}

/// Build a `ServerEnvelope` push event carrying a `SplitResized`.
#[must_use]
pub fn build_split_resized_push(resized: v3::SplitResized) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::SplitResized(resized))
}

/// Build a `FocusChanged` event.
#[must_use]
pub fn build_focus_changed(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    runtime_revision: u64,
) -> v3::FocusChanged {
    v3::FocusChanged {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        runtime_revision,
    }
}

/// Build a `ServerEnvelope` response carrying a `FocusChanged`.
#[must_use]
pub fn build_focus_changed_response(
    request_id: u64,
    changed: v3::FocusChanged,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::FocusChanged(changed),
    )
}

/// Build a `ServerEnvelope` push event carrying a `FocusChanged`.
#[must_use]
pub fn build_focus_changed_push(changed: v3::FocusChanged) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::FocusChanged(changed))
}

/// Decode a repeated `PaneTreeSide` path into a typed list, dropping any
/// `UNSPECIFIED` or out-of-range entries (which never address a real split
/// branch).
#[must_use]
pub fn decode_side_path(raw: &[i32]) -> Vec<v3::PaneTreeSide> {
    raw.iter()
        .copied()
        .filter_map(|v| v3::PaneTreeSide::try_from(v).ok())
        .filter(|s| !matches!(s, v3::PaneTreeSide::Unspecified))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes};
    use bytes::BytesMut;

    fn pid() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    #[test]
    fn leaf_node_carries_pane_id() {
        let p = pid();
        let node = pane_tree_leaf(p);
        let Some(v3::pane_tree_node::Node::Leaf(leaf)) = node.node else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.pane_id, uuid_to_bytes(p));
    }

    #[test]
    fn split_node_carries_axis_ratio_and_children() {
        let a = pid();
        let b = pid();
        let node = pane_tree_split(
            v3::PaneSplitAxis::Vertical,
            0.3,
            pane_tree_leaf(a),
            pane_tree_leaf(b),
        );
        let Some(v3::pane_tree_node::Node::Split(split)) = node.node else {
            panic!("expected split");
        };
        assert_eq!(split.axis, v3::PaneSplitAxis::Vertical as i32);
        assert!((split.ratio - 0.3).abs() < f32::EPSILON);
        assert!(split.first.is_some());
        assert!(split.second.is_some());
    }

    #[test]
    fn nested_tree_round_trips_on_the_wire() {
        let (a, b, c) = (pid(), pid(), pid());
        // Split(a, Split(b, c))
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(a),
            pane_tree_split(v3::PaneSplitAxis::Vertical, 0.4, pane_tree_leaf(b), pane_tree_leaf(c)),
        );
        let mut buf = BytesMut::new();
        encode_frame(&tree, &mut buf).unwrap();
        let decoded: v3::PaneTreeNode = decode_frame(&mut buf).unwrap();
        assert_eq!(tree, decoded);
    }

    #[test]
    fn pane_split_event_is_a_response_not_a_push() {
        let env = build_pane_split_response(
            7,
            build_pane_split(pid(), pid(), pid(), v3::PaneSplitAxis::Horizontal, 0.5, 3),
        );
        assert_eq!(env.request_id, 7);
        assert!(!crate::v3_envelope::is_push_event(&env));
        let Some(v3::server_envelope::Payload::PaneSplit(p)) = env.payload else {
            panic!("expected PaneSplit");
        };
        assert_eq!(p.runtime_revision, 3);
    }

    #[test]
    fn pane_split_push_has_zero_request_id() {
        let env =
            build_pane_split_push(build_pane_split(pid(), pid(), pid(), v3::PaneSplitAxis::Vertical, 0.6, 9));
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn split_resized_round_trips_path_and_ratio() {
        let resized = build_split_resized(
            pid(),
            vec![v3::PaneTreeSide::Second, v3::PaneTreeSide::First],
            0.25,
            12,
        );
        let env = build_split_resized_response(4, resized);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
        let Some(v3::server_envelope::Payload::SplitResized(r)) = decoded.payload else {
            panic!("expected SplitResized");
        };
        assert_eq!(r.path, vec![
            v3::PaneTreeSide::Second as i32,
            v3::PaneTreeSide::First as i32,
        ]);
        assert!((r.ratio - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn focus_changed_carries_pane_and_revision() {
        let p = pid();
        let env = build_focus_changed_response(1, build_focus_changed(pid(), p, 5));
        let Some(v3::server_envelope::Payload::FocusChanged(f)) = env.payload else {
            panic!("expected FocusChanged");
        };
        assert_eq!(f.pane_id, uuid_to_bytes(p));
        assert_eq!(f.runtime_revision, 5);
    }

    #[test]
    fn decode_side_path_keeps_only_real_branches() {
        let raw = vec![
            v3::PaneTreeSide::First as i32,
            v3::PaneTreeSide::Unspecified as i32,
            v3::PaneTreeSide::Second as i32,
            99, // out of range
        ];
        // Unspecified and out-of-range entries are dropped; only First/Second
        // (which address real split branches) survive.
        let decoded = decode_side_path(&raw);
        assert_eq!(decoded, vec![v3::PaneTreeSide::First, v3::PaneTreeSide::Second]);
    }
}

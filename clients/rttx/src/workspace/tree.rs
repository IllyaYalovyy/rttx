//! Render the client layout from the server-authoritative pane tree.
//!
//! RFC-031 §3: the server owns the workspace structure and pane identity; the
//! client is a view. On attach the client builds its entire [`LayoutNode`]
//! render tree from the `WorkspaceSnapshot` tree ([`v3::PaneTreeNode`]), and
//! every render leaf is keyed by the durable, server-assigned pane id. The
//! client mints no structure and no identity here — this is the consumer
//! counterpart to the `rttx_proto::v3_tree` builders.

use super::{LayoutNode, SplitOrientation};
use rttx_proto::v3;

/// Lower bound for a stored split ratio, matching the clamp applied when
/// ratios are pushed to / pulled from live `GtkPaned` positions
/// (`workspace::apply_paned_ratios`). Keeping the stored value in range avoids
/// a degenerate first render before the live clamp runs.
const MIN_RATIO: f64 = 0.05;
/// Upper bound for a stored split ratio (see [`MIN_RATIO`]).
const MAX_RATIO: f64 = 0.95;

/// Build the client render layout from a server pane-tree node.
///
/// Returns `None` for an empty node (a workspace with no panes), in which case
/// the client has no server structure to render.
///
/// Each leaf becomes a [`LayoutNode::Terminal`] whose `uuid` is the canonical
/// string form of the server pane id, so the render tree and the daemon share
/// one identity. A malformed leaf id, or a split missing a child, collapses to
/// whatever valid structure remains rather than discarding the whole tree.
#[must_use]
pub fn layout_from_pane_tree(node: &v3::PaneTreeNode) -> Option<LayoutNode> {
    match node.node.as_ref()? {
        v3::pane_tree_node::Node::Leaf(leaf) => leaf_layout(leaf),
        v3::pane_tree_node::Node::Split(split) => split_layout(split),
    }
}

fn leaf_layout(leaf: &v3::PaneTreeLeaf) -> Option<LayoutNode> {
    let pane_id = rttx_proto::bytes_to_uuid(&leaf.pane_id).ok()?;
    Some(LayoutNode::Terminal {
        uuid: pane_id.to_string(),
        profile: None,
        cwd: None,
        custom_title: None,
    })
}

fn split_layout(split: &v3::PaneTreeSplit) -> Option<LayoutNode> {
    let first = split.first.as_ref().and_then(|n| layout_from_pane_tree(n));
    let second = split.second.as_ref().and_then(|n| layout_from_pane_tree(n));

    match (first, second) {
        (Some(first), Some(second)) => Some(LayoutNode::Split {
            orientation: orientation_from_axis(split.axis),
            ratio: f64::from(split.ratio).clamp(MIN_RATIO, MAX_RATIO),
            first: Box::new(first),
            second: Box::new(second),
        }),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

/// Map a wire split axis to the client orientation.
///
/// `HORIZONTAL` places children side by side (a vertical divider), which is the
/// client's [`SplitOrientation::Horizontal`]. `UNSPECIFIED` and unknown values
/// default to side-by-side: a split always has two children, so a renderable
/// orientation is always required.
fn orientation_from_axis(axis: i32) -> SplitOrientation {
    match v3::PaneSplitAxis::try_from(axis) {
        Ok(v3::PaneSplitAxis::Vertical) => SplitOrientation::Vertical,
        _ => SplitOrientation::Horizontal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};
    use uuid::Uuid;

    fn leaf_uuid(node: &LayoutNode) -> &str {
        match node {
            LayoutNode::Terminal { uuid, .. } => uuid,
            LayoutNode::Split { .. } => panic!("expected a terminal leaf"),
        }
    }

    #[test]
    fn empty_node_renders_nothing() {
        let node = v3::PaneTreeNode { node: None };
        assert!(layout_from_pane_tree(&node).is_none());
    }

    #[test]
    fn leaf_renders_terminal_keyed_by_server_pane_id() {
        let pane = Uuid::new_v4();
        let layout = layout_from_pane_tree(&pane_tree_leaf(pane)).expect("leaf renders");

        match layout {
            LayoutNode::Terminal { uuid, profile, cwd, custom_title } => {
                assert_eq!(uuid, pane.to_string());
                assert!(profile.is_none());
                assert!(cwd.is_none());
                assert!(custom_title.is_none());
            }
            LayoutNode::Split { .. } => panic!("expected a terminal"),
        }
    }

    #[test]
    fn horizontal_axis_maps_to_side_by_side_orientation() {
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { orientation, .. } =
            layout_from_pane_tree(&tree).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert_eq!(orientation, SplitOrientation::Horizontal);
    }

    #[test]
    fn vertical_axis_maps_to_stacked_orientation() {
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Vertical,
            0.5,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { orientation, .. } =
            layout_from_pane_tree(&tree).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert_eq!(orientation, SplitOrientation::Vertical);
    }

    #[test]
    fn unspecified_axis_defaults_to_side_by_side() {
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Unspecified,
            0.5,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { orientation, .. } =
            layout_from_pane_tree(&tree).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert_eq!(orientation, SplitOrientation::Horizontal);
    }

    #[test]
    fn split_preserves_logical_ratio() {
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.3,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { ratio, .. } = layout_from_pane_tree(&tree).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert!((ratio - 0.3).abs() < 1e-6, "ratio {ratio} should be ~0.3");
    }

    #[test]
    fn out_of_range_ratio_is_clamped_to_renderable_bounds() {
        let too_small = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.0,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { ratio, .. } =
            layout_from_pane_tree(&too_small).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert!((ratio - MIN_RATIO).abs() < 1e-6, "ratio {ratio} should clamp to {MIN_RATIO}");

        let too_large = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            1.0,
            pane_tree_leaf(Uuid::new_v4()),
            pane_tree_leaf(Uuid::new_v4()),
        );
        let LayoutNode::Split { ratio, .. } =
            layout_from_pane_tree(&too_large).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert!((ratio - MAX_RATIO).abs() < 1e-6, "ratio {ratio} should clamp to {MAX_RATIO}");
    }

    #[test]
    fn nested_tree_renders_full_structure_with_server_ids() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        // Split(a, Split(b, c)).
        let tree = pane_tree_split(
            v3::PaneSplitAxis::Horizontal,
            0.5,
            pane_tree_leaf(a),
            pane_tree_split(v3::PaneSplitAxis::Vertical, 0.4, pane_tree_leaf(b), pane_tree_leaf(c)),
        );

        let LayoutNode::Split { first, second, .. } =
            layout_from_pane_tree(&tree).expect("split renders")
        else {
            panic!("expected a split");
        };
        assert_eq!(leaf_uuid(&first), a.to_string());

        let LayoutNode::Split { first: inner_first, second: inner_second, orientation, .. } =
            *second
        else {
            panic!("expected a nested split");
        };
        assert_eq!(orientation, SplitOrientation::Vertical);
        assert_eq!(leaf_uuid(&inner_first), b.to_string());
        assert_eq!(leaf_uuid(&inner_second), c.to_string());
    }

    #[test]
    fn malformed_leaf_id_renders_nothing() {
        let node = v3::PaneTreeNode {
            node: Some(v3::pane_tree_node::Node::Leaf(v3::PaneTreeLeaf {
                pane_id: vec![0, 1, 2], // not 16 bytes
            })),
        };
        assert!(layout_from_pane_tree(&node).is_none());
    }

    #[test]
    fn split_with_one_invalid_child_collapses_to_valid_child() {
        let good = Uuid::new_v4();
        let bad = v3::PaneTreeNode {
            node: Some(v3::pane_tree_node::Node::Leaf(v3::PaneTreeLeaf { pane_id: vec![9, 9] })),
        };
        let tree = pane_tree_split(v3::PaneSplitAxis::Horizontal, 0.5, pane_tree_leaf(good), bad);

        let layout = layout_from_pane_tree(&tree).expect("collapses to the valid child");
        assert_eq!(leaf_uuid(&layout), good.to_string());
    }
}

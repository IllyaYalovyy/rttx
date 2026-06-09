//! Authoritative pane tree for a workspace (RFC-031 §1–§2).
//!
//! The server is the single owner of a workspace's structure: a binary tree
//! whose leaves are panes and whose internal nodes are splits carrying a
//! logical ratio. A pane's identity ([`PaneId`]) is server-assigned and
//! immutable for the pane's lifetime (RFC-031 G1); the tree is the single
//! source of truth for arrangement, ordering, split ratios, and the
//! default-active pane (RFC-031 G2).
//!
//! This module is pure data-model logic with no I/O and no GTK. Persistence of
//! the tree (RFC-031 Step 2) and the wire protocol that exposes it
//! (RFC-031 Step 3) build on the types defined here.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server-assigned, immutable pane identifier.
///
/// A `PaneId` is minted exactly once when a pane is created and never changes
/// for that pane's lifetime — across shell respawn, daemon restart, or client
/// reconnect (RFC-031 G1). The type intentionally exposes no setter: there is
/// no API by which a live pane's id can be reassigned, so immutability is
/// guaranteed by shape rather than by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneId(Uuid);

impl PaneId {
    /// Mint a fresh, globally-unique pane id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Adopt an existing UUID as a pane id without minting a new identity.
    ///
    /// Used at the boundary with the current `Uuid`-keyed pane map; it does not
    /// create a new pane identity, it names the existing one.
    #[must_use]
    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// The underlying UUID for storage and wire encoding.
    #[must_use]
    pub const fn uuid(self) -> Uuid {
        self.0
    }
}

impl Default for PaneId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Orientation of a split node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// Children are placed side by side (a vertical divider between them).
    Horizontal,
    /// Children are stacked top and bottom (a horizontal divider between them).
    Vertical,
}

/// One step when addressing a node by its path from the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Descend into the first (left/top) child of a split.
    First,
    /// Descend into the second (right/bottom) child of a split.
    Second,
}

/// The authoritative pane-arrangement tree.
///
/// A leaf names exactly one pane; a split joins two subtrees with a logical
/// `ratio` in the open interval `(0, 1)` describing the fraction of space given
/// to `first`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "node")]
pub enum PaneTree {
    /// A single pane.
    Leaf {
        /// The pane occupying this leaf.
        pane: PaneId,
    },
    /// A split between two subtrees.
    Split {
        /// Split orientation.
        axis: SplitAxis,
        /// Fraction of space allocated to `first`, in `(0, 1)`.
        ratio: f32,
        /// First (left/top) subtree.
        first: Box<Self>,
        /// Second (right/bottom) subtree.
        second: Box<Self>,
    },
}

/// Whether a ratio is a legal split ratio: finite and strictly inside `(0, 1)`.
#[must_use]
pub fn ratio_is_valid(ratio: f32) -> bool {
    ratio.is_finite() && ratio > 0.0 && ratio < 1.0
}

impl PaneTree {
    /// A single-pane tree.
    #[must_use]
    pub const fn leaf(pane: PaneId) -> Self {
        Self::Leaf { pane }
    }

    /// Collect every pane id in left-to-right (in-order) order.
    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf { pane } => out.push(*pane),
            Self::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }

    /// The first (leftmost/topmost) pane in the subtree.
    #[must_use]
    fn first_leaf(&self) -> PaneId {
        let mut node = self;
        loop {
            match node {
                Self::Leaf { pane } => return *pane,
                Self::Split { first, .. } => node = first,
            }
        }
    }

    /// Replace the leaf holding `target` with a split of `target` and
    /// `new_pane`. Returns `true` if `target` was found and split.
    fn split_leaf(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        axis: SplitAxis,
        ratio: f32,
    ) -> bool {
        match self {
            Self::Leaf { pane } if *pane == target => {
                *self = Self::Split {
                    axis,
                    ratio,
                    first: Box::new(Self::leaf(target)),
                    second: Box::new(Self::leaf(new_pane)),
                };
                true
            }
            Self::Leaf { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, axis, ratio)
                    || second.split_leaf(target, new_pane, axis, ratio)
            }
        }
    }

    /// A mutable reference to the node addressed by `path` from this node.
    fn node_at_mut(&mut self, path: &[Side]) -> Option<&mut Self> {
        match path.split_first() {
            None => Some(self),
            Some((side, rest)) => match self {
                Self::Leaf { .. } => None,
                Self::Split { first, second, .. } => {
                    let child = match side {
                        Side::First => first,
                        Side::Second => second,
                    };
                    child.node_at_mut(rest)
                }
            },
        }
    }
}

/// Outcome of removing a pane from a subtree by recursive descent.
enum Removal {
    /// The pane was not in this subtree; the (unchanged) subtree is returned.
    NotFound(PaneTree),
    /// The pane was removed; `Some` carries the collapsed subtree, `None` means
    /// the subtree became empty (its only leaf was the removed pane).
    Removed(Option<PaneTree>),
}

fn remove_from(node: PaneTree, target: PaneId) -> Removal {
    match node {
        PaneTree::Leaf { pane } => {
            if pane == target {
                Removal::Removed(None)
            } else {
                Removal::NotFound(PaneTree::Leaf { pane })
            }
        }
        PaneTree::Split { axis, ratio, first, second } => match remove_from(*first, target) {
            Removal::Removed(None) => Removal::Removed(Some(*second)),
            Removal::Removed(Some(new_first)) => Removal::Removed(Some(PaneTree::Split {
                axis,
                ratio,
                first: Box::new(new_first),
                second,
            })),
            Removal::NotFound(orig_first) => match remove_from(*second, target) {
                Removal::Removed(None) => Removal::Removed(Some(orig_first)),
                Removal::Removed(Some(new_second)) => Removal::Removed(Some(PaneTree::Split {
                    axis,
                    ratio,
                    first: Box::new(orig_first),
                    second: Box::new(new_second),
                })),
                Removal::NotFound(orig_second) => Removal::NotFound(PaneTree::Split {
                    axis,
                    ratio,
                    first: Box::new(orig_first),
                    second: Box::new(orig_second),
                }),
            },
        },
    }
}

/// Result of closing a pane in a [`WorkspaceTree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The pane was removed; the (possibly unchanged) default-active pane is
    /// carried back so callers can mirror focus state.
    Removed {
        /// The default-active pane after removal.
        default_active: PaneId,
    },
    /// The pane was the last leaf; the tree is now empty.
    Emptied,
    /// No leaf held the given pane.
    NotFound,
}

/// A broken tree invariant, reported by [`WorkspaceTree::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeInvariant {
    /// A pane id appeared in more than one leaf.
    DuplicatePane(PaneId),
    /// A split carried a ratio outside the open interval `(0, 1)`.
    InvalidRatio,
    /// `default_active` did not name a pane present in the tree, or a non-empty
    /// tree had no default-active pane (or vice versa).
    DefaultActiveMismatch,
}

/// The server-owned workspace structure: a pane tree plus the default-active
/// pane used as fallback focus on a fresh attach (RFC-031 §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorkspaceTree {
    root: Option<PaneTree>,
    default_active: Option<PaneId>,
}

impl WorkspaceTree {
    /// An empty tree with no panes.
    #[must_use]
    pub const fn new() -> Self {
        Self { root: None, default_active: None }
    }

    /// Whether the tree holds no panes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The root node, if any.
    #[must_use]
    pub const fn root(&self) -> Option<&PaneTree> {
        self.root.as_ref()
    }

    /// The default-active pane, if any.
    #[must_use]
    pub const fn default_active(&self) -> Option<PaneId> {
        self.default_active
    }

    /// Every pane id in left-to-right order.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            root.collect(&mut out);
        }
        out
    }

    /// The number of panes (leaves) in the tree.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.panes().len()
    }

    /// Whether `pane` is present in the tree.
    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        self.panes().contains(&pane)
    }

    /// Seed an empty tree with its first pane. Returns `false` (no change) if
    /// the tree already has a root.
    pub fn insert_root(&mut self, pane: PaneId) -> bool {
        if self.root.is_some() {
            return false;
        }
        self.root = Some(PaneTree::leaf(pane));
        self.default_active = Some(pane);
        true
    }

    /// Split the leaf holding `target`, adding `new_pane` as the second child.
    ///
    /// Returns `false` (no change) if the ratio is invalid, `target` is absent,
    /// or `new_pane` already exists in the tree. The default-active pane is left
    /// unchanged.
    pub fn split(&mut self, target: PaneId, new_pane: PaneId, axis: SplitAxis, ratio: f32) -> bool {
        if !ratio_is_valid(ratio) || self.contains(new_pane) || !self.contains(target) {
            return false;
        }
        self.root.as_mut().is_some_and(|root| root.split_leaf(target, new_pane, axis, ratio))
    }

    /// Remove the pane `pane`, collapsing its parent split into the sibling
    /// subtree. If the removed pane was default-active, the default moves to the
    /// first remaining leaf.
    pub fn close(&mut self, pane: PaneId) -> CloseOutcome {
        let Some(root) = self.root.take() else {
            return CloseOutcome::NotFound;
        };
        match remove_from(root, pane) {
            Removal::NotFound(unchanged) => {
                self.root = Some(unchanged);
                CloseOutcome::NotFound
            }
            Removal::Removed(None) => {
                self.default_active = None;
                CloseOutcome::Emptied
            }
            Removal::Removed(Some(new_root)) => {
                let default_active = if self.default_active == Some(pane) {
                    new_root.first_leaf()
                } else {
                    self.default_active.unwrap_or_else(|| new_root.first_leaf())
                };
                self.default_active = Some(default_active);
                self.root = Some(new_root);
                CloseOutcome::Removed { default_active }
            }
        }
    }

    /// Set the ratio of the split node addressed by `path`.
    ///
    /// Returns `false` (no change) if the ratio is invalid or `path` does not
    /// address an existing split node.
    pub fn resize_split(&mut self, path: &[Side], ratio: f32) -> bool {
        if !ratio_is_valid(ratio) {
            return false;
        }
        let Some(root) = self.root.as_mut() else {
            return false;
        };
        match root.node_at_mut(path) {
            Some(PaneTree::Split { ratio: slot, .. }) => {
                *slot = ratio;
                true
            }
            _ => false,
        }
    }

    /// Make `pane` the default-active pane. Returns `false` (no change) if the
    /// pane is not present in the tree.
    pub fn set_default_active(&mut self, pane: PaneId) -> bool {
        if !self.contains(pane) {
            return false;
        }
        self.default_active = Some(pane);
        true
    }

    /// Check every structural invariant: ratios in `(0, 1)`, no duplicate panes,
    /// and a default-active pane that is present exactly when the tree is
    /// non-empty.
    ///
    /// # Errors
    /// Returns the first violated [`TreeInvariant`].
    pub fn validate(&self) -> Result<(), TreeInvariant> {
        let Some(root) = &self.root else {
            return if self.default_active.is_none() {
                Ok(())
            } else {
                Err(TreeInvariant::DefaultActiveMismatch)
            };
        };

        let mut seen = Vec::new();
        validate_node(root, &mut seen)?;

        match self.default_active {
            Some(active) if seen.contains(&active) => Ok(()),
            _ => Err(TreeInvariant::DefaultActiveMismatch),
        }
    }
}

fn validate_node(node: &PaneTree, seen: &mut Vec<PaneId>) -> Result<(), TreeInvariant> {
    match node {
        PaneTree::Leaf { pane } => {
            if seen.contains(pane) {
                return Err(TreeInvariant::DuplicatePane(*pane));
            }
            seen.push(*pane);
            Ok(())
        }
        PaneTree::Split { ratio, first, second, .. } => {
            if !ratio_is_valid(*ratio) {
                return Err(TreeInvariant::InvalidRatio);
            }
            validate_node(first, seen)?;
            validate_node(second, seen)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid() -> PaneId {
        PaneId::new()
    }

    #[test]
    fn pane_id_round_trips_through_uuid_without_minting() {
        let raw = Uuid::new_v4();
        let id = PaneId::from_uuid(raw);
        assert_eq!(id.uuid(), raw);
        // Adopting the same UUID twice yields equal ids — no new identity.
        assert_eq!(id, PaneId::from_uuid(raw));
    }

    #[test]
    fn new_pane_ids_are_unique() {
        assert_ne!(PaneId::new(), PaneId::new());
    }

    #[test]
    fn empty_tree_is_empty_and_valid() {
        let tree = WorkspaceTree::new();
        assert!(tree.is_empty());
        assert_eq!(tree.leaf_count(), 0);
        assert_eq!(tree.default_active(), None);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn insert_root_seeds_tree_and_default_active() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        assert!(tree.insert_root(a));
        assert!(!tree.is_empty());
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(tree.default_active(), Some(a));
        assert_eq!(tree.root(), Some(&PaneTree::leaf(a)));
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn insert_root_is_rejected_when_not_empty() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        assert!(tree.insert_root(a));
        assert!(!tree.insert_root(b));
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(tree.default_active(), Some(a));
    }

    #[test]
    fn split_replaces_target_leaf_with_split_node() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        assert!(tree.split(a, b, SplitAxis::Horizontal, 0.5));
        assert_eq!(tree.leaf_count(), 2);
        assert!(tree.contains(a));
        assert!(tree.contains(b));
        // default-active is unchanged by a split.
        assert_eq!(tree.default_active(), Some(a));
        assert_eq!(
            tree.root(),
            Some(&PaneTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(PaneTree::leaf(a)),
                second: Box::new(PaneTree::leaf(b)),
            })
        );
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn split_keeps_target_pane_id_stable() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        let before = tree.panes();
        tree.split(a, b, SplitAxis::Vertical, 0.3);
        // The original pane id still exists after splitting (immutable identity).
        assert!(tree.panes().contains(&before[0]));
        assert!(tree.contains(a));
    }

    #[test]
    fn nested_split_targets_correct_leaf() {
        let mut tree = WorkspaceTree::new();
        let (a, b, c) = (pid(), pid(), pid());
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        assert!(tree.split(b, c, SplitAxis::Vertical, 0.4));
        assert_eq!(tree.leaf_count(), 3);
        assert_eq!(tree.panes(), vec![a, b, c]);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn split_rejects_unknown_target() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        assert!(!tree.split(pid(), pid(), SplitAxis::Horizontal, 0.5));
        assert_eq!(tree.leaf_count(), 1);
    }

    #[test]
    fn split_rejects_duplicate_new_pane() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        // b already exists — splitting a with b again must be rejected.
        assert!(!tree.split(a, b, SplitAxis::Horizontal, 0.5));
        assert_eq!(tree.leaf_count(), 2);
    }

    #[test]
    fn split_rejects_invalid_ratios() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        for bad in [0.0, 1.0, -0.1, 1.5, f32::NAN, f32::INFINITY] {
            assert!(!tree.split(a, pid(), SplitAxis::Horizontal, bad), "ratio {bad} accepted");
        }
        assert_eq!(tree.leaf_count(), 1);
    }

    #[test]
    fn close_collapses_parent_into_sibling() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        assert_eq!(tree.close(a), CloseOutcome::Removed { default_active: b });
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(tree.root(), Some(&PaneTree::leaf(b)));
        assert_eq!(tree.default_active(), Some(b));
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn close_non_default_pane_keeps_default_active() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        // default-active is a; closing b must not move it.
        assert_eq!(tree.close(b), CloseOutcome::Removed { default_active: a });
        assert_eq!(tree.default_active(), Some(a));
    }

    #[test]
    fn close_last_leaf_empties_tree() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        assert_eq!(tree.close(a), CloseOutcome::Emptied);
        assert!(tree.is_empty());
        assert_eq!(tree.default_active(), None);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn close_unknown_pane_is_not_found() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        assert_eq!(tree.close(pid()), CloseOutcome::NotFound);
        assert_eq!(tree.leaf_count(), 1);
    }

    #[test]
    fn close_on_empty_tree_is_not_found() {
        let mut tree = WorkspaceTree::new();
        assert_eq!(tree.close(pid()), CloseOutcome::NotFound);
    }

    #[test]
    fn close_deep_nested_pane_collapses_correctly() {
        let mut tree = WorkspaceTree::new();
        let (a, b, c) = (pid(), pid(), pid());
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        tree.split(b, c, SplitAxis::Vertical, 0.5);
        // tree: Split(a, Split(b, c)). Removing b collapses inner split to c.
        assert!(matches!(tree.close(b), CloseOutcome::Removed { .. }));
        assert_eq!(tree.panes(), vec![a, c]);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn resize_split_updates_root_ratio() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        assert!(tree.resize_split(&[], 0.25));
        let PaneTree::Split { ratio, .. } = tree.root().unwrap() else {
            panic!("root should be a split");
        };
        assert!((ratio - 0.25).abs() < f32::EPSILON);
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn resize_split_navigates_path_to_nested_split() {
        let mut tree = WorkspaceTree::new();
        let (a, b, c) = (pid(), pid(), pid());
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        tree.split(b, c, SplitAxis::Vertical, 0.5);
        // root.second is the inner split.
        assert!(tree.resize_split(&[Side::Second], 0.7));
        let PaneTree::Split { second, .. } = tree.root().unwrap() else {
            panic!("root should be a split");
        };
        let PaneTree::Split { ratio, .. } = second.as_ref() else {
            panic!("second child should be a split");
        };
        assert!((ratio - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn resize_split_rejects_invalid_ratio() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        assert!(!tree.resize_split(&[], 0.0));
        assert!(!tree.resize_split(&[], 1.0));
    }

    #[test]
    fn resize_split_rejects_path_to_leaf() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        // root.first is a leaf, not a split.
        assert!(!tree.resize_split(&[Side::First], 0.3));
    }

    #[test]
    fn resize_split_rejects_path_past_leaf() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        assert!(!tree.resize_split(&[Side::First], 0.3));
    }

    #[test]
    fn set_default_active_moves_focus_to_existing_pane() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        let b = pid();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.5);
        assert!(tree.set_default_active(b));
        assert_eq!(tree.default_active(), Some(b));
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn set_default_active_rejects_absent_pane() {
        let mut tree = WorkspaceTree::new();
        let a = pid();
        tree.insert_root(a);
        assert!(!tree.set_default_active(pid()));
        assert_eq!(tree.default_active(), Some(a));
    }

    #[test]
    fn validate_detects_invalid_ratio() {
        let a = pid();
        let b = pid();
        let tree = WorkspaceTree {
            root: Some(PaneTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 1.5,
                first: Box::new(PaneTree::leaf(a)),
                second: Box::new(PaneTree::leaf(b)),
            }),
            default_active: Some(a),
        };
        assert_eq!(tree.validate(), Err(TreeInvariant::InvalidRatio));
    }

    #[test]
    fn validate_detects_duplicate_pane() {
        let a = pid();
        let tree = WorkspaceTree {
            root: Some(PaneTree::Split {
                axis: SplitAxis::Horizontal,
                ratio: 0.5,
                first: Box::new(PaneTree::leaf(a)),
                second: Box::new(PaneTree::leaf(a)),
            }),
            default_active: Some(a),
        };
        assert_eq!(tree.validate(), Err(TreeInvariant::DuplicatePane(a)));
    }

    #[test]
    fn validate_detects_default_active_not_in_tree() {
        let a = pid();
        let tree = WorkspaceTree { root: Some(PaneTree::leaf(a)), default_active: Some(pid()) };
        assert_eq!(tree.validate(), Err(TreeInvariant::DefaultActiveMismatch));
    }

    #[test]
    fn validate_detects_default_active_on_empty_tree() {
        let tree = WorkspaceTree { root: None, default_active: Some(pid()) };
        assert_eq!(tree.validate(), Err(TreeInvariant::DefaultActiveMismatch));
    }

    #[test]
    fn many_splits_then_closes_preserves_invariants() {
        let mut tree = WorkspaceTree::new();
        let root = pid();
        tree.insert_root(root);
        let mut leaves = vec![root];
        for _ in 0..16 {
            let target = leaves[leaves.len() / 2];
            let new = pid();
            assert!(tree.split(target, new, SplitAxis::Horizontal, 0.5));
            leaves.push(new);
            assert!(tree.validate().is_ok());
            assert_eq!(tree.leaf_count(), leaves.len());
        }
        // Close every pane except the last; tree stays valid throughout.
        while leaves.len() > 1 {
            let victim = leaves.remove(0);
            assert!(matches!(tree.close(victim), CloseOutcome::Removed { .. }));
            assert!(tree.validate().is_ok());
            assert!(!tree.contains(victim));
        }
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(tree.close(leaves[0]), CloseOutcome::Emptied);
        assert!(tree.is_empty());
    }
}

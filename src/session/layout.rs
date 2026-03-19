use serde::{Deserialize, Serialize};

/// Represents the layout tree of terminals within a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LayoutNode {
    Terminal {
        uuid: String,
        profile: Option<String>,
        cwd: Option<String>,
        custom_title: Option<String>,
    },
    Split {
        orientation: SplitOrientation,
        /// Position ratio 0.0-1.0 of the divider
        ratio: f64,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

/// Persistent state of a single session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    pub uuid: String,
    pub name: String,
    pub layout: LayoutNode,
}

/// Persistent state of the entire application window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowState {
    pub sessions: Vec<SessionState>,
    pub active_session_index: usize,
    pub width: i32,
    pub height: i32,
    pub is_maximized: bool,
}

impl LayoutNode {
    pub fn new_terminal() -> Self {
        LayoutNode::Terminal {
            uuid: uuid::Uuid::new_v4().to_string(),
            profile: None,
            cwd: None,
            custom_title: None,
        }
    }

    pub fn split(self, orientation: SplitOrientation) -> Self {
        LayoutNode::Split {
            orientation,
            ratio: 0.5,
            first: Box::new(self),
            second: Box::new(LayoutNode::new_terminal()),
        }
    }

    pub fn terminal_count(&self) -> usize {
        match self {
            LayoutNode::Terminal { .. } => 1,
            LayoutNode::Split { first, second, .. } => {
                first.terminal_count() + second.terminal_count()
            }
        }
    }

    pub fn remove_terminal(&self, target_uuid: &str) -> Option<LayoutNode> {
        match self {
            LayoutNode::Terminal { uuid, .. } => {
                if uuid == target_uuid {
                    None
                } else {
                    Some(self.clone())
                }
            }
            LayoutNode::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                if matches!(first.as_ref(), LayoutNode::Terminal { uuid, .. } if uuid == target_uuid)
                {
                    return Some(*second.clone());
                }
                if matches!(second.as_ref(), LayoutNode::Terminal { uuid, .. } if uuid == target_uuid)
                {
                    return Some(*first.clone());
                }
                let new_first = first.remove_terminal(target_uuid)?;
                let new_second = second.remove_terminal(target_uuid)?;
                Some(LayoutNode::Split {
                    orientation: *orientation,
                    ratio: *ratio,
                    first: Box::new(new_first),
                    second: Box::new(new_second),
                })
            }
        }
    }

    pub fn terminal_uuids(&self) -> Vec<String> {
        match self {
            LayoutNode::Terminal { uuid, .. } => vec![uuid.clone()],
            LayoutNode::Split { first, second, .. } => {
                let mut uuids = first.terminal_uuids();
                uuids.extend(second.terminal_uuids());
                uuids
            }
        }
    }

    pub fn split_terminal(
        &self,
        target_uuid: &str,
        orientation: SplitOrientation,
    ) -> Option<LayoutNode> {
        match self {
            LayoutNode::Terminal { uuid, .. } if uuid == target_uuid => {
                Some(self.clone().split(orientation))
            }
            LayoutNode::Terminal { .. } => None,
            LayoutNode::Split {
                orientation: ori,
                ratio,
                first,
                second,
            } => {
                if let Some(new_first) = first.split_terminal(target_uuid, orientation) {
                    Some(LayoutNode::Split {
                        orientation: *ori,
                        ratio: *ratio,
                        first: Box::new(new_first),
                        second: second.clone(),
                    })
                } else {
                    second
                        .split_terminal(target_uuid, orientation)
                        .map(|new_second| LayoutNode::Split {
                            orientation: *ori,
                            ratio: *ratio,
                            first: first.clone(),
                            second: Box::new(new_second),
                        })
                }
            }
        }
    }

    /// Returns the depth of the layout tree (1 for a single terminal).
    pub fn depth(&self) -> usize {
        match self {
            LayoutNode::Terminal { .. } => 1,
            LayoutNode::Split { first, second, .. } => {
                1 + first.depth().max(second.depth())
            }
        }
    }

    /// Check if a terminal UUID exists in the tree.
    pub fn contains_terminal(&self, target_uuid: &str) -> bool {
        match self {
            LayoutNode::Terminal { uuid, .. } => uuid == target_uuid,
            LayoutNode::Split { first, second, .. } => {
                first.contains_terminal(target_uuid) || second.contains_terminal(target_uuid)
            }
        }
    }
}

impl SessionState {
    pub fn new(name: impl Into<String>) -> Self {
        SessionState {
            uuid: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            layout: LayoutNode::new_terminal(),
        }
    }
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            sessions: vec![SessionState::new("Session 1")],
            active_session_index: 0,
            width: 900,
            height: 600,
            is_maximized: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // ── Terminal creation ─────────────────────────────────────────

    #[test]
    fn new_terminal_has_valid_uuid() {
        let node = LayoutNode::new_terminal();
        if let LayoutNode::Terminal { uuid, .. } = &node {
            assert!(uuid::Uuid::parse_str(uuid).is_ok());
        } else {
            panic!("Expected Terminal variant");
        }
    }

    #[test]
    fn two_new_terminals_have_different_uuids() {
        let a = LayoutNode::new_terminal();
        let b = LayoutNode::new_terminal();
        assert_ne!(a.terminal_uuids(), b.terminal_uuids());
    }

    // ── Splitting ────────────────────────────────────────────────

    #[rstest]
    #[case(SplitOrientation::Horizontal)]
    #[case(SplitOrientation::Vertical)]
    fn split_creates_two_children(#[case] orientation: SplitOrientation) {
        let node = term("t1");
        let split = node.split(orientation);
        assert_eq!(split.terminal_count(), 2);
        if let LayoutNode::Split { orientation: o, ratio, .. } = &split {
            assert_eq!(*o, orientation);
            assert!((ratio - 0.5).abs() < f64::EPSILON);
        } else {
            panic!("Expected Split variant");
        }
    }

    #[test]
    fn split_preserves_original_terminal() {
        let node = term("original");
        let split = node.split(SplitOrientation::Horizontal);
        assert!(split.contains_terminal("original"));
    }

    #[test]
    fn nested_splits_count_correctly() {
        // (t1 | t2) / t3
        let layout = vsplit(hsplit(term("t1"), term("t2")), term("t3"));
        assert_eq!(layout.terminal_count(), 3);
        assert_eq!(layout.depth(), 3);
    }

    #[rstest]
    // build_chain produces a left-leaning tree:
    // 1 terminal  → depth 1
    // 2 terminals → hsplit(t0, t1) → depth 2
    // 3 terminals → hsplit(hsplit(t0, t1), t2) → depth 3
    // 4 terminals → hsplit(hsplit(hsplit(t0,t1),t2),t3) → depth 4
    #[case(1, 1)]
    #[case(2, 2)]
    #[case(3, 3)]
    #[case(4, 4)]
    #[case(7, 7)]
    fn depth_matches_expected(#[case] num_terminals: usize, #[case] expected_depth: usize) {
        let layout = build_chain(num_terminals);
        assert_eq!(layout.terminal_count(), num_terminals);
        assert_eq!(layout.depth(), expected_depth);
    }

    /// Build a left-leaning chain of splits with n terminals.
    fn build_chain(n: usize) -> LayoutNode {
        if n <= 1 {
            return term("t0");
        }
        let mut layout = hsplit(term("t0"), term("t1"));
        for i in 2..n {
            layout = hsplit(layout, term(&format!("t{i}")));
        }
        layout
    }

    // ── Split specific terminal ──────────────────────────────────

    #[rstest]
    #[case("t1", SplitOrientation::Horizontal, 3)]
    #[case("t2", SplitOrientation::Vertical, 3)]
    fn split_specific_terminal_in_tree(
        #[case] target: &str,
        #[case] orientation: SplitOrientation,
        #[case] expected_count: usize,
    ) {
        let layout = hsplit(term("t1"), term("t2"));
        let result = layout.split_terminal(target, orientation);
        assert!(result.is_some());
        assert_eq!(result.unwrap().terminal_count(), expected_count);
    }

    #[test]
    fn split_nonexistent_terminal_returns_none() {
        let layout = hsplit(term("t1"), term("t2"));
        assert!(layout.split_terminal("ghost", SplitOrientation::Horizontal).is_none());
    }

    #[test]
    fn split_deeply_nested_terminal() {
        // ((t1 | t2) / t3) | t4
        let layout = hsplit(vsplit(hsplit(term("t1"), term("t2")), term("t3")), term("t4"));
        let result = layout.split_terminal("t1", SplitOrientation::Vertical);
        assert!(result.is_some());
        let new_layout = result.unwrap();
        assert_eq!(new_layout.terminal_count(), 5);
        assert!(new_layout.contains_terminal("t1"));
    }

    // ── Remove terminal ──────────────────────────────────────────

    #[test]
    fn remove_first_child_returns_second() {
        let layout = hsplit(term("t1"), term("t2"));
        let result = layout.remove_terminal("t1").unwrap();
        assert_eq!(result.terminal_count(), 1);
        assert!(result.contains_terminal("t2"));
        assert!(!result.contains_terminal("t1"));
    }

    #[test]
    fn remove_second_child_returns_first() {
        let layout = hsplit(term("t1"), term("t2"));
        let result = layout.remove_terminal("t2").unwrap();
        assert!(result.contains_terminal("t1"));
    }

    #[test]
    fn remove_from_nested_preserves_structure() {
        // (t1 | t2) / t3 → remove t2 → t1 / t3
        let layout = vsplit(hsplit(term("t1"), term("t2")), term("t3"));
        let result = layout.remove_terminal("t2").unwrap();
        assert_eq!(result.terminal_count(), 2);
        assert!(result.contains_terminal("t1"));
        assert!(result.contains_terminal("t3"));
    }

    #[test]
    fn remove_nonexistent_returns_unchanged() {
        let layout = hsplit(term("t1"), term("t2"));
        let result = layout.remove_terminal("ghost").unwrap();
        assert_eq!(result.terminal_count(), 2);
    }

    #[test]
    fn remove_root_terminal_returns_none() {
        let layout = term("t1");
        assert!(layout.remove_terminal("t1").is_none());
    }

    // ── UUID collection ──────────────────────────────────────────

    #[test]
    fn terminal_uuids_are_unique() {
        let layout = hsplit(vsplit(term("a"), term("b")), hsplit(term("c"), term("d")));
        let uuids = layout.terminal_uuids();
        assert_eq!(uuids.len(), 4);
        let unique: std::collections::HashSet<_> = uuids.iter().collect();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn contains_terminal_works() {
        let layout = hsplit(term("t1"), vsplit(term("t2"), term("t3")));
        assert!(layout.contains_terminal("t1"));
        assert!(layout.contains_terminal("t2"));
        assert!(layout.contains_terminal("t3"));
        assert!(!layout.contains_terminal("t4"));
    }

    // ── Serialization ────────────────────────────────────────────

    #[test]
    fn session_state_roundtrip() {
        let s = session("s1", "My Session", hsplit(term("t1"), term("t2")));
        let json = serde_json::to_string(&s).unwrap();
        let deserialized: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(s, deserialized);
    }

    #[test]
    fn window_state_roundtrip() {
        let state = window_state(vec![
            session("s1", "Session 1", term("t1")),
            session("s2", "Session 2", hsplit(term("t2"), vsplit(term("t3"), term("t4")))),
        ]);
        let json = serde_json::to_string_pretty(&state).unwrap();
        let deserialized: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deserialized);
    }

    #[test]
    fn complex_layout_roundtrip() {
        let layout = hsplit(
            vsplit(
                hsplit(
                    term_full("t1", "/home/user", "build"),
                    term_full("t2", "/tmp", "logs"),
                ),
                term("t3"),
            ),
            split_ratio(SplitOrientation::Horizontal, 0.7, term("t4"), term("t5")),
        );
        assert_eq!(layout.terminal_count(), 5);
        let json = serde_json::to_string(&layout).unwrap();
        let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, deserialized);
    }

    #[test]
    fn default_window_state_is_valid() {
        let state = WindowState::default();
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session_index, 0);
        assert!(state.width > 0);
        assert!(state.height > 0);
        assert_eq!(state.sessions[0].layout.terminal_count(), 1);
    }

    // ── Invariant: split then remove restores count ──────────────

    #[test]
    fn split_then_remove_new_restores_original_count() {
        let layout = hsplit(term("t1"), term("t2"));
        let original_count = layout.terminal_count();

        // Split t1
        let after_split = layout.split_terminal("t1", SplitOrientation::Vertical).unwrap();
        assert_eq!(after_split.terminal_count(), original_count + 1);

        // Find the new terminal (the one that's not t1 or t2)
        let new_uuid = after_split
            .terminal_uuids()
            .into_iter()
            .find(|u| u != "t1" && u != "t2")
            .unwrap();

        // Remove it
        let after_remove = after_split.remove_terminal(&new_uuid).unwrap();
        assert_eq!(after_remove.terminal_count(), original_count);
    }
}

// ── Property-based tests ─────────────────────────────────────────

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Global counter to ensure unique UUIDs across proptest runs.
    static UUID_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_id() -> String {
        format!("t{}", UUID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    fn arb_orientation() -> impl Strategy<Value = SplitOrientation> {
        prop_oneof![
            Just(SplitOrientation::Horizontal),
            Just(SplitOrientation::Vertical),
        ]
    }

    /// Generate a layout tree with guaranteed unique UUIDs.
    /// We use a counter-based approach instead of random IDs.
    fn arb_layout(max_depth: u32) -> impl Strategy<Value = LayoutNode> {
        let leaf = Just(()).prop_map(|_| LayoutNode::Terminal {
            uuid: unique_id(),
            profile: None,
            cwd: None,
            custom_title: None,
        });

        leaf.prop_recursive(max_depth, 64, 2, |inner| {
            (arb_orientation(), 0.1f64..0.9, inner.clone(), inner).prop_map(
                |(orientation, ratio, first, second)| LayoutNode::Split {
                    orientation,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                },
            )
        })
    }

    /// Compare two layouts with f64 tolerance for ratios.
    fn layouts_equal_approx(a: &LayoutNode, b: &LayoutNode) -> bool {
        match (a, b) {
            (
                LayoutNode::Terminal { uuid: ua, profile: pa, cwd: ca, custom_title: ta },
                LayoutNode::Terminal { uuid: ub, profile: pb, cwd: cb, custom_title: tb },
            ) => ua == ub && pa == pb && ca == cb && ta == tb,
            (
                LayoutNode::Split { orientation: oa, ratio: ra, first: fa, second: sa },
                LayoutNode::Split { orientation: ob, ratio: rb, first: fb, second: sb },
            ) => oa == ob && (ra - rb).abs() < 1e-14 && layouts_equal_approx(fa, fb) && layouts_equal_approx(sa, sb),
            _ => false,
        }
    }

    proptest! {
        /// terminal_count always equals the number of UUIDs collected.
        #[test]
        fn count_equals_uuid_count(layout in arb_layout(4)) {
            prop_assert_eq!(layout.terminal_count(), layout.terminal_uuids().len());
        }

        /// Serialization roundtrip preserves the layout (with f64 tolerance).
        #[test]
        fn serde_roundtrip(layout in arb_layout(4)) {
            let json = serde_json::to_string(&layout).unwrap();
            let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
            prop_assert!(layouts_equal_approx(&layout, &deserialized));
        }

        /// Splitting any terminal increases count by exactly 1.
        #[test]
        fn split_increases_count_by_one(
            layout in arb_layout(3),
            orientation in arb_orientation(),
        ) {
            let uuids = layout.terminal_uuids();
            if let Some(target) = uuids.first() {
                let original_count = layout.terminal_count();
                if let Some(new_layout) = layout.split_terminal(target, orientation) {
                    prop_assert_eq!(new_layout.terminal_count(), original_count + 1);
                    prop_assert!(new_layout.contains_terminal(target));
                }
            }
        }

        /// Removing a terminal from a multi-terminal layout decreases count by exactly 1.
        #[test]
        fn remove_decreases_count_by_one(layout in arb_layout(3)) {
            if layout.terminal_count() >= 2 {
                let uuids = layout.terminal_uuids();
                // All UUIDs should be unique with our generator
                let unique: std::collections::HashSet<_> = uuids.iter().collect();
                prop_assert_eq!(uuids.len(), unique.len(), "UUIDs must be unique");

                if let Some(target) = uuids.first() {
                    if let Some(new_layout) = layout.remove_terminal(target) {
                        prop_assert_eq!(
                            new_layout.terminal_count(),
                            layout.terminal_count() - 1
                        );
                        prop_assert!(!new_layout.contains_terminal(target));
                    }
                }
            }
        }

        /// Depth is always >= 1.
        #[test]
        fn depth_at_least_one(layout in arb_layout(4)) {
            prop_assert!(layout.depth() >= 1);
        }

        /// Split ratio is preserved through serialization.
        #[test]
        fn ratio_preserved(ratio in 0.1f64..0.9) {
            let layout = LayoutNode::Split {
                orientation: SplitOrientation::Horizontal,
                ratio,
                first: Box::new(LayoutNode::Terminal {
                    uuid: unique_id(), profile: None, cwd: None, custom_title: None,
                }),
                second: Box::new(LayoutNode::Terminal {
                    uuid: unique_id(), profile: None, cwd: None, custom_title: None,
                }),
            };
            let json = serde_json::to_string(&layout).unwrap();
            let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
            if let LayoutNode::Split { ratio: r, .. } = deserialized {
                prop_assert!((r - ratio).abs() < 1e-10);
            }
        }

        /// All UUIDs in a generated tree are unique.
        #[test]
        fn all_uuids_unique(layout in arb_layout(4)) {
            let uuids = layout.terminal_uuids();
            let unique: std::collections::HashSet<_> = uuids.iter().collect();
            prop_assert_eq!(uuids.len(), unique.len());
        }
    }
}

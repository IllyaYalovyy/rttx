use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Represents the layout tree of terminals within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LayoutNode {
    Terminal {
        uuid: String,
        profile: Option<String>,
        cwd: Option<String>,
        custom_title: Option<String>,
    },
    Split {
        orientation: SplitOrientation,
        ratio: f64,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl PartialEq for LayoutNode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Terminal { uuid: u1, profile: p1, cwd: c1, custom_title: t1 },
                Self::Terminal { uuid: u2, profile: p2, cwd: c2, custom_title: t2 },
            ) => u1 == u2 && p1 == p2 && c1 == c2 && t1 == t2,
            (
                Self::Split { orientation: o1, ratio: r1, first: f1, second: s1 },
                Self::Split { orientation: o2, ratio: r2, first: f2, second: s2 },
            ) => o1 == o2 && (r1 - r2).abs() < f64::EPSILON && f1 == f2 && s1 == s2,
            _ => false,
        }
    }
}

impl Eq for LayoutNode {}

/// Orientation of a terminal split.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneSource {
    EmptyShell,
    Bookmark { name: String },
    Command { title: String },
    SessionTemplate { name: String },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupStep {
    SendText { text: String, execute: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneTarget {
    LocalFolder { path: String },
    LocalTmux { session: String },
    RemoteShell { ssh_target: String, remote_folder: Option<String> },
    RemoteTmux { ssh_target: String, tmux_session: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneRecovery {
    pub source: PaneSource,
    #[serde(default)]
    pub target: Option<PaneTarget>,
    #[serde(default)]
    pub startup: Vec<StartupStep>,
}

impl PaneRecovery {
    #[must_use]
    pub const fn empty_shell() -> Self {
        Self { source: PaneSource::EmptyShell, target: None, startup: Vec::new() }
    }
}

impl PaneTarget {
    #[must_use]
    pub const fn initial_cwd(&self) -> Option<&str> {
        match self {
            Self::LocalFolder { path } => Some(path.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn managed_startup_input(&self) -> Option<String> {
        match self {
            Self::LocalFolder { .. } => None,
            Self::LocalTmux { session } => Some(format!("exec {}\n", tmux_attach_command(session))),
            Self::RemoteShell { ssh_target, remote_folder } => {
                let remote_command = remote_folder.as_deref().map(remote_shell_command);
                Some(format!("exec {}\n", ssh_exec_command(ssh_target, remote_command.as_deref())))
            }
            Self::RemoteTmux { ssh_target, tmux_session } => Some(format!(
                "exec {}\n",
                ssh_exec_command(ssh_target, Some(&tmux_attach_command(tmux_session)))
            )),
        }
    }

    #[must_use]
    pub const fn manages_child_lifecycle(&self) -> bool {
        !matches!(self, Self::LocalFolder { .. })
    }

    #[must_use]
    pub fn failure_message(&self, status: i32) -> String {
        match self {
            Self::LocalFolder { path } => {
                format!("Failed to open local folder {path} (exit status {status})")
            }
            Self::LocalTmux { session } => {
                format!("Failed to attach local tmux session {session} (exit status {status})")
            }
            Self::RemoteShell { ssh_target, .. } => {
                format!("Failed to connect to {ssh_target} (exit status {status})")
            }
            Self::RemoteTmux { ssh_target, tmux_session } => {
                format!(
                    "Failed to attach tmux session {tmux_session} on {ssh_target} (exit status {status})"
                )
            }
        }
    }
}

impl StartupStep {
    #[must_use]
    pub fn terminal_input(&self) -> String {
        match self {
            Self::SendText { text, execute } => {
                if *execute {
                    format!("{text}\n")
                } else {
                    text.clone()
                }
            }
        }
    }
}

fn remote_shell_command(path: &str) -> String {
    format!("cd {} && exec ${{SHELL:-/bin/bash}} -l", shell_quote(path))
}

fn tmux_attach_command(session: &str) -> String {
    format!("tmux attach-session -t {}", shell_quote(session))
}

fn ssh_exec_command(ssh_target: &str, remote_command: Option<&str>) -> String {
    remote_command.map_or_else(
        || format!("ssh {ssh_target}"),
        |remote_command| format!("ssh -t {ssh_target} {}", shell_quote(remote_command)),
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl LayoutNode {
    #[must_use]
    pub fn new_terminal() -> Self {
        Self::Terminal {
            uuid: uuid::Uuid::new_v4().to_string(),
            profile: None,
            cwd: None,
            custom_title: None,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn new_terminal_with_uuid(uuid: &str) -> Self {
        Self::Terminal { uuid: uuid.to_string(), profile: None, cwd: None, custom_title: None }
    }

    #[must_use]
    pub fn split(&self, orientation: SplitOrientation) -> Self {
        Self::Split {
            orientation,
            ratio: 0.5,
            first: Box::new(self.clone()),
            second: Box::new(Self::new_terminal()),
        }
    }

    #[must_use]
    pub fn terminal_count(&self) -> usize {
        match self {
            Self::Terminal { .. } => 1,
            Self::Split { first, second, .. } => first.terminal_count() + second.terminal_count(),
        }
    }

    #[must_use]
    pub fn terminal_uuids(&self) -> Vec<String> {
        match self {
            Self::Terminal { uuid, .. } => vec![uuid.clone()],
            Self::Split { first, second, .. } => {
                let mut uuids = first.terminal_uuids();
                uuids.extend(second.terminal_uuids());
                uuids
            }
        }
    }

    #[must_use]
    pub fn contains_terminal(&self, uuid: &str) -> bool {
        match self {
            Self::Terminal { uuid: u, .. } => u == uuid,
            Self::Split { first, second, .. } => {
                first.contains_terminal(uuid) || second.contains_terminal(uuid)
            }
        }
    }

    #[must_use]
    pub fn split_terminal(&self, target_uuid: &str, orientation: SplitOrientation) -> Option<Self> {
        match self {
            Self::Terminal { uuid, .. } if uuid == target_uuid => Some(self.split(orientation)),
            Self::Terminal { .. } => None,
            Self::Split { orientation: o, ratio, first, second } => {
                first.split_terminal(target_uuid, orientation).map_or_else(
                    || {
                        second.split_terminal(target_uuid, orientation).map(|new_second| {
                            Self::Split {
                                orientation: *o,
                                ratio: *ratio,
                                first: first.clone(),
                                second: Box::new(new_second),
                            }
                        })
                    },
                    |new_first| {
                        Some(Self::Split {
                            orientation: *o,
                            ratio: *ratio,
                            first: Box::new(new_first),
                            second: second.clone(),
                        })
                    },
                )
            }
        }
    }

    #[must_use]
    pub fn split_terminal_with_new_uuid(
        &self,
        target_uuid: &str,
        orientation: SplitOrientation,
    ) -> Option<(Self, String)> {
        match self {
            Self::Terminal { uuid, .. } if uuid == target_uuid => {
                let new_node = self.split(orientation);
                if let Self::Split { ref second, .. } = new_node {
                    if let Self::Terminal { uuid: new_uuid, .. } = second.as_ref() {
                        return Some((new_node.clone(), new_uuid.clone()));
                    }
                }
                None
            }
            Self::Terminal { .. } => None,
            Self::Split { orientation: o, ratio, first, second } => {
                first.split_terminal_with_new_uuid(target_uuid, orientation).map_or_else(
                    || {
                        second.split_terminal_with_new_uuid(target_uuid, orientation).map(
                            |(new_second, new_uuid)| {
                                (
                                    Self::Split {
                                        orientation: *o,
                                        ratio: *ratio,
                                        first: first.clone(),
                                        second: Box::new(new_second),
                                    },
                                    new_uuid,
                                )
                            },
                        )
                    },
                    |(new_first, new_uuid)| {
                        Some((
                            Self::Split {
                                orientation: *o,
                                ratio: *ratio,
                                first: Box::new(new_first),
                                second: second.clone(),
                            },
                            new_uuid,
                        ))
                    },
                )
            }
        }
    }

    #[must_use]
    pub fn remove_terminal(&self, target_uuid: &str) -> Option<Self> {
        match self {
            Self::Terminal { uuid, .. } if uuid == target_uuid => None,
            Self::Terminal { .. } => Some(self.clone()),
            Self::Split { orientation, ratio, first, second } => {
                let new_first = first.remove_terminal(target_uuid);
                let new_second = second.remove_terminal(target_uuid);

                match (new_first, new_second) {
                    (None, None) => None,
                    (None, Some(s)) => Some(s),
                    (Some(f), None) => Some(f),
                    (Some(f), Some(s)) => Some(Self::Split {
                        orientation: *orientation,
                        ratio: *ratio,
                        first: Box::new(f),
                        second: Box::new(s),
                    }),
                }
            }
        }
    }

    pub fn swap_terminals(&mut self, a: &str, b: &str) {
        if a == b {
            return;
        }
        if !self.contains_terminal(a) || !self.contains_terminal(b) {
            return;
        }

        let mut rep_a = None;
        let mut rep_b = None;

        Self::collect_replacements(self, a, b, &mut rep_a, &mut rep_b);

        if let (Some(node_a), Some(node_b)) = (rep_a, rep_b) {
            Self::apply_replacements(self, a, b, node_b, node_a);
        }
    }

    fn collect_replacements(
        node: &Self,
        a: &str,
        b: &str,
        rep_a: &mut Option<Self>,
        rep_b: &mut Option<Self>,
    ) {
        match node {
            Self::Terminal { uuid, .. } => {
                if uuid == a {
                    *rep_a = Some(node.clone());
                } else if uuid == b {
                    *rep_b = Some(node.clone());
                }
            }
            Self::Split { first, second, .. } => {
                Self::collect_replacements(first, a, b, rep_a, rep_b);
                Self::collect_replacements(second, a, b, rep_a, rep_b);
            }
        }
    }

    fn apply_replacements(node: &mut Self, a: &str, b: &str, val_a: Self, val_b: Self) {
        match node {
            Self::Terminal { uuid, .. } => {
                if uuid == a {
                    *node = val_a;
                } else if uuid == b {
                    *node = val_b;
                }
            }
            Self::Split { first, second, .. } => {
                Self::apply_replacements(first, a, b, val_a.clone(), val_b.clone());
                Self::apply_replacements(second, a, b, val_a, val_b);
            }
        }
    }

    pub fn set_terminal_cwd(&mut self, target_uuid: &str, cwd: Option<String>) -> bool {
        match self {
            Self::Terminal { uuid, cwd: terminal_cwd, .. } => {
                if uuid == target_uuid {
                    *terminal_cwd = cwd;
                    true
                } else {
                    false
                }
            }
            Self::Split { first, second, .. } => {
                first.set_terminal_cwd(target_uuid, cwd.clone())
                    || second.set_terminal_cwd(target_uuid, cwd)
            }
        }
    }
}

/// State of a single terminal session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionState {
    pub uuid: String,
    pub name: String,
    pub layout: LayoutNode,
    #[serde(default)]
    pub terminal_recovery: BTreeMap<String, PaneRecovery>,
    #[serde(default)]
    pub active_terminal_uuid: Option<String>,
    #[serde(default)]
    pub input_sync: bool,
}

impl SessionState {
    #[must_use]
    pub fn new(name: String) -> Self {
        Self::new_with_initial_cwd(name, None)
    }

    #[must_use]
    pub fn new_with_initial_cwd(name: String, initial_cwd: Option<String>) -> Self {
        let layout = LayoutNode::Terminal {
            uuid: uuid::Uuid::new_v4().to_string(),
            profile: None,
            cwd: initial_cwd,
            custom_title: None,
        };
        let mut terminal_recovery = BTreeMap::new();
        if let Some(terminal_uuid) = layout.terminal_uuids().into_iter().next() {
            terminal_recovery.insert(terminal_uuid, PaneRecovery::empty_shell());
        }
        let active_terminal_uuid = layout.terminal_uuids().into_iter().next();
        Self {
            uuid: uuid::Uuid::new_v4().to_string(),
            name,
            layout,
            terminal_recovery,
            active_terminal_uuid,
            input_sync: false,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn default_for_test() -> Self {
        let mut terminal_recovery = BTreeMap::new();
        terminal_recovery.insert("test-terminal-uuid".to_string(), PaneRecovery::empty_shell());
        Self {
            uuid: "test-session-uuid".to_string(),
            name: "Session 1".to_string(),
            layout: LayoutNode::new_terminal_with_uuid("test-terminal-uuid"),
            terminal_recovery,
            active_terminal_uuid: Some("test-terminal-uuid".to_string()),
            input_sync: false,
        }
    }
}

impl SessionState {
    pub fn set_recovery(&mut self, terminal_uuid: &str, recovery: PaneRecovery) {
        self.terminal_recovery.insert(terminal_uuid.to_string(), recovery);
    }

    #[must_use]
    pub fn recovery_for(&self, terminal_uuid: &str) -> Option<&PaneRecovery> {
        self.terminal_recovery.get(terminal_uuid)
    }

    pub fn prune_recovery(&mut self) {
        let valid_uuids = self.layout.terminal_uuids();
        self.terminal_recovery.retain(|terminal_uuid, _| valid_uuids.contains(terminal_uuid));
    }

    pub fn normalize_active_terminal(&mut self) {
        if self
            .active_terminal_uuid
            .as_deref()
            .is_some_and(|terminal_uuid| self.layout.contains_terminal(terminal_uuid))
        {
            return;
        }
        self.active_terminal_uuid = self.layout.terminal_uuids().into_iter().next();
    }
}

/// Persistent state of the entire application window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowState {
    pub sessions: Vec<SessionState>,
    pub active_session_index: usize,
    pub width: i32,
    pub height: i32,
    pub is_maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            sessions: vec![SessionState::new("Session 1".into())],
            active_session_index: 0,
            width: 900,
            height: 600,
            is_maximized: false,
        }
    }
}

impl WindowState {
    #[cfg(test)]
    #[must_use]
    pub fn default_for_test() -> Self {
        Self {
            sessions: vec![SessionState::default_for_test()],
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
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn term(uuid: &str) -> LayoutNode {
        LayoutNode::Terminal { uuid: uuid.into(), profile: None, cwd: None, custom_title: None }
    }

    fn term_full(uuid: &str, cwd: &str, title: &str) -> LayoutNode {
        LayoutNode::Terminal {
            uuid: uuid.into(),
            profile: None,
            cwd: Some(cwd.into()),
            custom_title: Some(title.into()),
        }
    }

    fn hsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
        LayoutNode::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn split_ratio(
        orientation: SplitOrientation,
        ratio: f64,
        first: LayoutNode,
        second: LayoutNode,
    ) -> LayoutNode {
        LayoutNode::Split { orientation, ratio, first: Box::new(first), second: Box::new(second) }
    }

    #[test]
    fn new_terminal_has_valid_uuid() {
        let node = LayoutNode::new_terminal();
        if let LayoutNode::Terminal { uuid, .. } = node {
            assert!(!uuid.is_empty());
        } else {
            panic!("new_terminal must return a Terminal variant");
        }
    }

    #[test]
    fn split_creates_two_children() {
        let node = term("t1");
        let split = node.split(SplitOrientation::Horizontal);
        assert_eq!(split.terminal_count(), 2);
    }

    #[test]
    fn split_preserves_original_terminal() {
        let node = term("t1");
        let split = node.split(SplitOrientation::Horizontal);
        let uuids = split.terminal_uuids();
        assert!(uuids.contains(&"t1".to_string()));
    }

    #[test]
    fn two_new_terminals_have_different_uuids() {
        let t1 = LayoutNode::new_terminal();
        let t2 = LayoutNode::new_terminal();
        if let (LayoutNode::Terminal { uuid: u1, .. }, LayoutNode::Terminal { uuid: u2, .. }) =
            (t1, t2)
        {
            assert_ne!(u1, u2);
        }
    }

    #[test]
    fn terminal_uuids_are_unique() {
        let root = term("t1");
        let split1 = root.split(SplitOrientation::Horizontal);
        let split2 = split1.split(SplitOrientation::Vertical);
        let uuids = split2.terminal_uuids();
        let mut unique = uuids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(uuids.len(), unique.len());
    }

    #[test]
    fn nested_splits_count_correctly() {
        let root = hsplit(term("t1"), hsplit(term("t2"), term("t3")));
        assert_eq!(root.terminal_count(), 3);
    }

    #[test]
    fn remove_root_terminal_returns_none() {
        let root = term("t1");
        assert!(root.remove_terminal("t1").is_none());
    }

    #[test]
    fn remove_first_child_returns_second() {
        let root = hsplit(term("t1"), term("t2"));
        let result = root.remove_terminal("t1").unwrap();
        assert_eq!(result, term("t2"));
    }

    #[test]
    fn remove_second_child_returns_first() {
        let root = hsplit(term("t1"), term("t2"));
        let result = root.remove_terminal("t2").unwrap();
        assert_eq!(result, term("t1"));
    }

    #[test]
    fn remove_from_nested_preserves_structure() {
        let root = hsplit(term("t1"), hsplit(term("t2"), term("t3")));
        let result = root.remove_terminal("t2").unwrap();
        assert_eq!(result, hsplit(term("t1"), term("t3")));
    }

    #[test]
    fn remove_nonexistent_returns_unchanged() {
        let root = hsplit(term("t1"), term("t2"));
        let result = root.remove_terminal("t3").unwrap();
        assert_eq!(result, root);
    }

    #[test]
    fn split_specific_terminal_in_tree() {
        let root = hsplit(term("t1"), term("t2"));
        let result = root.split_terminal("t2", SplitOrientation::Vertical).unwrap();
        assert_eq!(result.terminal_count(), 3);
        let uuids = result.terminal_uuids();
        assert!(uuids.contains(&"t1".to_string()));
        assert!(uuids.contains(&"t2".to_string()));
    }

    #[test]
    fn split_nonexistent_terminal_returns_none() {
        let root = hsplit(term("t1"), term("t2"));
        assert!(root.split_terminal("t3", SplitOrientation::Vertical).is_none());
    }

    #[test]
    fn split_deeply_nested_terminal() {
        let root = hsplit(term("t1"), hsplit(term("t2"), term("t3")));
        let result = root.split_terminal("t3", SplitOrientation::Vertical).unwrap();
        assert_eq!(result.terminal_count(), 4);
        assert!(result.contains_terminal("t3"));
    }

    #[test]
    fn split_terminal_with_new_uuid_reports_created_terminal() {
        let root = term("t1");
        let (new_tree, new_uuid) =
            root.split_terminal_with_new_uuid("t1", SplitOrientation::Horizontal).unwrap();
        assert_eq!(new_tree.terminal_count(), 2);
        assert!(new_tree.contains_terminal(&new_uuid));
        assert_ne!(new_uuid, "t1");
    }

    #[test]
    fn contains_terminal_works() {
        let root = hsplit(term("t1"), term("t2"));
        assert!(root.contains_terminal("t1"));
        assert!(root.contains_terminal("t2"));
        assert!(!root.contains_terminal("t3"));
    }

    #[rstest]
    #[case(term("t1"), 1)]
    #[case(hsplit(term("t1"), term("t2")), 2)]
    #[case(hsplit(term("t1"), hsplit(term("t2"), term("t3"))), 3)]
    #[case(hsplit(hsplit(term("t1"), term("t2")), hsplit(term("t3"), term("t4"))), 3)]
    #[case(hsplit(term("t1"), hsplit(term("t2"), hsplit(term("t3"), term("t4")))), 4)]
    fn depth_matches_expected(#[case] node: LayoutNode, #[case] expected: usize) {
        fn get_depth(n: &LayoutNode) -> usize {
            match n {
                LayoutNode::Terminal { .. } => 1,
                LayoutNode::Split { first, second, .. } => {
                    1 + get_depth(first).max(get_depth(second))
                }
            }
        }
        assert_eq!(get_depth(&node), expected);
    }

    #[test]
    fn complex_layout_roundtrip() {
        let layout = hsplit(
            term("t1"),
            LayoutNode::Split {
                orientation: SplitOrientation::Vertical,
                ratio: 0.3,
                first: Box::new(term("t2")),
                second: Box::new(term("t3")),
            },
        );
        let json = serde_json::to_string(&layout).unwrap();
        let restored: LayoutNode = serde_json::from_str(&json).unwrap();
        assert_eq!(layout, restored);
    }

    #[test]
    fn session_state_roundtrip() {
        let mut terminal_recovery = BTreeMap::new();
        terminal_recovery.insert(
            "t2".into(),
            PaneRecovery {
                source: PaneSource::Bookmark { name: "Prod".into() },
                target: None,
                startup: vec![StartupStep::SendText {
                    text: "ssh prod && tmux attach -t web".into(),
                    execute: true,
                }],
            },
        );
        let session = SessionState {
            uuid: "s1".into(),
            name: "Work".into(),
            layout: hsplit(term("t1"), term("t2")),
            terminal_recovery,
            active_terminal_uuid: Some("t2".into()),
            input_sync: true,
        };
        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }

    #[test]
    fn window_state_roundtrip() {
        let state = WindowState {
            sessions: vec![SessionState::new("S1".into())],
            active_session_index: 0,
            width: 800,
            height: 600,
            is_maximized: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let restored: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn default_window_state_is_valid() {
        let state = WindowState::default();
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session_index, 0);
        assert!(!state.sessions[0].uuid.is_empty());
    }

    #[test]
    fn swap_terminals_exchanges_positions() {
        let mut layout = hsplit(term("t1"), term("t2"));
        layout.swap_terminals("t1", "t2");
        assert_eq!(layout.terminal_uuids(), vec!["t2", "t1"]);
    }

    #[test]
    fn swap_terminals_preserves_count() {
        let mut layout = hsplit(term("t1"), hsplit(term("t2"), term("t3")));
        let count = layout.terminal_count();
        layout.swap_terminals("t1", "t3");
        assert_eq!(layout.terminal_count(), count);
    }

    #[test]
    fn swap_nonexistent_terminal_is_noop() {
        let mut layout = hsplit(term("t1"), term("t2"));
        let before = layout.clone();
        layout.swap_terminals("t1", "t3");
        assert_eq!(layout, before);
    }

    #[test]
    fn swap_same_terminal_is_noop() {
        let mut layout = hsplit(term("t1"), term("t2"));
        let before = layout.clone();
        layout.swap_terminals("t1", "t1");
        assert_eq!(layout, before);
    }

    #[test]
    fn swap_preserves_full_terminal_data() {
        let mut layout =
            hsplit(term_full("t1", "/home/alice", "editor"), term_full("t2", "/tmp", "build"));
        layout.swap_terminals("t1", "t2");

        let uuids = layout.terminal_uuids();
        assert_eq!(uuids, vec!["t2", "t1"]);

        if let LayoutNode::Split { first, second, .. } = &layout {
            if let LayoutNode::Terminal { uuid, cwd, custom_title, .. } = first.as_ref() {
                assert_eq!(uuid, "t2");
                assert_eq!(cwd.as_deref(), Some("/tmp"));
                assert_eq!(custom_title.as_deref(), Some("build"));
            } else {
                panic!("Expected Terminal");
            }
            if let LayoutNode::Terminal { uuid, cwd, custom_title, .. } = second.as_ref() {
                assert_eq!(uuid, "t1");
                assert_eq!(cwd.as_deref(), Some("/home/alice"));
                assert_eq!(custom_title.as_deref(), Some("editor"));
            } else {
                panic!("Expected Terminal");
            }
        } else {
            panic!("Expected Split");
        }
    }

    #[test]
    fn split_preserves_parent_ratio() {
        let layout = split_ratio(SplitOrientation::Horizontal, 0.7, term("t1"), term("t2"));
        let result = layout.split_terminal("t1", SplitOrientation::Vertical).unwrap();
        if let LayoutNode::Split { ratio, .. } = &result {
            assert!(
                (*ratio - 0.7).abs() < f64::EPSILON,
                "Parent ratio changed from 0.7 to {ratio}"
            );
        } else {
            panic!("Expected Split");
        }
    }

    #[test]
    fn remove_preserves_sibling_ratio() {
        let inner = split_ratio(SplitOrientation::Horizontal, 0.3, term("t1"), term("t2"));
        let layout = split_ratio(SplitOrientation::Vertical, 0.7, inner, term("t3"));
        let result = layout.remove_terminal("t1").unwrap();
        if let LayoutNode::Split { ratio, .. } = &result {
            assert!((*ratio - 0.7).abs() < f64::EPSILON, "Outer ratio changed from 0.7 to {ratio}");
        } else {
            panic!("Expected Split");
        }
    }

    #[test]
    fn remove_last_terminal_returns_none_not_empty_tree() {
        let layout = term("only");
        let result = layout.remove_terminal("only");
        assert!(result.is_none(), "Removing the only terminal must return None");
    }

    #[test]
    fn new_terminal_uuid_is_valid_v4() {
        let node = LayoutNode::new_terminal();
        if let LayoutNode::Terminal { uuid, .. } = &node {
            assert!(uuid::Uuid::parse_str(uuid).is_ok(), "UUID '{uuid}' is not valid UUID format");
            assert_eq!(
                uuid::Uuid::parse_str(uuid).unwrap().get_version(),
                Some(uuid::Version::Random),
                "UUID must be v4 (random)"
            );
        }
    }

    #[test]
    fn split_ratio_is_valid() {
        let node = term("t1");
        let split = node.split(SplitOrientation::Horizontal);
        if let LayoutNode::Split { ratio, .. } = &split {
            assert!(*ratio > 0.0 && *ratio < 1.0, "Ratio {ratio} out of (0,1)");
        }
    }

    #[test]
    fn set_terminal_cwd_updates_nested_target_terminal_only() {
        let mut layout = hsplit(
            term_full("t1", "/old/one", "one"),
            split_ratio(
                SplitOrientation::Vertical,
                0.5,
                term_full("t2", "/old/two", "two"),
                term_full("t3", "/old/three", "three"),
            ),
        );

        assert!(layout.set_terminal_cwd("t2", Some("/new/two".into())));

        let LayoutNode::Split { first, second, .. } = &layout else {
            panic!("expected split");
        };
        let LayoutNode::Terminal { cwd: first_cwd, .. } = first.as_ref() else {
            panic!("expected terminal");
        };
        assert_eq!(first_cwd.as_deref(), Some("/old/one"));

        let LayoutNode::Split { first: inner_first, second: inner_second, .. } = second.as_ref()
        else {
            panic!("expected nested split");
        };
        let LayoutNode::Terminal { cwd: second_cwd, .. } = inner_first.as_ref() else {
            panic!("expected target terminal");
        };
        let LayoutNode::Terminal { cwd: third_cwd, .. } = inner_second.as_ref() else {
            panic!("expected sibling terminal");
        };
        assert_eq!(second_cwd.as_deref(), Some("/new/two"));
        assert_eq!(third_cwd.as_deref(), Some("/old/three"));
    }

    #[test]
    fn set_terminal_cwd_returns_false_for_unknown_terminal() {
        let mut layout = hsplit(term("t1"), term("t2"));
        assert!(!layout.set_terminal_cwd("missing", Some("/tmp".into())));
    }

    #[test]
    fn startup_step_terminal_input_respects_execute_flag() {
        assert_eq!(
            StartupStep::SendText { text: "echo hi".into(), execute: true }.terminal_input(),
            "echo hi\n"
        );
        assert_eq!(
            StartupStep::SendText { text: "echo hi".into(), execute: false }.terminal_input(),
            "echo hi"
        );
    }

    #[test]
    fn pane_target_managed_startup_input_uses_attach_only_tmux() {
        assert_eq!(
            PaneTarget::LocalTmux { session: "dev".into() }.managed_startup_input().as_deref(),
            Some("exec tmux attach-session -t 'dev'\n")
        );
        assert_eq!(
            PaneTarget::RemoteTmux {
                ssh_target: "deploy@example.com".into(),
                tmux_session: "web".into(),
            }
            .managed_startup_input()
            .as_deref(),
            Some("exec ssh -t deploy@example.com 'tmux attach-session -t '\"'\"'web'\"'\"''\n")
        );
    }

    #[test]
    fn pane_target_remote_shell_with_folder_builds_replayable_command() {
        assert_eq!(
            PaneTarget::RemoteShell {
                ssh_target: "deploy@example.com".into(),
                remote_folder: Some("/srv/app".into()),
            }
            .managed_startup_input()
            .as_deref(),
            Some(
                "exec ssh -t deploy@example.com 'cd '\"'\"'/srv/app'\"'\"' && exec ${SHELL:-/bin/bash} -l'\n"
            )
        );
    }

    #[test]
    fn pane_recovery_roundtrips_structured_target() {
        let mut terminal_recovery = BTreeMap::new();
        terminal_recovery.insert(
            "t1".into(),
            PaneRecovery {
                source: PaneSource::Bookmark { name: "Prod".into() },
                target: Some(PaneTarget::RemoteTmux {
                    ssh_target: "deploy@example.com".into(),
                    tmux_session: "web".into(),
                }),
                startup: Vec::new(),
            },
        );
        let session = SessionState {
            uuid: "session-1".into(),
            name: "Prod".into(),
            layout: term("t1"),
            terminal_recovery,
            active_terminal_uuid: Some("t1".into()),
            input_sync: false,
        };

        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }

    #[test]
    fn new_session_starts_with_empty_shell_recovery_for_initial_terminal() {
        let session = SessionState::new("Work".into());
        let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();

        assert_eq!(session.recovery_for(&terminal_uuid), Some(&PaneRecovery::empty_shell()));
    }

    #[test]
    fn prune_recovery_removes_closed_terminal_entries() {
        let mut session = SessionState {
            uuid: "s1".into(),
            name: "Work".into(),
            layout: hsplit(term("t1"), term("t2")),
            terminal_recovery: BTreeMap::from([
                (
                    "t1".into(),
                    PaneRecovery {
                        source: PaneSource::Manual,
                        target: None,
                        startup: vec![StartupStep::SendText {
                            text: "echo one".into(),
                            execute: true,
                        }],
                    },
                ),
                (
                    "t2".into(),
                    PaneRecovery {
                        source: PaneSource::Bookmark { name: "Prod".into() },
                        target: None,
                        startup: vec![StartupStep::SendText {
                            text: "ssh prod".into(),
                            execute: true,
                        }],
                    },
                ),
                (
                    "ghost".into(),
                    PaneRecovery {
                        source: PaneSource::Command { title: "Detached".into() },
                        target: None,
                        startup: vec![StartupStep::SendText {
                            text: "echo stale".into(),
                            execute: false,
                        }],
                    },
                ),
            ]),
            active_terminal_uuid: Some("ghost".into()),
            input_sync: false,
        };

        session.layout = session.layout.remove_terminal("t2").unwrap();
        session.prune_recovery();

        assert!(session.recovery_for("t1").is_some());
        assert!(session.recovery_for("t2").is_none());
        assert!(session.recovery_for("ghost").is_none());
    }

    #[test]
    fn normalize_active_terminal_falls_back_to_first_live_terminal() {
        let mut session = SessionState {
            uuid: "s1".into(),
            name: "Work".into(),
            layout: hsplit(term("t1"), term("t2")),
            terminal_recovery: Default::default(),
            active_terminal_uuid: Some("ghost".into()),
            input_sync: false,
        };

        session.normalize_active_terminal();
        assert_eq!(session.active_terminal_uuid.as_deref(), Some("t1"));

        session.layout = session.layout.remove_terminal("t1").unwrap();
        session.normalize_active_terminal();
        assert_eq!(session.active_terminal_uuid.as_deref(), Some("t2"));
    }
}

#[cfg(test)]
pub mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn arb_orientation() -> impl Strategy<Value = SplitOrientation> {
        prop_oneof![Just(SplitOrientation::Horizontal), Just(SplitOrientation::Vertical),]
    }

    fn arb_layout() -> impl Strategy<Value = LayoutNode> {
        let leaf = any::<u32>().prop_map(|_| LayoutNode::new_terminal());
        leaf.prop_recursive(4, 16, 2, |inner| {
            (arb_orientation(), (0.05..0.95f64), inner.clone(), inner).prop_map(
                |(orientation, ratio, first, second)| LayoutNode::Split {
                    orientation,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                },
            )
        })
    }

    proptest! {
        #[test]
        fn split_increases_count_by_one(layout in arb_layout()) {
            let uuids = layout.terminal_uuids();
            let target = &uuids[0];
            let new_layout = layout.split_terminal(target, SplitOrientation::Horizontal).unwrap();
            prop_assert_eq!(new_layout.terminal_count(), layout.terminal_count() + 1);
        }

        #[test]
        fn remove_decreases_count_by_one(layout in arb_layout()) {
            let uuids = layout.terminal_uuids();
            if uuids.len() > 1 {
                let target = &uuids[0];
                let new_layout = layout.remove_terminal(target).unwrap();
                prop_assert_eq!(new_layout.terminal_count(), layout.terminal_count() - 1);
            }
        }

        #[test]
        fn swap_preserves_all_uuids(layout in arb_layout()) {
            let uuids = layout.terminal_uuids();
            if uuids.len() >= 2 {
                let mut new_layout = layout.clone();
                new_layout.swap_terminals(&uuids[0], &uuids[1]);
                let mut new_uuids = new_layout.terminal_uuids();
                let mut old_uuids = uuids;
                new_uuids.sort();
                old_uuids.sort();
                prop_assert_eq!(new_uuids, old_uuids);
            }
        }

        #[test]
        fn all_uuids_unique(layout in arb_layout()) {
            let uuids = layout.terminal_uuids();
            let count = uuids.len();
            let mut unique = uuids;
            unique.sort();
            unique.dedup();
            prop_assert_eq!(count, unique.len());
        }

        #[test]
        fn ratio_preserved(layout in arb_layout()) {
            if let LayoutNode::Split { ratio, .. } = &layout {
                let uuids = layout.terminal_uuids();
                let target = &uuids[0];
                let new_layout = layout.split_terminal(target, SplitOrientation::Vertical).unwrap();
                if let LayoutNode::Split { ratio: r2, .. } = new_layout {
                    prop_assert_eq!(*ratio, r2);
                }
            }
        }

        #[test]
        fn serde_roundtrip(layout in arb_layout()) {
            let json = serde_json::to_string(&layout).unwrap();
            let restored: LayoutNode = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(layout, restored);
        }

        #[test]
        fn depth_at_least_one(layout in arb_layout()) {
            fn get_depth(n: &LayoutNode) -> usize {
                match n {
                    LayoutNode::Terminal { .. } => 1,
                    LayoutNode::Split { first, second, .. } => 1 + get_depth(first).max(get_depth(second))
                }
            }
            prop_assert!(get_depth(&layout) >= 1);
        }

        #[test]
        fn count_equals_uuid_count(layout in arb_layout()) {
            prop_assert_eq!(layout.terminal_count(), layout.terminal_uuids().len());
        }

        #[test]
        fn split_remove_restores_uuids(layout in arb_layout()) {
            let original_uuids = layout.terminal_uuids();
            let target = &original_uuids[0];
            if let Some((new_layout, new_uuid)) = layout.split_terminal_with_new_uuid(target, SplitOrientation::Horizontal) {
                if let Some(restored_layout) = new_layout.remove_terminal(&new_uuid) {
                    let restored = restored_layout.terminal_uuids();
                    prop_assert_eq!(original_uuids, restored, "Split+remove must restore original UUID set");
                }
            }
        }
    }
}

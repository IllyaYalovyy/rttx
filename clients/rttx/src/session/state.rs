//! Persisted session and window state.
//!
//! `SessionState` and `WindowState` are serialized to `sessions.json`.
//! Changes here must preserve backward compatibility via `#[serde(default)]`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::layout::LayoutNode;
use super::recovery::PaneRecovery;
use crate::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};

/// How a session's terminals are backed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    #[default]
    Direct,
    Persistent {
        daemon_session_id: String,
    },
    RemotePersistent {
        host: String,
        daemon_session_id: String,
    },
}

impl SessionMode {
    #[must_use]
    pub const fn is_persistent(&self) -> bool {
        !matches!(self, Self::Direct)
    }

    #[must_use]
    pub fn daemon_session_id(&self) -> Option<&str> {
        match self {
            Self::Direct => None,
            Self::Persistent { daemon_session_id }
            | Self::RemotePersistent { daemon_session_id, .. } => Some(daemon_session_id),
        }
    }

    #[must_use]
    pub fn host(&self) -> Option<&str> {
        match self {
            Self::RemotePersistent { host, .. } => Some(host),
            _ => None,
        }
    }
}

/// Accent color for a session's sidebar indicator dot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionColor {
    #[default]
    Blue,
    Green,
    Yellow,
    Red,
    Purple,
    Pink,
    Teal,
    Orange,
}

impl SessionColor {
    /// All available colors in assignment order.
    pub const ALL: [Self; 8] = [
        Self::Blue,
        Self::Green,
        Self::Yellow,
        Self::Red,
        Self::Purple,
        Self::Pink,
        Self::Teal,
        Self::Orange,
    ];

    /// CSS class name for the color dot.
    #[must_use]
    pub const fn css_class(self) -> &'static str {
        match self {
            Self::Blue => "accent-blue",
            Self::Green => "accent-green",
            Self::Yellow => "accent-yellow",
            Self::Red => "accent-red",
            Self::Purple => "accent-purple",
            Self::Pink => "accent-pink",
            Self::Teal => "accent-teal",
            Self::Orange => "accent-orange",
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
    #[serde(default)]
    pub mode: SessionMode,
    #[serde(default)]
    pub runtime: WorkspaceRuntime,
    #[serde(default)]
    pub color: SessionColor,
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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
        }
    }

    #[must_use]
    pub fn new_managed_local(
        name: String,
        policy: WorkspacePolicy,
        initial_cwd: Option<String>,
    ) -> Self {
        let mut session = Self::new_with_initial_cwd(name, initial_cwd);
        let layout_terminal_uuids = session.layout.terminal_uuids();
        session.runtime = WorkspaceRuntime::managed_local(policy, &layout_terminal_uuids);
        session.sync_legacy_mode_from_runtime();
        session
    }

    #[must_use]
    pub fn new_managed_remote(
        name: String,
        host: &str,
        policy: WorkspacePolicy,
        initial_cwd: Option<String>,
    ) -> Self {
        let mut session = Self::new_with_initial_cwd(name, initial_cwd);
        let layout_terminal_uuids = session.layout.terminal_uuids();
        session.runtime = WorkspaceRuntime::managed_remote(host, policy, &layout_terminal_uuids);
        session.sync_legacy_mode_from_runtime();
        session
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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
        }
    }
}

impl SessionState {
    #[must_use]
    pub const fn uses_managed_runtime(&self) -> bool {
        self.runtime.is_managed() || self.mode.is_persistent()
    }

    pub fn normalize_runtime_metadata(&mut self) {
        if !self.runtime.is_managed() {
            match &self.mode {
                SessionMode::Direct => {}
                SessionMode::Persistent { daemon_session_id } => {
                    self.runtime.managed = true;
                    self.runtime.endpoint = RuntimeEndpoint::Local;
                    self.runtime.policy = WorkspacePolicy::Persistent;
                    if !daemon_session_id.is_empty() {
                        self.runtime.runtime_id = Some(daemon_session_id.clone());
                    }
                }
                SessionMode::RemotePersistent { host, daemon_session_id } => {
                    self.runtime.managed = true;
                    self.runtime.endpoint = RuntimeEndpoint::Remote { host: host.clone() };
                    self.runtime.policy = WorkspacePolicy::Persistent;
                    if !daemon_session_id.is_empty() {
                        self.runtime.runtime_id = Some(daemon_session_id.clone());
                    }
                }
            }
        }

        self.runtime.ensure_placeholder_bindings(&self.layout.terminal_uuids());
        self.sync_legacy_mode_from_runtime();
    }

    pub fn sync_legacy_mode_from_runtime(&mut self) {
        self.mode = if self.runtime.is_managed() {
            let daemon_session_id = self.runtime.runtime_id.clone().unwrap_or_default();
            match &self.runtime.endpoint {
                RuntimeEndpoint::Local => SessionMode::Persistent { daemon_session_id },
                RuntimeEndpoint::Remote { host } => {
                    SessionMode::RemotePersistent { host: host.clone(), daemon_session_id }
                }
            }
        } else {
            SessionMode::Direct
        };
    }

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

    pub fn replace_terminal_uuid(&mut self, old_uuid: &str, new_uuid: &str) -> bool {
        if old_uuid == new_uuid || !self.layout.replace_terminal_uuid(old_uuid, new_uuid) {
            return false;
        }

        if let Some(recovery) = self.terminal_recovery.remove(old_uuid) {
            self.terminal_recovery.insert(new_uuid.to_string(), recovery);
        }

        self.runtime.replace_layout_terminal_uuid(old_uuid, new_uuid);

        if self.active_terminal_uuid.as_deref() == Some(old_uuid) {
            self.active_terminal_uuid = Some(new_uuid.to_string());
        }

        true
    }
}

const fn default_left_sidebar_width() -> i32 {
    220
}

const fn default_right_sidebar_width() -> i32 {
    320
}

/// Persistent state of the entire application window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowState {
    pub sessions: Vec<SessionState>,
    pub active_session_index: usize,
    pub width: i32,
    pub height: i32,
    pub is_maximized: bool,
    #[serde(default = "default_left_sidebar_width")]
    pub left_sidebar_width: i32,
    #[serde(default = "default_right_sidebar_width")]
    pub right_sidebar_width: i32,
    /// Runtime IDs that the user explicitly closed. Prevents inventory
    /// resurrection until the daemon actually removes the runtime.
    #[serde(default)]
    pub dismissed_runtime_ids: std::collections::BTreeSet<String>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            sessions: vec![SessionState::new("Session 1".into())],
            active_session_index: 0,
            width: 900,
            height: 600,
            is_maximized: false,
            left_sidebar_width: default_left_sidebar_width(),
            right_sidebar_width: default_right_sidebar_width(),
            dismissed_runtime_ids: std::collections::BTreeSet::new(),
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
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};
    use crate::session::recovery::{PaneSource, PaneTarget, StartupStep};
    use crate::test_helpers::{hsplit, term};
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }

    #[test]
    fn session_replace_terminal_uuid_updates_recovery_and_focus() {
        let mut session = SessionState::default_for_test();
        session.set_recovery(
            "other-terminal",
            PaneRecovery { source: PaneSource::Manual, target: None, startup: vec![] },
        );

        assert!(session.replace_terminal_uuid("test-terminal-uuid", "daemon-pane"));
        assert_eq!(session.layout.terminal_uuids(), vec!["daemon-pane"]);
        assert_eq!(session.active_terminal_uuid.as_deref(), Some("daemon-pane"));
        assert!(session.recovery_for("test-terminal-uuid").is_none());
        assert_eq!(session.recovery_for("daemon-pane"), Some(&PaneRecovery::empty_shell()));
        assert!(session.recovery_for("other-terminal").is_some());
    }

    #[test]
    fn session_replace_terminal_uuid_is_noop_for_missing_terminal() {
        let mut session = SessionState::default_for_test();
        let original = session.clone();
        assert!(!session.replace_terminal_uuid("missing", "daemon-pane"));
        assert_eq!(session, original);
    }

    #[test]
    fn window_state_roundtrip() {
        let state = WindowState {
            sessions: vec![SessionState::new("S1".into())],
            active_session_index: 0,
            width: 800,
            height: 600,
            is_maximized: true,
            ..WindowState::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let restored: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);
    }

    #[test]
    fn session_mode_default_is_direct() {
        let session = SessionState::new("Test".into());
        assert_eq!(session.mode, SessionMode::Direct);
        assert!(!session.mode.is_persistent());
        assert!(session.mode.daemon_session_id().is_none());
        assert!(session.mode.host().is_none());
        assert!(!session.uses_managed_runtime());
    }

    #[test]
    fn session_mode_persistent_accessors() {
        let mode = SessionMode::Persistent { daemon_session_id: "ds1".into() };
        assert!(mode.is_persistent());
        assert_eq!(mode.daemon_session_id(), Some("ds1"));
        assert!(mode.host().is_none());
    }

    #[test]
    fn session_mode_remote_persistent_accessors() {
        let mode = SessionMode::RemotePersistent {
            host: "user@devbox".into(),
            daemon_session_id: "ds2".into(),
        };
        assert!(mode.is_persistent());
        assert_eq!(mode.daemon_session_id(), Some("ds2"));
        assert_eq!(mode.host(), Some("user@devbox"));
    }

    #[test]
    fn session_mode_roundtrips_through_json() {
        for mode in [
            SessionMode::Direct,
            SessionMode::Persistent { daemon_session_id: "abc-123".into() },
            SessionMode::RemotePersistent {
                host: "admin@prod".into(),
                daemon_session_id: "def-456".into(),
            },
        ] {
            let mut session = SessionState::new("Test".into());
            session.mode = mode.clone();
            let json = serde_json::to_string(&session).unwrap();
            let restored: SessionState = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.mode, mode);
        }
    }

    #[test]
    fn backward_compat_session_without_mode_field() {
        let json = r#"{
            "uuid": "s1",
            "name": "Old",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}},
            "terminal_recovery": {},
            "active_terminal_uuid": "t1",
            "input_sync": false
        }"#;
        let session: SessionState = serde_json::from_str(json).unwrap();
        assert_eq!(session.mode, SessionMode::Direct);
    }

    #[test]
    fn persistent_session_in_window_state_roundtrips() {
        let mut session = SessionState::new("Persistent".into());
        session.mode = SessionMode::Persistent { daemon_session_id: "ds-1".into() };
        session.normalize_runtime_metadata();

        let state = WindowState {
            sessions: vec![SessionState::new("Direct".into()), session],
            active_session_index: 1,
            width: 1920,
            height: 1080,
            is_maximized: false,
            ..WindowState::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let restored: WindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.sessions[0].mode, SessionMode::Direct);
        assert_eq!(
            restored.sessions[1].mode,
            SessionMode::Persistent { daemon_session_id: "ds-1".into() }
        );
        assert!(restored.sessions[1].runtime.is_managed());
        assert_eq!(restored.sessions[1].runtime.runtime_id.as_deref(), Some("ds-1"));
    }

    #[test]
    fn new_managed_local_session_sets_runtime_metadata() {
        let session =
            SessionState::new_managed_local("Workspace".into(), WorkspacePolicy::Ephemeral, None);
        assert!(session.uses_managed_runtime());
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
        assert_eq!(session.runtime.policy, WorkspacePolicy::Ephemeral);
        assert_eq!(session.runtime.pane_bindings.len(), 1);
        let only_binding = session.runtime.pane_bindings.iter().next().unwrap();
        assert_eq!(only_binding.0, only_binding.1);
    }

    #[test]
    fn normalize_runtime_metadata_migrates_remote_legacy_mode() {
        let mut session = SessionState::new("Remote".into());
        session.mode = SessionMode::RemotePersistent {
            host: "deploy@example.com".into(),
            daemon_session_id: "runtime-1".into(),
        };
        session.normalize_runtime_metadata();
        assert!(session.runtime.is_managed());
        assert_eq!(
            session.runtime.endpoint,
            RuntimeEndpoint::Remote { host: "deploy@example.com".into() }
        );
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(session.runtime.runtime_id.as_deref(), Some("runtime-1"));
        assert_eq!(session.runtime.pane_bindings.len(), 1);
    }

    #[test]
    fn normalize_runtime_metadata_preserves_detached_remote_workspace_without_runtime_id() {
        let json = r#"{
            "uuid": "workspace-1",
            "name": "Detached Remote",
            "layout": {"Terminal": {"uuid": "pane-1", "profile": null, "cwd": null, "custom_title": null}},
            "terminal_recovery": {},
            "active_terminal_uuid": "pane-1",
            "input_sync": false,
            "mode": {
                "remote-persistent": {
                    "host": "deploy@example.com",
                    "daemon_session_id": ""
                }
            }
        }"#;
        let mut session: SessionState = serde_json::from_str(json).unwrap();
        session.normalize_runtime_metadata();
        assert!(session.runtime.is_managed());
        assert_eq!(
            session.runtime.endpoint,
            RuntimeEndpoint::Remote { host: "deploy@example.com".into() }
        );
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(session.runtime.runtime_id, None);
        assert_eq!(
            session.mode,
            SessionMode::RemotePersistent {
                host: "deploy@example.com".into(),
                daemon_session_id: String::new(),
            }
        );
        assert_eq!(session.runtime.pane_bindings.get("pane-1").map(String::as_str), Some("pane-1"));
        assert!(session.runtime.pending_layout_panes.contains("pane-1"));
    }

    #[test]
    fn default_window_state_is_valid() {
        let state = WindowState::default();
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.active_session_index, 0);
        assert!(!state.sessions[0].uuid.is_empty());
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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
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
            terminal_recovery: BTreeMap::default(),
            active_terminal_uuid: Some("ghost".into()),
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
        };
        session.normalize_active_terminal();
        assert_eq!(session.active_terminal_uuid.as_deref(), Some("t1"));
        session.layout = session.layout.remove_terminal("t1").unwrap();
        session.normalize_active_terminal();
        assert_eq!(session.active_terminal_uuid.as_deref(), Some("t2"));
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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }
}

#[cfg(test)]
mod module_boundary_tests {
    use super::*;
    use crate::session::layout::SplitOrientation;

    /// Verify that layout tree operations and state recovery stay consistent
    /// across the module boundary after the layout/state/recovery split.
    #[test]
    fn split_then_prune_recovery_respects_module_boundary() {
        let mut session = SessionState::new("Test".into());
        let uuid = session.layout.terminal_uuids()[0].clone();

        let (new_layout, new_uuid) = session
            .layout
            .split_terminal_with_new_uuid(&uuid, SplitOrientation::Horizontal)
            .unwrap();
        session.layout = new_layout;
        session.set_recovery(&new_uuid, PaneRecovery::empty_shell());

        assert_eq!(session.layout.terminal_count(), 2);
        assert!(session.recovery_for(&new_uuid).is_some());

        session.layout = session.layout.remove_terminal(&new_uuid).unwrap();
        session.prune_recovery();

        assert_eq!(session.layout.terminal_count(), 1);
        assert!(session.recovery_for(&new_uuid).is_none());
        assert!(session.recovery_for(&uuid).is_some());
    }

    #[test]
    fn new_managed_remote_sets_endpoint_and_mode() {
        let session = SessionState::new_managed_remote(
            "Work".into(),
            "server.example.com",
            WorkspacePolicy::Persistent,
            Some("/home/user".into()),
        );
        assert!(session.runtime.is_managed());
        assert_eq!(
            session.runtime.endpoint,
            RuntimeEndpoint::Remote { host: "server.example.com".into() }
        );
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(
            session.mode,
            SessionMode::RemotePersistent {
                host: "server.example.com".into(),
                daemon_session_id: String::new(),
            }
        );
        assert_eq!(
            session.layout.terminal_cwd(&session.layout.terminal_uuids()[0]).as_deref(),
            Some("/home/user")
        );
    }

    #[test]
    fn session_color_survives_serde_roundtrip() {
        let mut session = SessionState::new("Test".into());
        session.color = SessionColor::Purple;
        let json = serde_json::to_string(&session).unwrap();
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.color, SessionColor::Purple);
    }

    #[test]
    fn session_color_defaults_to_blue_for_old_state() {
        let mut session = SessionState::new("Test".into());
        session.color = SessionColor::Blue;
        let json = serde_json::to_string(&session).unwrap();
        // Remove color field to simulate old state.
        let json = json.replace(r#","color":"blue""#, "");
        let restored: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.color, SessionColor::Blue);
    }

    #[test]
    fn session_color_all_has_eight_variants() {
        assert_eq!(SessionColor::ALL.len(), 8);
    }

    #[test]
    fn session_color_css_classes_are_unique() {
        let classes: std::collections::HashSet<_> =
            SessionColor::ALL.iter().map(|c| c.css_class()).collect();
        assert_eq!(classes.len(), 8);
    }
}

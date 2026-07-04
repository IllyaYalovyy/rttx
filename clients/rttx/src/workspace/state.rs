//! Session and window state types.
//!
//! `WorkspaceState` is the in-memory representation of a single workspace.
//! `WindowState` is the in-memory aggregate used by the window; persistence
//! is handled by `ClientStore` (RFC-023).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::layout::LayoutNode;
use super::recovery::PaneRecovery;
use crate::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};

/// Accent color for a session's sidebar indicator dot.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceColor {
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

impl WorkspaceColor {
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
pub struct WorkspaceState {
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
    pub runtime: WorkspaceRuntime,
    #[serde(default)]
    pub color: WorkspaceColor,
    #[serde(default)]
    pub zoomed_terminal_uuid: Option<String>,
    #[serde(default)]
    pub user_renamed: bool,
}

impl WorkspaceState {
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
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }
    }

    #[must_use]
    pub fn new_managed_local(
        name: String,
        policy: WorkspacePolicy,
        initial_cwd: Option<String>,
    ) -> Self {
        let mut session = Self::new_with_initial_cwd(name, initial_cwd);
        session.runtime = WorkspaceRuntime::managed_local(policy);
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
        session.runtime = WorkspaceRuntime::managed_remote(host, policy);
        session
    }

    #[cfg(test)]
    #[must_use]
    pub fn default_for_test() -> Self {
        let mut terminal_recovery = BTreeMap::new();
        terminal_recovery.insert("test-terminal-uuid".to_string(), PaneRecovery::empty_shell());
        Self {
            uuid: "test-session-uuid".to_string(),
            name: "Workspace 1".to_string(),
            layout: LayoutNode::new_terminal_with_uuid("test-terminal-uuid"),
            terminal_recovery,
            active_terminal_uuid: Some("test-terminal-uuid".to_string()),
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }
    }
}

impl WorkspaceState {
    #[must_use]
    pub const fn uses_managed_runtime(&self) -> bool {
        self.runtime.is_managed()
    }

    #[must_use]
    pub const fn is_zoomed(&self) -> bool {
        self.zoomed_terminal_uuid.is_some()
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

    /// Resolve where closing `terminal_uuid` must take effect.
    ///
    /// Returns `Some((runtime_id, runtime_pane_id))` when the close must be sent
    /// to the daemon — a managed workspace with more than one pane whose target
    /// is a live daemon pane. Returns `None` when the close stays client-local:
    /// an unmanaged workspace, the final pane (which closes the whole
    /// workspace), or a workspace with no daemon runtime yet.
    ///
    /// Under the identity invariant (RFC-031) every layout terminal uuid *is*
    /// its server pane id, so the runtime pane id is the terminal uuid itself.
    #[must_use]
    pub fn managed_close_target(&self, terminal_uuid: &str) -> Option<(String, String)> {
        if !self.uses_managed_runtime() || self.layout.terminal_count() <= 1 {
            return None;
        }
        if !self.layout.contains_terminal(terminal_uuid) {
            return None;
        }
        let runtime_id = self.runtime.runtime_id.clone()?;
        Some((runtime_id, terminal_uuid.to_string()))
    }

    pub fn replace_terminal_uuid(&mut self, old_uuid: &str, new_uuid: &str) -> bool {
        if old_uuid == new_uuid || !self.layout.replace_terminal_uuid(old_uuid, new_uuid) {
            return false;
        }

        if let Some(recovery) = self.terminal_recovery.remove(old_uuid) {
            self.terminal_recovery.insert(new_uuid.to_string(), recovery);
        }

        if self.active_terminal_uuid.as_deref() == Some(old_uuid) {
            self.active_terminal_uuid = Some(new_uuid.to_string());
        }

        if self.zoomed_terminal_uuid.as_deref() == Some(old_uuid) {
            self.zoomed_terminal_uuid = Some(new_uuid.to_string());
        }

        true
    }
}

/// Derive a workspace name from the endpoint and working directory.
///
/// Returns `None` when no meaningful name can be derived.
#[must_use]
pub fn auto_name_for_workspace(endpoint: &RuntimeEndpoint, cwd: Option<&str>) -> Option<String> {
    if let RuntimeEndpoint::Remote { host, .. } = endpoint {
        let short = host.split('@').next_back().unwrap_or(host);
        let short = short.split('.').next().unwrap_or(short);
        if !short.is_empty() {
            return Some(short.to_string());
        }
    }
    let path = cwd?;
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    if name.is_empty() { None } else { Some(name.to_string()) }
}

/// Produce a workspace name, always returning a value.
/// Uses `auto_name_for_workspace` when possible, otherwise falls back to
/// "Workspace N" where N is the 1-based count.
#[must_use]
pub fn workspace_display_name(
    endpoint: &RuntimeEndpoint,
    cwd: Option<&str>,
    count: usize,
) -> String {
    auto_name_for_workspace(endpoint, cwd).unwrap_or_else(|| format!("Workspace {count}"))
}

const fn default_left_sidebar_width() -> i32 {
    220
}

const fn default_right_sidebar_width() -> i32 {
    320
}

/// Persistent state of the entire application window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub workspaces: Vec<WorkspaceState>,
    pub active_workspace_index: usize,
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
    /// Reverse index: `"endpoint_key\0runtime_pane_id"` → `(workspace_id, layout_terminal_uuid)`.
    /// Derived from pane bindings; not persisted.
    #[serde(skip)]
    pub pane_reverse_index: std::collections::HashMap<String, (String, String)>,
}

impl PartialEq for WindowState {
    fn eq(&self, other: &Self) -> bool {
        self.workspaces == other.workspaces
            && self.active_workspace_index == other.active_workspace_index
            && self.width == other.width
            && self.height == other.height
            && self.is_maximized == other.is_maximized
            && self.left_sidebar_width == other.left_sidebar_width
            && self.right_sidebar_width == other.right_sidebar_width
            && self.dismissed_runtime_ids == other.dismissed_runtime_ids
    }
}

impl Eq for WindowState {}

impl WindowState {
    /// Composite key for the pane reverse index.
    pub(crate) fn pane_index_key(endpoint_key: &str, runtime_pane_id: &str) -> String {
        format!("{endpoint_key}\0{runtime_pane_id}")
    }

    /// Rebuild the reverse index from all workspace layout terminals.
    ///
    /// Under the identity invariant every layout terminal uuid *is* its server
    /// pane id, so each managed layout terminal maps to itself.
    pub fn rebuild_pane_reverse_index(&mut self) {
        self.pane_reverse_index.clear();
        for session in &self.workspaces {
            if !session.uses_managed_runtime() {
                continue;
            }
            let endpoint_key = session.runtime.endpoint.key();
            for layout_uuid in session.layout.terminal_uuids() {
                let key = Self::pane_index_key(&endpoint_key, &layout_uuid);
                self.pane_reverse_index.insert(key, (session.uuid.clone(), layout_uuid));
            }
        }
    }
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            workspaces: vec![WorkspaceState::new("Workspace 1".into())],
            active_workspace_index: 0,
            width: 900,
            height: 600,
            is_maximized: false,
            left_sidebar_width: default_left_sidebar_width(),
            right_sidebar_width: default_right_sidebar_width(),
            dismissed_runtime_ids: std::collections::BTreeSet::new(),
            pane_reverse_index: std::collections::HashMap::new(),
        }
    }
}

impl WindowState {
    #[cfg(test)]
    #[must_use]
    pub fn default_for_test() -> Self {
        Self {
            workspaces: vec![WorkspaceState::default_for_test()],
            active_workspace_index: 0,
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
    use crate::test_helpers::{hsplit, term};
    use crate::workspace::recovery::{PaneSource, PaneTarget, StartupStep};
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;

    #[test]
    fn session_state_roundtrip() {
        let mut terminal_recovery = BTreeMap::new();
        terminal_recovery.insert(
            "t2".into(),
            PaneRecovery {
                source: PaneSource::Manual,
                target: None,
                startup: vec![StartupStep::SendText {
                    text: "ssh prod && tmux attach -t web".into(),
                    execute: true,
                }],
            },
        );
        let session = WorkspaceState {
            uuid: "s1".into(),
            name: "Work".into(),
            layout: hsplit(term("t1"), term("t2")),
            terminal_recovery,
            active_terminal_uuid: Some("t2".into()),
            input_sync: true,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        };
        let json = serde_json::to_string(&session).unwrap();
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }

    #[test]
    fn session_replace_terminal_uuid_updates_recovery_and_focus() {
        let mut session = WorkspaceState::default_for_test();
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
        let mut session = WorkspaceState::default_for_test();
        let original = session.clone();
        assert!(!session.replace_terminal_uuid("missing", "daemon-pane"));
        assert_eq!(session, original);
    }

    #[test]
    fn window_state_roundtrip() {
        let state = WindowState {
            workspaces: vec![WorkspaceState::new("S1".into())],
            active_workspace_index: 0,
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
    fn mode_field_absent_from_workspace_state() {
        let session =
            WorkspaceState::new_managed_local("Test".into(), WorkspacePolicy::Persistent, None);
        let json = serde_json::to_string(&session).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("mode").is_none(), "mode field must not exist in WorkspaceState");
    }

    #[test]
    fn new_managed_local_session_sets_runtime_metadata() {
        let session =
            WorkspaceState::new_managed_local("Workspace".into(), WorkspacePolicy::Ephemeral, None);
        assert!(session.uses_managed_runtime());
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
        assert_eq!(session.runtime.policy, WorkspacePolicy::Ephemeral);
        assert_eq!(session.layout.terminal_uuids().len(), 1);
    }

    #[test]
    fn default_window_state_is_valid() {
        let state = WindowState::default();
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.active_workspace_index, 0);
        assert!(!state.workspaces[0].uuid.is_empty());
    }

    #[test]
    fn new_session_starts_with_empty_shell_recovery_for_initial_terminal() {
        let session = WorkspaceState::new("Work".into());
        let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
        assert_eq!(session.recovery_for(&terminal_uuid), Some(&PaneRecovery::empty_shell()));
    }

    #[test]
    fn prune_recovery_removes_closed_terminal_entries() {
        let mut session = WorkspaceState {
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
                        source: PaneSource::Manual,
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
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        };
        session.layout = session.layout.remove_terminal("t2").unwrap();
        session.prune_recovery();
        assert!(session.recovery_for("t1").is_some());
        assert!(session.recovery_for("t2").is_none());
        assert!(session.recovery_for("ghost").is_none());
    }

    #[test]
    fn normalize_active_terminal_falls_back_to_first_live_terminal() {
        let mut session = WorkspaceState {
            uuid: "s1".into(),
            name: "Work".into(),
            layout: hsplit(term("t1"), term("t2")),
            terminal_recovery: BTreeMap::default(),
            active_terminal_uuid: Some("ghost".into()),
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
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
                source: PaneSource::Manual,
                target: Some(PaneTarget::RemoteShell {
                    ssh_target: "deploy@example.com".into(),
                    remote_folder: Some("/srv/app".into()),
                }),
                startup: Vec::new(),
            },
        );
        let session = WorkspaceState {
            uuid: "session-1".into(),
            name: "Prod".into(),
            layout: term("t1"),
            terminal_recovery,
            active_terminal_uuid: Some("t1".into()),
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        };
        let json = serde_json::to_string(&session).unwrap();
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(session, restored);
    }

    #[test]
    fn zoom_state_defaults_to_none_when_absent() {
        let json = r#"{"uuid":"s1","name":"W","layout":{"Terminal":{"uuid":"t1"}}}"#;
        let session: WorkspaceState = serde_json::from_str(json).unwrap();
        assert!(session.zoomed_terminal_uuid.is_none());
    }

    #[test]
    fn zoom_state_roundtrips_through_serde() {
        let mut session = WorkspaceState::default_for_test();
        session.zoomed_terminal_uuid = Some("test-terminal-uuid".to_string());
        let json = serde_json::to_string(&session).unwrap();
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.zoomed_terminal_uuid.as_deref(), Some("test-terminal-uuid"));
    }

    #[test]
    fn is_zoomed_reflects_zoom_state() {
        let mut session = WorkspaceState::default_for_test();
        assert!(!session.is_zoomed());
        session.zoomed_terminal_uuid = Some("test-terminal-uuid".to_string());
        assert!(session.is_zoomed());
    }

    #[test]
    fn window_state_rejects_sessions_key() {
        let json = r#"{
            "sessions": [{"uuid":"s1","name":"Old","layout":{"Terminal":{"uuid":"t1"}}}],
            "active_session_index": 0,
            "width": 800,
            "height": 600,
            "is_maximized": false
        }"#;
        let result = serde_json::from_str::<WindowState>(json);
        assert!(
            result.is_err(),
            "the removed sessions/active_session_index keys must no longer parse"
        );
    }

    #[test]
    fn unknown_mode_field_is_ignored_on_deserialize() {
        let json = r#"{
            "uuid": "s1",
            "name": "Legacy",
            "layout": {"Terminal": {"uuid": "t1"}},
            "mode": {"persistent": {"daemon_runtime_id": "rt-1"}}
        }"#;
        let session: WorkspaceState = serde_json::from_str(json).unwrap();
        assert!(!session.uses_managed_runtime(), "unknown mode field should be silently ignored");
    }
}

#[cfg(test)]
mod module_boundary_tests {
    use super::*;
    use crate::workspace::layout::SplitOrientation;

    /// Verify that layout tree operations and state recovery stay consistent
    /// across the module boundary after the layout/state/recovery split.
    #[test]
    fn split_then_prune_recovery_respects_module_boundary() {
        let mut session = WorkspaceState::new("Test".into());
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
    fn new_managed_remote_sets_endpoint_and_runtime() {
        let session = WorkspaceState::new_managed_remote(
            "Work".into(),
            "server.example.com",
            WorkspacePolicy::Persistent,
            Some("/home/user".into()),
        );
        assert!(session.runtime.is_managed());
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::remote("server.example.com"));
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(
            session.layout.terminal_cwd(&session.layout.terminal_uuids()[0]).as_deref(),
            Some("/home/user")
        );
    }

    #[test]
    fn session_color_survives_serde_roundtrip() {
        let mut session = WorkspaceState::new("Test".into());
        session.color = WorkspaceColor::Purple;
        let json = serde_json::to_string(&session).unwrap();
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.color, WorkspaceColor::Purple);
    }

    #[test]
    fn session_color_defaults_to_blue_for_old_state() {
        let mut session = WorkspaceState::new("Test".into());
        session.color = WorkspaceColor::Blue;
        let json = serde_json::to_string(&session).unwrap();
        // Remove color field to simulate old state.
        let json = json.replace(r#","color":"blue""#, "");
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.color, WorkspaceColor::Blue);
    }

    #[test]
    fn session_color_all_has_eight_variants() {
        assert_eq!(WorkspaceColor::ALL.len(), 8);
    }

    #[test]
    fn session_color_css_classes_are_unique() {
        let classes: std::collections::HashSet<_> =
            WorkspaceColor::ALL.iter().map(|c| c.css_class()).collect();
        assert_eq!(classes.len(), 8);
    }

    #[test]
    fn app_css_contains_no_unsupported_at_rules() {
        let css = crate::application::APP_CSS;
        assert!(!css.contains("@media"), "GTK4 CSS does not support @media queries");
    }

    #[test]
    fn accent_css_has_dark_and_light_rules_for_every_color() {
        let dark = crate::application::accent_css_for_dark(true);
        let light = crate::application::accent_css_for_dark(false);
        for color in WorkspaceColor::ALL {
            let cls = color.css_class();
            assert!(dark.contains(&format!(".{cls}")), ".{cls} missing from dark accent CSS");
            assert!(light.contains(&format!(".{cls}")), ".{cls} missing from light accent CSS");
        }
    }

    #[test]
    fn accent_css_uses_different_palette_tiers_for_light_and_dark() {
        let dark = crate::application::accent_css_for_dark(true);
        let light = crate::application::accent_css_for_dark(false);
        assert!(dark.contains("@blue_3"), "dark should use _3 tier");
        assert!(light.contains("@blue_5"), "light should use _5 tier");
        assert!(!dark.contains("_5;"), "dark should not use _5 tier");
        assert!(!light.contains("_3;"), "light should not use _3 tier");
    }

    #[test]
    fn auto_name_from_cwd_uses_basename() {
        let ep = RuntimeEndpoint::Local;
        assert_eq!(
            auto_name_for_workspace(&ep, Some("/home/user/projects/rttx")),
            Some("rttx".into())
        );
    }

    #[test]
    fn auto_name_from_remote_uses_short_hostname() {
        let ep = RuntimeEndpoint::remote("etf@nucbox.local");
        assert_eq!(auto_name_for_workspace(&ep, None), Some("nucbox".into()));
    }

    #[test]
    fn auto_name_remote_without_user_prefix() {
        let ep = RuntimeEndpoint::remote("builder.example.com");
        assert_eq!(auto_name_for_workspace(&ep, None), Some("builder".into()));
    }

    #[test]
    fn auto_name_remote_ignores_cwd() {
        let ep = RuntimeEndpoint::remote("etf@nucbox");
        assert_eq!(auto_name_for_workspace(&ep, Some("/home/etf/work")), Some("nucbox".into()));
    }

    #[test]
    fn auto_name_returns_none_for_local_without_cwd() {
        assert_eq!(auto_name_for_workspace(&RuntimeEndpoint::Local, None), None);
    }

    #[test]
    fn auto_name_returns_none_for_root_path() {
        assert_eq!(auto_name_for_workspace(&RuntimeEndpoint::Local, Some("/")), None);
    }

    #[test]
    fn auto_name_home_directory() {
        let ep = RuntimeEndpoint::Local;
        assert_eq!(auto_name_for_workspace(&ep, Some("/home/user")), Some("user".into()));
    }

    #[test]
    fn workspace_display_name_uses_cwd_basename() {
        assert_eq!(
            workspace_display_name(&RuntimeEndpoint::Local, Some("/home/user/projects"), 1),
            "projects"
        );
    }

    #[test]
    fn workspace_display_name_falls_back_to_counter() {
        assert_eq!(workspace_display_name(&RuntimeEndpoint::Local, None, 3), "Workspace 3");
    }

    #[test]
    fn workspace_display_name_uses_hostname_for_remote() {
        let ep = RuntimeEndpoint::remote("user@builder.example.com");
        assert_eq!(workspace_display_name(&ep, None, 1), "builder");
    }

    #[test]
    fn user_renamed_defaults_to_false_on_deserialize() {
        let json = r#"{"uuid":"u","name":"Test","layout":{"Terminal":{"uuid":"t"}}}"#;
        let session: WorkspaceState = serde_json::from_str(json).unwrap();
        assert!(!session.user_renamed);
    }

    #[test]
    fn user_renamed_roundtrips_through_serde() {
        let mut session = WorkspaceState::new("Test".into());
        session.user_renamed = true;
        let json = serde_json::to_string(&session).unwrap();
        let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert!(restored.user_renamed);
    }
}

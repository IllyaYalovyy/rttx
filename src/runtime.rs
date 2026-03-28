use crate::daemon::DaemonError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Retention policy for a managed workspace runtime.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspacePolicy {
    /// Keep the runtime across detach and daemon restart.
    #[default]
    Persistent,
    /// Do not reconstruct the runtime after daemon restart.
    Ephemeral,
}

impl WorkspacePolicy {
    /// Human-readable label for the sidebar and status UI.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Persistent => "Persistent",
            Self::Ephemeral => "Ephemeral",
        }
    }

    /// Wire value for daemon session creation.
    #[must_use]
    pub const fn as_proto(self) -> i32 {
        match self {
            Self::Persistent => rttx_proto::proto::RuntimePolicy::Persistent as i32,
            Self::Ephemeral => rttx_proto::proto::RuntimePolicy::Ephemeral as i32,
        }
    }
}

/// Daemon endpoint that backs a managed workspace runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeEndpoint {
    /// The local daemon on the current machine.
    #[default]
    Local,
    /// A remote daemon reached through SSH.
    Remote { host: String },
}

impl RuntimeEndpoint {
    /// Stable key string for maps and diagnostics.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Remote { host } => format!("remote:{host}"),
        }
    }
}

/// Persisted managed-runtime metadata attached to a workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRuntime {
    /// Whether this workspace is daemon-managed.
    #[serde(default)]
    pub managed: bool,
    /// Endpoint that hosts the runtime.
    #[serde(default)]
    pub endpoint: RuntimeEndpoint,
    /// Runtime retention policy.
    #[serde(default)]
    pub policy: WorkspacePolicy,
    /// Live daemon runtime ID, if known.
    #[serde(default)]
    pub runtime_id: Option<String>,
    /// Stable layout-terminal UUID -> runtime-pane UUID bindings.
    #[serde(default)]
    pub pane_bindings: BTreeMap<String, String>,
    /// Layout panes that still need a daemon pane assignment.
    #[serde(default)]
    pub pending_layout_panes: BTreeSet<String>,
}

impl WorkspaceRuntime {
    /// Create managed local runtime metadata for a new workspace.
    #[must_use]
    pub fn managed_local(policy: WorkspacePolicy, layout_terminal_uuids: &[String]) -> Self {
        let mut runtime = Self {
            managed: true,
            endpoint: RuntimeEndpoint::Local,
            policy,
            runtime_id: None,
            pane_bindings: BTreeMap::new(),
            pending_layout_panes: BTreeSet::new(),
        };
        runtime.ensure_placeholder_bindings(layout_terminal_uuids);
        runtime
    }

    /// True when this workspace should use the daemon-backed terminal path.
    #[must_use]
    pub const fn is_managed(&self) -> bool {
        self.managed
    }

    /// Ensure every layout terminal has at least a self-binding placeholder.
    pub fn ensure_placeholder_bindings(&mut self, layout_terminal_uuids: &[String]) {
        for terminal_uuid in layout_terminal_uuids {
            if !self.pane_bindings.contains_key(terminal_uuid) {
                self.pane_bindings.insert(terminal_uuid.clone(), terminal_uuid.clone());
                self.pending_layout_panes.insert(terminal_uuid.clone());
            }
        }
        self.pane_bindings.retain(|layout_uuid, _| layout_terminal_uuids.contains(layout_uuid));
        self.pending_layout_panes.retain(|layout_uuid| layout_terminal_uuids.contains(layout_uuid));
    }

    /// Replace a layout terminal UUID while preserving runtime bindings.
    pub fn replace_layout_terminal_uuid(&mut self, old_uuid: &str, new_uuid: &str) {
        if old_uuid == new_uuid {
            return;
        }
        if let Some(bound_runtime_uuid) = self.pane_bindings.remove(old_uuid) {
            self.pane_bindings.insert(new_uuid.to_string(), bound_runtime_uuid);
        }
        if self.pending_layout_panes.remove(old_uuid) {
            self.pending_layout_panes.insert(new_uuid.to_string());
        }
    }

    /// Bind a layout pane to a runtime pane.
    pub fn bind_runtime_pane(&mut self, layout_uuid: &str, runtime_pane_uuid: &str) {
        self.pane_bindings.insert(layout_uuid.to_string(), runtime_pane_uuid.to_string());
        self.pending_layout_panes.remove(layout_uuid);
    }

    /// Whether the layout pane is still waiting for a daemon pane assignment.
    #[must_use]
    pub fn is_layout_pane_pending(&self, layout_uuid: &str) -> bool {
        self.pending_layout_panes.contains(layout_uuid)
    }
}

/// Connection state for a managed workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Starting,
    Connecting,
    Connected,
    Reconnecting { attempt: u32, retry_in_secs: u32 },
    Blocked(ConnectionProblem),
    Disconnected,
    Recovered,
}

impl ConnectionStatus {
    /// Human-readable label for panes and sidebar summaries.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Starting => "Starting".into(),
            Self::Connecting => "Connecting".into(),
            Self::Connected => "Connected".into(),
            Self::Reconnecting { attempt, retry_in_secs } => {
                format!("Reconnecting in {retry_in_secs}s (attempt {attempt})")
            }
            Self::Blocked(problem) => format!("Action Required: {}", problem.label()),
            Self::Disconnected => "Disconnected".into(),
            Self::Recovered => "Recovered".into(),
        }
    }

    /// Whether terminal input should be enabled for this state.
    #[must_use]
    pub const fn accepts_input(&self) -> bool {
        matches!(self, Self::Connected | Self::Recovered)
    }

    /// Short label suitable for compact pane headers.
    #[must_use]
    pub fn short_label(&self) -> String {
        match self {
            Self::Starting => "Starting".into(),
            Self::Connecting => "Connecting".into(),
            Self::Connected => "Connected".into(),
            Self::Reconnecting { retry_in_secs, .. } => format!("Retry {retry_in_secs}s"),
            Self::Blocked(_) => "Action Required".into(),
            Self::Disconnected => "Disconnected".into(),
            Self::Recovered => "Recovered".into(),
        }
    }
}

/// Class of connection problem used by the pure connection state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionProblem {
    DaemonUnavailable,
    VersionMismatch,
    OwnershipConflict,
    PermissionDenied,
    Protocol(String),
    UserActionRequired(String),
}

impl ConnectionProblem {
    /// Whether the manager should auto-retry this problem.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::DaemonUnavailable)
    }

    /// Human-readable short label.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::DaemonUnavailable => "Daemon unavailable".into(),
            Self::VersionMismatch => "Version mismatch".into(),
            Self::OwnershipConflict => "Runtime already owned".into(),
            Self::PermissionDenied => "Permission denied".into(),
            Self::Protocol(detail) | Self::UserActionRequired(detail) => detail.clone(),
        }
    }
}

/// Map a transport/protocol error into reconnectable vs blocked UI policy.
#[must_use]
pub fn classify_connection_problem(error: &DaemonError) -> ConnectionProblem {
    match error {
        DaemonError::VersionMismatch { .. } => ConnectionProblem::VersionMismatch,
        DaemonError::AttachBlocked(_) => ConnectionProblem::OwnershipConflict,
        DaemonError::ServerError { code, message } if *code == 8 => {
            ConnectionProblem::UserActionRequired(message.clone())
        }
        DaemonError::ServerError { code, .. } if *code == 9 => ConnectionProblem::OwnershipConflict,
        DaemonError::ServerError { code, message } if *code == 4 => {
            ConnectionProblem::UserActionRequired(message.clone())
        }
        DaemonError::ServerError { message, .. } => {
            ConnectionProblem::UserActionRequired(message.clone())
        }
        DaemonError::Io(_) | DaemonError::Disconnected => ConnectionProblem::DaemonUnavailable,
        DaemonError::Frame(frame_error) => ConnectionProblem::Protocol(frame_error.to_string()),
        DaemonError::UnexpectedMessage => {
            ConnectionProblem::Protocol("Unexpected daemon message".into())
        }
    }
}

/// Pure event for advancing a connection state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    Started,
    Connected,
    Lost,
    RetryScheduled { attempt: u32, retry_in_secs: u32 },
    Failed(ConnectionProblem),
    Recovered,
}

/// Advance a connection status without involving GTK or daemon I/O.
#[must_use]
pub fn advance_connection_status(
    _current: &ConnectionStatus,
    event: ConnectionEvent,
) -> ConnectionStatus {
    match event {
        ConnectionEvent::Started => ConnectionStatus::Starting,
        ConnectionEvent::Connected => ConnectionStatus::Connected,
        ConnectionEvent::Lost => ConnectionStatus::Disconnected,
        ConnectionEvent::RetryScheduled { attempt, retry_in_secs } => {
            ConnectionStatus::Reconnecting { attempt, retry_in_secs }
        }
        ConnectionEvent::Recovered => ConnectionStatus::Recovered,
        ConnectionEvent::Failed(problem) => ConnectionStatus::Blocked(problem),
    }
}

/// UI-facing connection banner configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPresentation {
    pub header_label: String,
    pub banner_title: String,
    pub banner_body: String,
    pub banner_visible: bool,
    pub show_retry: bool,
    pub show_close: bool,
    pub show_edit_connection: bool,
    pub input_enabled: bool,
}

/// UI-facing close/detach/terminate action configuration for a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceActionPresentation {
    pub title: String,
    pub body: String,
    pub close_label: String,
    pub show_detach_runtime: bool,
    pub show_terminate_runtime: bool,
}

/// Render workspace action semantics into user-facing copy.
#[must_use]
pub fn present_workspace_actions(
    policy: Option<WorkspacePolicy>,
    runtime_attached: bool,
    pane_count: usize,
) -> WorkspaceActionPresentation {
    let pane_summary = if pane_count > 1 {
        format!("This workspace has {pane_count} panes. ")
    } else {
        String::new()
    };

    match (policy, runtime_attached) {
        (Some(WorkspacePolicy::Persistent), true) => WorkspaceActionPresentation {
            title: "Workspace Actions".into(),
            body: format!(
                "{pane_summary}Close Workspace removes this workspace from rttx and deletes its \
                 local metadata. The persistent runtime keeps running on the daemon. Detach \
                 Runtime keeps the workspace so you can reconnect later. Terminate Runtime stops \
                 the runtime and its running processes."
            ),
            close_label: "Close Workspace".into(),
            show_detach_runtime: true,
            show_terminate_runtime: true,
        },
        (Some(WorkspacePolicy::Ephemeral), true) => WorkspaceActionPresentation {
            title: "Workspace Actions".into(),
            body: format!(
                "{pane_summary}Close Workspace removes this workspace from rttx and deletes its \
                 local metadata. This workspace uses an ephemeral runtime, so detaching the last \
                 client will terminate that runtime automatically. Detach Runtime keeps the \
                 workspace visible but still ends the runtime when this is the last attached \
                 client. Terminate Runtime stops it immediately."
            ),
            close_label: "Close Workspace".into(),
            show_detach_runtime: true,
            show_terminate_runtime: true,
        },
        (Some(_), false) => WorkspaceActionPresentation {
            title: "Close Workspace?".into(),
            body: format!(
                "{pane_summary}This workspace is not attached to a runtime right now. Close \
                 Workspace removes its local metadata from rttx."
            ),
            close_label: "Close Workspace".into(),
            show_detach_runtime: false,
            show_terminate_runtime: false,
        },
        (None, _) => {
            let body = if pane_count > 1 {
                format!(
                    "This workspace has {pane_count} panes. All panes and their running \
                     processes will be closed."
                )
            } else {
                "This workspace will be closed.".into()
            };
            WorkspaceActionPresentation {
                title: "Close Workspace?".into(),
                body,
                close_label: "Close Workspace".into(),
                show_detach_runtime: false,
                show_terminate_runtime: false,
            }
        }
    }
}

/// Render a connection state into user-facing copy and control visibility.
#[must_use]
pub fn present_connection_status(
    endpoint: &RuntimeEndpoint,
    status: &ConnectionStatus,
) -> ConnectionPresentation {
    let endpoint_label = match endpoint {
        RuntimeEndpoint::Local => "local daemon".to_string(),
        RuntimeEndpoint::Remote { host } => host.clone(),
    };
    let show_edit_connection = matches!(endpoint, RuntimeEndpoint::Remote { .. })
        && matches!(status, ConnectionStatus::Blocked(_));

    let (banner_title, banner_body, banner_visible, show_retry, show_close) = match status {
        ConnectionStatus::Starting => (
            "Starting local daemon".into(),
            "This workspace is waiting for the local daemon to come online.".into(),
            true,
            false,
            true,
        ),
        ConnectionStatus::Connecting => (
            format!("Connecting to {endpoint_label}"),
            "This workspace is attaching to its runtime.".into(),
            true,
            false,
            true,
        ),
        ConnectionStatus::Connected => (String::new(), String::new(), false, false, false),
        ConnectionStatus::Reconnecting { attempt, retry_in_secs } => (
            format!("Reconnecting in {retry_in_secs}s"),
            format!(
                "The runtime connection dropped. rttx will retry automatically (attempt {attempt})."
            ),
            true,
            true,
            true,
        ),
        ConnectionStatus::Blocked(problem) => {
            (format!("Action required for {endpoint_label}"), problem.label(), true, true, true)
        }
        ConnectionStatus::Disconnected => (
            format!("Disconnected from {endpoint_label}"),
            "The runtime is unavailable right now.".into(),
            true,
            true,
            true,
        ),
        ConnectionStatus::Recovered => (
            "Connection restored".into(),
            format!("The workspace is connected to {endpoint_label} again."),
            true,
            false,
            false,
        ),
    };

    ConnectionPresentation {
        header_label: status.short_label(),
        banner_title,
        banner_body,
        banner_visible,
        show_retry,
        show_close,
        show_edit_connection,
        input_enabled: status.accepts_input(),
    }
}

/// Deterministic, non-destructive binding reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingReconciliation {
    /// Valid layout -> runtime bindings after reconciliation.
    pub bindings: BTreeMap<String, String>,
    /// Runtime panes that still need recovered GUI panes.
    pub recovered_runtime_panes: Vec<String>,
    /// Layout panes that have no live runtime pane.
    pub disconnected_layout_panes: Vec<String>,
}

/// Reconcile persisted bindings against the live runtime pane inventory.
///
/// This never infers bindings by position alone. If a binding is missing,
/// only an exact same-ID match is accepted automatically.
#[must_use]
pub fn reconcile_bindings(
    layout_terminal_uuids: &[String],
    persisted_bindings: &BTreeMap<String, String>,
    runtime_pane_uuids: &[String],
) -> BindingReconciliation {
    let layout_set: BTreeSet<_> = layout_terminal_uuids.iter().cloned().collect();
    let runtime_set: BTreeSet<_> = runtime_pane_uuids.iter().cloned().collect();
    let mut bindings = BTreeMap::new();
    let mut claimed_runtime_panes = BTreeSet::new();

    for layout_uuid in layout_terminal_uuids {
        if let Some(runtime_uuid) = persisted_bindings.get(layout_uuid)
            && runtime_set.contains(runtime_uuid)
            && claimed_runtime_panes.insert(runtime_uuid.clone())
        {
            bindings.insert(layout_uuid.clone(), runtime_uuid.clone());
            continue;
        }

        if runtime_set.contains(layout_uuid) && claimed_runtime_panes.insert(layout_uuid.clone()) {
            bindings.insert(layout_uuid.clone(), layout_uuid.clone());
        }
    }

    let recovered_runtime_panes =
        runtime_set.difference(&claimed_runtime_panes).cloned().collect::<Vec<_>>();
    let disconnected_layout_panes =
        layout_set.difference(&bindings.keys().cloned().collect()).cloned().collect::<Vec<_>>();

    BindingReconciliation { bindings, recovered_runtime_panes, disconnected_layout_panes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_runtime_initializes_placeholder_bindings() {
        let runtime = WorkspaceRuntime::managed_local(
            WorkspacePolicy::Ephemeral,
            &["pane-a".into(), "pane-b".into()],
        );

        assert!(runtime.is_managed());
        assert_eq!(runtime.policy, WorkspacePolicy::Ephemeral);
        assert_eq!(runtime.pane_bindings["pane-a"], "pane-a");
        assert_eq!(runtime.pane_bindings["pane-b"], "pane-b");
        assert!(runtime.is_layout_pane_pending("pane-a"));
        assert!(runtime.is_layout_pane_pending("pane-b"));
    }

    #[test]
    fn replacing_layout_terminal_uuid_preserves_binding() {
        let mut runtime =
            WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent, &["old".into()]);
        runtime.bind_runtime_pane("old", "daemon-pane");

        runtime.replace_layout_terminal_uuid("old", "new");

        assert_eq!(runtime.pane_bindings.get("old"), None);
        assert_eq!(runtime.pane_bindings.get("new").map(String::as_str), Some("daemon-pane"));
        assert!(!runtime.is_layout_pane_pending("new"));
    }

    #[test]
    fn reconciliation_keeps_explicit_bindings_and_marks_missing_objects() {
        let layout = vec!["left".into(), "right".into()];
        let bindings = BTreeMap::from([
            ("left".into(), "pane-1".into()),
            ("right".into(), "missing-pane".into()),
        ]);
        let runtime_panes = vec!["pane-1".into(), "pane-2".into()];

        let reconciled = reconcile_bindings(&layout, &bindings, &runtime_panes);

        assert_eq!(reconciled.bindings.len(), 1);
        assert_eq!(reconciled.bindings["left"], "pane-1");
        assert_eq!(reconciled.recovered_runtime_panes, vec!["pane-2"]);
        assert_eq!(reconciled.disconnected_layout_panes, vec!["right"]);
    }

    #[test]
    fn reconciliation_accepts_same_id_match_without_position_inference() {
        let layout = vec!["pane-a".into()];
        let reconciled = reconcile_bindings(&layout, &BTreeMap::new(), &["pane-a".into()]);
        assert_eq!(reconciled.bindings["pane-a"], "pane-a");
        assert!(reconciled.recovered_runtime_panes.is_empty());
        assert!(reconciled.disconnected_layout_panes.is_empty());
    }

    #[test]
    fn classify_connection_problem_marks_only_daemon_unavailable_as_transient() {
        assert!(classify_connection_problem(&DaemonError::Disconnected).is_transient());
        assert!(
            !classify_connection_problem(&DaemonError::VersionMismatch { server: 2, client: 1 })
                .is_transient()
        );
        assert!(
            !classify_connection_problem(&DaemonError::ServerError {
                code: 9,
                message: "owned".into(),
            })
            .is_transient()
        );
    }

    #[test]
    fn connection_state_machine_distinguishes_retryable_and_blocked_failures() {
        let reconnecting = advance_connection_status(
            &ConnectionStatus::Connecting,
            ConnectionEvent::RetryScheduled { attempt: 1, retry_in_secs: 3 },
        );
        assert_eq!(reconnecting, ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 3 });

        let blocked = advance_connection_status(
            &ConnectionStatus::Connecting,
            ConnectionEvent::Failed(ConnectionProblem::OwnershipConflict),
        );
        assert_eq!(blocked, ConnectionStatus::Blocked(ConnectionProblem::OwnershipConflict));
    }

    #[test]
    fn connection_presentation_hides_banner_only_when_connected() {
        let presentation =
            present_connection_status(&RuntimeEndpoint::Local, &ConnectionStatus::Connected);
        assert!(!presentation.banner_visible);
        assert!(presentation.input_enabled);
        assert!(!presentation.show_retry);
    }

    #[test]
    fn connection_presentation_for_reconnecting_remote_shows_countdown_and_controls() {
        let presentation = present_connection_status(
            &RuntimeEndpoint::Remote { host: "builder.example".into() },
            &ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
        );

        assert_eq!(presentation.header_label, "Retry 4s");
        assert_eq!(presentation.banner_title, "Reconnecting in 4s");
        assert!(presentation.banner_body.contains("attempt 2"));
        assert!(presentation.banner_visible);
        assert!(presentation.show_retry);
        assert!(presentation.show_close);
        assert!(!presentation.show_edit_connection);
        assert!(!presentation.input_enabled);
    }

    #[test]
    fn connection_presentation_for_blocked_remote_allows_editing() {
        let presentation = present_connection_status(
            &RuntimeEndpoint::Remote { host: "builder.example".into() },
            &ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
        );

        assert!(presentation.banner_visible);
        assert!(presentation.show_retry);
        assert!(presentation.show_close);
        assert!(presentation.show_edit_connection);
        assert!(!presentation.input_enabled);
        assert!(presentation.banner_body.contains("Permission denied"));
    }

    #[test]
    fn workspace_actions_for_persistent_runtime_offer_detach_and_terminate() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Persistent), true, 2);

        assert_eq!(presentation.title, "Workspace Actions");
        assert_eq!(presentation.close_label, "Close Workspace");
        assert!(presentation.show_detach_runtime);
        assert!(presentation.show_terminate_runtime);
        assert!(presentation.body.contains("persistent runtime keeps running"));
        assert!(presentation.body.contains("reconnect later"));
        assert!(presentation.body.contains("2 panes"));
    }

    #[test]
    fn workspace_actions_for_ephemeral_runtime_warn_about_last_detach() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Ephemeral), true, 1);

        assert!(presentation.show_detach_runtime);
        assert!(presentation.show_terminate_runtime);
        assert!(presentation.body.contains("ephemeral runtime"));
        assert!(presentation.body.contains("last attached client"));
    }

    #[test]
    fn workspace_actions_for_detached_managed_workspace_only_offer_close() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Persistent), false, 1);

        assert_eq!(presentation.title, "Close Workspace?");
        assert!(!presentation.show_detach_runtime);
        assert!(!presentation.show_terminate_runtime);
        assert!(presentation.body.contains("not attached to a runtime"));
    }

    #[test]
    fn workspace_actions_for_unmanaged_workspace_keep_simple_close_copy() {
        let presentation = present_workspace_actions(None, false, 3);

        assert_eq!(presentation.title, "Close Workspace?");
        assert_eq!(presentation.close_label, "Close Workspace");
        assert!(!presentation.show_detach_runtime);
        assert!(!presentation.show_terminate_runtime);
        assert!(presentation.body.contains("3 panes"));
        assert!(presentation.body.contains("running processes"));
    }
}

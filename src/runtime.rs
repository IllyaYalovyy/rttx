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
    Reconnecting { attempt: u32 },
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
            Self::Reconnecting { attempt } => format!("Reconnecting ({attempt})"),
            Self::Blocked(problem) => format!("Action Required: {}", problem.label()),
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
    RetryScheduled { attempt: u32 },
    Failed(ConnectionProblem),
    Recovered,
}

/// Advance a connection status without involving GTK or daemon I/O.
#[must_use]
pub fn advance_connection_status(
    current: &ConnectionStatus,
    event: ConnectionEvent,
) -> ConnectionStatus {
    match event {
        ConnectionEvent::Started => ConnectionStatus::Starting,
        ConnectionEvent::Connected => ConnectionStatus::Connected,
        ConnectionEvent::Lost => ConnectionStatus::Disconnected,
        ConnectionEvent::RetryScheduled { attempt } => ConnectionStatus::Reconnecting { attempt },
        ConnectionEvent::Recovered => ConnectionStatus::Recovered,
        ConnectionEvent::Failed(problem) if problem.is_transient() => match current {
            ConnectionStatus::Reconnecting { attempt } => {
                ConnectionStatus::Reconnecting { attempt: attempt.saturating_add(1) }
            }
            _ => ConnectionStatus::Reconnecting { attempt: 1 },
        },
        ConnectionEvent::Failed(problem) => ConnectionStatus::Blocked(problem),
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
            ConnectionEvent::Failed(ConnectionProblem::DaemonUnavailable),
        );
        assert_eq!(reconnecting, ConnectionStatus::Reconnecting { attempt: 1 });

        let blocked = advance_connection_status(
            &ConnectionStatus::Connecting,
            ConnectionEvent::Failed(ConnectionProblem::OwnershipConflict),
        );
        assert_eq!(blocked, ConnectionStatus::Blocked(ConnectionProblem::OwnershipConflict));
    }
}

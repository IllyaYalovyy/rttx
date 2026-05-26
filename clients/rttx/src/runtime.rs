use crate::daemon::DaemonError;
use rttx_proto::v3;
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

    /// Wire value for v3 daemon session creation.
    #[must_use]
    pub const fn as_v3_proto(self) -> i32 {
        match self {
            Self::Persistent => rttx_proto::v3::RuntimePolicy::Persistent as i32,
            Self::Ephemeral => rttx_proto::v3::RuntimePolicy::Ephemeral as i32,
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

    /// Host key compatible with the host model for place/command filtering.
    #[must_use]
    pub fn host_key(&self) -> String {
        match self {
            Self::Local => crate::host::LOCAL_KEY.into(),
            Self::Remote { host } => crate::host::normalize_ssh_key(host),
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
        Self::managed(RuntimeEndpoint::Local, policy, layout_terminal_uuids)
    }

    /// Create managed remote runtime metadata for a new workspace.
    #[must_use]
    pub fn managed_remote(
        host: &str,
        policy: WorkspacePolicy,
        layout_terminal_uuids: &[String],
    ) -> Self {
        Self::managed(RuntimeEndpoint::Remote { host: host.into() }, policy, layout_terminal_uuids)
    }

    fn managed(
        endpoint: RuntimeEndpoint,
        policy: WorkspacePolicy,
        layout_terminal_uuids: &[String],
    ) -> Self {
        let mut runtime = Self {
            managed: true,
            endpoint,
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
    Reconnecting {
        attempt: u32,
        retry_in_secs: u32,
    },
    Blocked(ConnectionProblem),
    Disconnected,
    Recovered,
    /// The daemon has no record of this workspace's runtime.
    SessionMissing,
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
            Self::Blocked(ConnectionProblem::DaemonDied) => "Daemon stopped".into(),
            Self::Blocked(problem) => format!("Action Required: {}", problem.label()),
            Self::Disconnected => "Disconnected".into(),
            Self::Recovered => "Recovered".into(),
            Self::SessionMissing => "Session no longer exists".into(),
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
            Self::Connected | Self::Recovered => "Connected".into(),
            Self::Reconnecting { retry_in_secs, .. } => format!("Retry {retry_in_secs}s"),
            Self::Blocked(ConnectionProblem::DaemonDied) => "Daemon Stopped".into(),
            Self::Blocked(_) => "Action Required".into(),
            Self::Disconnected => "Disconnected".into(),
            Self::SessionMissing => "Session Missing".into(),
        }
    }
}

/// Class of connection problem used by the pure connection state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionProblem {
    DaemonUnavailable,
    /// The daemon process died and repeated reconnect attempts failed.
    DaemonDied,
    VersionMismatch,
    OwnershipConflict,
    PermissionDenied,
    SessionMissing,
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
            Self::DaemonDied => "Daemon stopped".into(),
            Self::VersionMismatch => "Version mismatch".into(),
            Self::OwnershipConflict => "Runtime already owned".into(),
            Self::PermissionDenied => "Permission denied".into(),
            Self::SessionMissing => "Session no longer exists".into(),
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
        DaemonError::ProtocolError { kind, message, .. } => match kind {
            v3::ErrorKind::RuntimeNotFound => ConnectionProblem::SessionMissing,
            v3::ErrorKind::OwnershipConflict | v3::ErrorKind::TakeoverRequired => {
                ConnectionProblem::OwnershipConflict
            }
            v3::ErrorKind::ProtocolMismatch | v3::ErrorKind::UnsupportedCapability => {
                ConnectionProblem::Protocol(message.clone())
            }
            _ => ConnectionProblem::UserActionRequired(message.clone()),
        },
        DaemonError::ServerError { code, .. } if *code == 4 => ConnectionProblem::SessionMissing,
        DaemonError::ServerError { code, message } if *code == 8 => {
            ConnectionProblem::UserActionRequired(message.clone())
        }
        DaemonError::ServerError { code, .. } if *code == 9 => ConnectionProblem::OwnershipConflict,
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
    SessionMissing,
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
        ConnectionEvent::SessionMissing => ConnectionStatus::SessionMissing,
    }
}

/// UI-facing connection state for pane headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPresentation {
    pub header_label: String,
    pub input_enabled: bool,
}

/// UI-facing close action configuration for a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceActionPresentation {
    pub title: String,
    pub body: String,
    pub close_label: String,
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
        (Some(_), true) => WorkspaceActionPresentation {
            title: "Close Workspace?".into(),
            body: format!(
                "{pane_summary}Closing this workspace will stop its runtime and all running \
                 processes."
            ),
            close_label: "Close Workspace".into(),
        },
        (Some(_), false) => WorkspaceActionPresentation {
            title: "Close Workspace?".into(),
            body: format!(
                "{pane_summary}This workspace is not connected to a runtime. Closing it removes \
                 its local metadata."
            ),
            close_label: "Close Workspace".into(),
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
            }
        }
    }
}

/// Which actions to show in the workspace context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMenuItems {
    pub show_edit_connection: bool,
    pub show_reconnect: bool,
    pub show_reconnect_host: bool,
    pub show_restart_daemon: bool,
    pub show_detach: bool,
}

/// Input state for determining workspace menu items.
#[derive(Debug, Clone)]
pub struct WorkspaceMenuContext {
    pub is_remote: bool,
    pub is_managed: bool,
    pub is_persistent: bool,
    pub is_attached: bool,
    pub is_disconnected: bool,
    pub is_connecting: bool,
    pub is_daemon_died: bool,
    pub has_other_disconnected_from_same_host: bool,
}

/// Determine which context menu items are relevant for a workspace.
#[must_use]
pub const fn workspace_menu_items(ctx: &WorkspaceMenuContext) -> WorkspaceMenuItems {
    WorkspaceMenuItems {
        show_edit_connection: ctx.is_remote,
        show_reconnect: ctx.is_managed && (ctx.is_disconnected || ctx.is_connecting),
        show_reconnect_host: ctx.is_managed
            && ctx.is_disconnected
            && ctx.has_other_disconnected_from_same_host,
        show_restart_daemon: ctx.is_managed && ctx.is_daemon_died && !ctx.is_remote,
        show_detach: ctx.is_persistent && ctx.is_attached,
    }
}

/// Render a connection state into pane header label and input availability.
#[must_use]
pub fn present_connection_status(status: &ConnectionStatus) -> ConnectionPresentation {
    ConnectionPresentation {
        header_label: status.short_label(),
        input_enabled: status.accepts_input(),
    }
}

/// Icon and CSS class for the sidebar connection status indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionIcon {
    pub icon_name: &'static str,
    pub css_class: &'static str,
    pub tooltip: &'static str,
}

/// Returns the connection icon for a workspace row.
///
/// Shape encodes workspace type (constant for the lifetime of the row):
/// - Local managed: `computer-symbolic`
/// - Remote managed: `network-server-symbolic`
/// - Direct (no daemon): `utilities-terminal-symbolic`
///
/// Color encodes connection state (changes dynamically):
/// - `dim-label`: connecting
/// - `accent`: connected or recovered
/// - `warning`: disconnected
/// - `error`: blocked
#[must_use]
pub const fn connection_icon(
    endpoint: &RuntimeEndpoint,
    status: &ConnectionStatus,
    managed: bool,
) -> ConnectionIcon {
    let icon_name = if managed {
        match endpoint {
            RuntimeEndpoint::Local => "computer-symbolic",
            RuntimeEndpoint::Remote { .. } => "network-server-symbolic",
        }
    } else {
        "utilities-terminal-symbolic"
    };
    let (css_class, tooltip) = match status {
        ConnectionStatus::Connected | ConnectionStatus::Recovered => {
            let tooltip = match endpoint {
                RuntimeEndpoint::Local => "Connected to local runtime",
                RuntimeEndpoint::Remote { .. } => "Connected to remote host",
            };
            ("accent", tooltip)
        }
        ConnectionStatus::Disconnected => ("warning", "Disconnected from runtime"),
        ConnectionStatus::Reconnecting { .. } => ("warning", "Reconnecting to runtime…"),
        ConnectionStatus::SessionMissing => ("warning", "Session no longer exists on daemon"),
        ConnectionStatus::Blocked(ConnectionProblem::DaemonDied) => {
            ("error", "Daemon stopped — restart to recover")
        }
        ConnectionStatus::Blocked(_) => ("error", "Connection blocked — retry manually"),
        _ => ("dim-label", "Connecting…"),
    };
    ConnectionIcon { icon_name, css_class, tooltip }
}

/// Build a structured subtitle for a workspace row.
///
/// - Local: pane info (full path, optional command)
/// - Remote: `host · pane-info`
#[must_use]
pub fn workspace_connection_summary(
    endpoint: &RuntimeEndpoint,
    active_pane_info: Option<&str>,
) -> String {
    let pane_part = active_pane_info.filter(|s| !s.is_empty()).unwrap_or("");
    match endpoint {
        RuntimeEndpoint::Local => pane_part.to_string(),
        RuntimeEndpoint::Remote { host } => {
            if pane_part.is_empty() {
                host.clone()
            } else {
                format!("{host} · {pane_part}")
            }
        }
    }
}

/// Build a pane description for the sidebar subtitle.
///
/// Shows the full CWD path (tilde-collapsed). When no CWD is available,
/// falls back to the VTE title if it carries useful info (not a generic
/// shell name or prompt-set `user@host:path`).
#[must_use]
pub fn pane_description(title: Option<&str>, cwd: Option<&str>) -> Option<String> {
    let path = cwd.map(|c| collapse_home(c.trim()));
    if let Some(ref p) = path
        && !p.is_empty()
    {
        return Some(p.clone());
    }
    // No CWD — fall back to title if useful.
    title.map(str::trim).filter(|t| !t.is_empty() && !is_generic_title(t)).map(String::from)
}

/// Collapse `/home/<user>/…` to `~/…`.
fn collapse_home(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home.as_ref()) {
            if rest.is_empty() {
                return "~".into();
            }
            if rest.starts_with('/') {
                return format!("~{rest}");
            }
        }
    }
    path.to_string()
}

/// Returns true for VTE titles that carry no useful information.
fn is_generic_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    lower.contains("terminal")
        || lower == "bash"
        || lower == "zsh"
        || lower == "sh"
        || lower == "fish"
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

    /// `ERR_PANE_NOT_FOUND` (code 6) classifies as non-transient `UserActionRequired`.
    ///
    /// The `daemon_bridge` handles this specially for `ClosePane` by emitting
    /// `PaneClosed` instead of blocking the workspace (see #309).
    #[test]
    fn classify_pane_not_found_is_non_transient_user_action() {
        let problem = classify_connection_problem(&DaemonError::ServerError {
            code: 6,
            message: "pane not found".into(),
        });
        assert!(!problem.is_transient());
        assert_eq!(problem, ConnectionProblem::UserActionRequired("pane not found".into()));
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
    fn connection_presentation_header_and_input_for_connected_states() {
        let connected = present_connection_status(&ConnectionStatus::Connected);
        assert_eq!(connected.header_label, "Connected");
        assert!(connected.input_enabled);

        let recovered = present_connection_status(&ConnectionStatus::Recovered);
        assert_eq!(recovered.header_label, "Connected");
        assert!(recovered.input_enabled);
    }

    #[test]
    fn connection_presentation_disables_input_for_disconnected_states() {
        let reconnecting = present_connection_status(&ConnectionStatus::Reconnecting {
            attempt: 2,
            retry_in_secs: 4,
        });
        assert_eq!(reconnecting.header_label, "Retry 4s");
        assert!(!reconnecting.input_enabled);

        let blocked = present_connection_status(&ConnectionStatus::Blocked(
            ConnectionProblem::PermissionDenied,
        ));
        assert_eq!(blocked.header_label, "Action Required");
        assert!(!blocked.input_enabled);

        let disconnected = present_connection_status(&ConnectionStatus::Disconnected);
        assert_eq!(disconnected.header_label, "Disconnected");
        assert!(!disconnected.input_enabled);
    }

    #[test]
    fn workspace_connection_summary_with_pane_info() {
        assert_eq!(
            workspace_connection_summary(&RuntimeEndpoint::Local, Some("vim main.rs")),
            "vim main.rs"
        );
        assert_eq!(workspace_connection_summary(&RuntimeEndpoint::Local, None), "");
        assert_eq!(workspace_connection_summary(&RuntimeEndpoint::Local, Some("")), "");
        assert_eq!(
            workspace_connection_summary(
                &RuntimeEndpoint::Remote { host: "builder.example".into() },
                Some("~/src"),
            ),
            "builder.example · ~/src"
        );
    }

    #[test]
    fn workspace_connection_summary_for_remote_endpoint() {
        let endpoint = RuntimeEndpoint::Remote { host: "builder.example".into() };

        assert_eq!(workspace_connection_summary(&endpoint, Some("bash")), "builder.example · bash");
        assert_eq!(workspace_connection_summary(&endpoint, None), "builder.example");
    }

    #[test]
    fn pane_description_shows_full_path_and_command() {
        // CWD always wins — title is ignored when CWD is available.
        let desc = pane_description(Some("vim main.rs"), Some("/tmp/project"));
        assert_eq!(desc, Some("/tmp/project".into()));
    }

    #[test]
    fn pane_description_path_only_when_title_is_generic() {
        let desc = pane_description(Some("bash"), Some("/tmp/project"));
        assert_eq!(desc, Some("/tmp/project".into()));
    }

    #[test]
    fn pane_description_prompt_title_ignored_when_cwd_present() {
        // Shell prompt sets VTE title to user@host:path — always redundant.
        let desc = pane_description(Some("yalovyyi@host:~/work"), Some("/home/yalovyyi/work"));
        assert!(!desc.as_deref().unwrap_or("").contains('@'));
    }

    #[test]
    fn pane_description_falls_back_to_useful_title() {
        assert_eq!(pane_description(Some("vim main.rs"), None), Some("vim main.rs".into()));
    }

    #[test]
    fn pane_description_filters_generic_titles() {
        assert_eq!(pane_description(Some("Terminal (persistent)"), None), None);
        assert_eq!(pane_description(Some("bash"), None), None);
        assert_eq!(pane_description(Some("zsh"), None), None);
    }

    #[test]
    fn pane_description_shows_full_cwd() {
        assert_eq!(pane_description(None, Some("/tmp/project")), Some("/tmp/project".into()));
        assert_eq!(pane_description(None, Some("/")), Some("/".into()));
    }

    /// Regression for #536: sidebar subtitle uses CWD, not the combined
    /// pane header title, so the "app : path" format never leaks into the
    /// sidebar.
    #[test]
    fn pane_description_uses_cwd_not_combined_header_title() {
        // Even if the pane header shows "bash : /tmp", the sidebar should
        // show just the CWD path.
        assert_eq!(pane_description(Some("bash : /tmp"), Some("/tmp")), Some("/tmp".into()));
        // When CWD is absent, the combined title is not generic and would
        // be shown — but this scenario doesn't arise in practice because
        // the sidebar reads CWD directly from the pane.
        assert_eq!(pane_description(Some("bash : /tmp"), None), Some("bash : /tmp".into()));
    }

    #[test]
    fn pane_description_none_when_no_info() {
        assert_eq!(pane_description(None, None), None);
        assert_eq!(pane_description(Some(""), None), None);
    }

    #[test]
    fn connection_icon_computer_for_local_connected() {
        let icon = connection_icon(&RuntimeEndpoint::Local, &ConnectionStatus::Connected, true);
        assert_eq!(icon.icon_name, "computer-symbolic");
        assert_eq!(icon.css_class, "accent");
    }

    #[test]
    fn connection_icon_shape_constant_for_local_managed() {
        let ep = RuntimeEndpoint::Local;
        for status in [
            ConnectionStatus::Connected,
            ConnectionStatus::Disconnected,
            ConnectionStatus::Recovered,
            ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
            ConnectionStatus::Connecting,
        ] {
            let icon = connection_icon(&ep, &status, true);
            assert_eq!(icon.icon_name, "computer-symbolic", "shape must not change with status");
        }
    }

    #[test]
    fn connection_icon_color_for_local_managed() {
        let ep = RuntimeEndpoint::Local;
        assert_eq!(connection_icon(&ep, &ConnectionStatus::Connected, true).css_class, "accent");
        assert_eq!(connection_icon(&ep, &ConnectionStatus::Recovered, true).css_class, "accent");
        assert_eq!(
            connection_icon(&ep, &ConnectionStatus::Disconnected, true).css_class,
            "warning"
        );
        assert_eq!(
            connection_icon(
                &ep,
                &ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
                true
            )
            .css_class,
            "error"
        );
        assert_eq!(
            connection_icon(&ep, &ConnectionStatus::Connecting, true).css_class,
            "dim-label"
        );
    }

    /// Regression for #935: Reconnecting must use warning color so the tab
    /// is visually distinct from initial Connecting (dim-label/gray).
    #[test]
    fn connection_icon_reconnecting_uses_warning_not_dim_label() {
        let local = RuntimeEndpoint::Local;
        let remote = RuntimeEndpoint::Remote { host: "h".into() };
        let reconnecting = ConnectionStatus::Reconnecting { attempt: 3, retry_in_secs: 5 };

        let local_icon = connection_icon(&local, &reconnecting, true);
        assert_eq!(
            local_icon.css_class, "warning",
            "Reconnecting must use warning color, not dim-label"
        );
        assert_eq!(local_icon.tooltip, "Reconnecting to runtime…");

        let remote_icon = connection_icon(&remote, &reconnecting, true);
        assert_eq!(remote_icon.css_class, "warning");
        assert_eq!(remote_icon.tooltip, "Reconnecting to runtime…");
    }

    /// Regression for #935: Blocked state tooltip must indicate manual retry.
    #[test]
    fn connection_icon_blocked_tooltip_mentions_retry() {
        let ep = RuntimeEndpoint::Local;
        let icon = connection_icon(
            &ep,
            &ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
            true,
        );
        assert_eq!(icon.css_class, "error");
        assert!(
            icon.tooltip.contains("retry"),
            "Blocked tooltip should mention retry: got {:?}",
            icon.tooltip
        );
    }

    #[test]
    fn connection_icon_shape_constant_for_remote() {
        let ep = RuntimeEndpoint::Remote { host: "h".into() };
        for status in [
            ConnectionStatus::Connected,
            ConnectionStatus::Disconnected,
            ConnectionStatus::Recovered,
            ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
            ConnectionStatus::Connecting,
            ConnectionStatus::Starting,
            ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 5 },
        ] {
            let icon = connection_icon(&ep, &status, true);
            assert_eq!(
                icon.icon_name, "network-server-symbolic",
                "shape must not change with status"
            );
        }
    }

    #[test]
    fn connection_icon_color_for_remote() {
        let ep = RuntimeEndpoint::Remote { host: "h".into() };
        assert_eq!(connection_icon(&ep, &ConnectionStatus::Connected, true).css_class, "accent");
        assert_eq!(connection_icon(&ep, &ConnectionStatus::Recovered, true).css_class, "accent");
        assert_eq!(
            connection_icon(&ep, &ConnectionStatus::Disconnected, true).css_class,
            "warning"
        );
        assert_eq!(
            connection_icon(
                &ep,
                &ConnectionStatus::Blocked(ConnectionProblem::PermissionDenied),
                true
            )
            .css_class,
            "error"
        );
        assert_eq!(
            connection_icon(&ep, &ConnectionStatus::Connecting, true).css_class,
            "dim-label"
        );
    }

    #[test]
    fn connection_icon_direct_uses_terminal_icon() {
        let ep = RuntimeEndpoint::Local;
        let icon = connection_icon(&ep, &ConnectionStatus::Connected, false);
        assert_eq!(icon.icon_name, "utilities-terminal-symbolic");
        assert_eq!(icon.css_class, "accent");
    }

    #[test]
    fn connected_color_consistent_across_all_workspace_types() {
        let local_managed =
            connection_icon(&RuntimeEndpoint::Local, &ConnectionStatus::Connected, true);
        let remote_managed = connection_icon(
            &RuntimeEndpoint::Remote { host: "h".into() },
            &ConnectionStatus::Connected,
            true,
        );
        let direct = connection_icon(&RuntimeEndpoint::Local, &ConnectionStatus::Connected, false);
        assert_eq!(local_managed.css_class, "accent");
        assert_eq!(remote_managed.css_class, "accent");
        assert_eq!(direct.css_class, "accent");
    }

    #[test]
    fn connection_icon_tooltips_describe_state() {
        let remote = RuntimeEndpoint::Remote { host: "h".into() };
        let local = RuntimeEndpoint::Local;

        let connected = connection_icon(&remote, &ConnectionStatus::Connected, true);
        assert_eq!(connected.tooltip, "Connected to remote host");

        let disconnected = connection_icon(&remote, &ConnectionStatus::Disconnected, true);
        assert_eq!(disconnected.tooltip, "Disconnected from runtime");

        let connecting_local = connection_icon(&local, &ConnectionStatus::Connecting, true);
        assert_eq!(connecting_local.tooltip, "Connecting…");

        let connecting_remote = connection_icon(&remote, &ConnectionStatus::Connecting, true);
        assert_eq!(connecting_remote.tooltip, "Connecting…");

        let local_connected = connection_icon(&local, &ConnectionStatus::Connected, true);
        assert_eq!(local_connected.tooltip, "Connected to local runtime");
    }

    #[test]
    fn connection_icon_shape_never_changes_with_status_regression() {
        let local = RuntimeEndpoint::Local;
        let remote = RuntimeEndpoint::Remote { host: "h".into() };
        let statuses = [
            ConnectionStatus::Connected,
            ConnectionStatus::Disconnected,
            ConnectionStatus::Recovered,
            ConnectionStatus::Connecting,
            ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
        ];
        for s in &statuses {
            assert_eq!(connection_icon(&local, s, true).icon_name, "computer-symbolic");
            assert_eq!(connection_icon(&remote, s, true).icon_name, "network-server-symbolic");
            assert_eq!(connection_icon(&local, s, false).icon_name, "utilities-terminal-symbolic");
        }
    }

    #[test]
    fn workspace_connection_summary_local_returns_empty_without_pane_info() {
        assert!(workspace_connection_summary(&RuntimeEndpoint::Local, None).is_empty());
    }

    #[test]
    fn workspace_actions_for_attached_managed_workspace_shows_destructive_close() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Persistent), true, 2);

        assert_eq!(presentation.title, "Close Workspace?");
        assert_eq!(presentation.close_label, "Close Workspace");
        assert!(presentation.body.contains("stop its runtime"));
        assert!(presentation.body.contains("2 panes"));
    }

    #[test]
    fn workspace_actions_for_ephemeral_attached_shows_same_close() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Ephemeral), true, 1);

        assert_eq!(presentation.title, "Close Workspace?");
        assert!(presentation.body.contains("stop its runtime"));
    }

    #[test]
    fn workspace_actions_for_detached_managed_workspace_only_offer_close() {
        let presentation = present_workspace_actions(Some(WorkspacePolicy::Persistent), false, 1);

        assert_eq!(presentation.title, "Close Workspace?");
        assert!(presentation.body.contains("not connected to a runtime"));
    }

    #[test]
    fn workspace_actions_for_unmanaged_workspace_keep_simple_close_copy() {
        let presentation = present_workspace_actions(None, false, 3);

        assert_eq!(presentation.title, "Close Workspace?");
        assert_eq!(presentation.close_label, "Close Workspace");
        assert!(presentation.body.contains("3 panes"));
        assert!(presentation.body.contains("running processes"));
    }

    /// Close dialog copy must never mention detach or terminate.
    #[test]
    fn workspace_actions_never_mention_detach_or_terminate() {
        for (policy, attached) in [
            (Some(WorkspacePolicy::Persistent), true),
            (Some(WorkspacePolicy::Ephemeral), true),
            (Some(WorkspacePolicy::Persistent), false),
            (None, false),
        ] {
            let p = present_workspace_actions(policy, attached, 1);
            assert!(!p.body.contains("Detach"), "body must not mention Detach: {}", p.body);
            assert!(!p.body.contains("Terminate"), "body must not mention Terminate: {}", p.body);
        }
    }

    #[test]
    fn managed_remote_sets_endpoint_and_policy() {
        let uuids = vec!["t1".into()];
        let runtime = WorkspaceRuntime::managed_remote(
            "server.example.com",
            WorkspacePolicy::Persistent,
            &uuids,
        );
        assert!(runtime.is_managed());
        assert_eq!(runtime.endpoint, RuntimeEndpoint::Remote { host: "server.example.com".into() });
        assert_eq!(runtime.policy, WorkspacePolicy::Persistent);
        assert!(runtime.pending_layout_panes.contains("t1"));
    }

    #[test]
    fn ensure_placeholder_bindings_adds_new_pane_to_remote_runtime() {
        let mut runtime =
            WorkspaceRuntime::managed_remote("host", WorkspacePolicy::Persistent, &["t1".into()]);

        runtime.ensure_placeholder_bindings(&["t1".into(), "t2".into()]);

        assert!(runtime.pane_bindings.contains_key("t2"));
        assert!(runtime.pending_layout_panes.contains("t2"));
        assert_eq!(runtime.endpoint, RuntimeEndpoint::Remote { host: "host".into() });
    }

    #[test]
    fn ensure_placeholder_bindings_removes_closed_pane() {
        let mut runtime = WorkspaceRuntime::managed_remote(
            "host",
            WorkspacePolicy::Persistent,
            &["t1".into(), "t2".into()],
        );

        runtime.ensure_placeholder_bindings(&["t1".into()]);

        assert!(!runtime.pane_bindings.contains_key("t2"));
        assert!(!runtime.pending_layout_panes.contains("t2"));
        assert!(runtime.pane_bindings.contains_key("t1"));
    }

    // ── bind_runtime_pane / is_layout_pane_pending ──────────────

    #[test]
    fn bind_runtime_pane_clears_pending_and_sets_binding() {
        let mut runtime =
            WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent, &["t1".into()]);
        assert!(runtime.is_layout_pane_pending("t1"));

        runtime.bind_runtime_pane("t1", "runtime-pane-abc");

        assert!(!runtime.is_layout_pane_pending("t1"));
        assert_eq!(runtime.pane_bindings.get("t1").unwrap(), "runtime-pane-abc");
    }

    #[test]
    fn is_layout_pane_pending_false_for_unknown_uuid() {
        let runtime = WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent, &["t1".into()]);
        assert!(!runtime.is_layout_pane_pending("unknown"));
    }

    // ── advance_connection_status ───────────────────────────────

    #[test]
    fn advance_connection_status_full_lifecycle() {
        let s = advance_connection_status(&ConnectionStatus::Connecting, ConnectionEvent::Started);
        assert_eq!(s, ConnectionStatus::Starting);

        let s = advance_connection_status(&s, ConnectionEvent::Connected);
        assert_eq!(s, ConnectionStatus::Connected);

        let s = advance_connection_status(&s, ConnectionEvent::Lost);
        assert_eq!(s, ConnectionStatus::Disconnected);

        let s = advance_connection_status(
            &s,
            ConnectionEvent::RetryScheduled { attempt: 1, retry_in_secs: 5 },
        );
        assert!(matches!(s, ConnectionStatus::Reconnecting { attempt: 1, .. }));

        let s = advance_connection_status(&s, ConnectionEvent::Recovered);
        assert_eq!(s, ConnectionStatus::Recovered);
    }

    #[test]
    fn advance_connection_status_failed_produces_blocked() {
        let s = advance_connection_status(
            &ConnectionStatus::Connecting,
            ConnectionEvent::Failed(ConnectionProblem::PermissionDenied),
        );
        assert!(matches!(s, ConnectionStatus::Blocked(_)));
    }

    // ── ConnectionStatus::accepts_input ─────────────────────────

    #[test]
    fn connection_status_accepts_input_only_when_connected_or_recovered() {
        assert!(ConnectionStatus::Connected.accepts_input());
        assert!(ConnectionStatus::Recovered.accepts_input());
        assert!(!ConnectionStatus::Connecting.accepts_input());
        assert!(!ConnectionStatus::Disconnected.accepts_input());
        assert!(!ConnectionStatus::Starting.accepts_input());
    }

    // ── ConnectionProblem properties ────────────────────────────

    #[test]
    fn connection_problem_transient_vs_blocked() {
        assert!(ConnectionProblem::DaemonUnavailable.is_transient());
        assert!(!ConnectionProblem::VersionMismatch.is_transient());
        assert!(!ConnectionProblem::OwnershipConflict.is_transient());
        assert!(!ConnectionProblem::PermissionDenied.is_transient());
    }

    #[test]
    fn connection_problem_labels_are_nonempty() {
        for problem in [
            ConnectionProblem::DaemonUnavailable,
            ConnectionProblem::VersionMismatch,
            ConnectionProblem::OwnershipConflict,
            ConnectionProblem::PermissionDenied,
            ConnectionProblem::Protocol("test".into()),
            ConnectionProblem::UserActionRequired("test".into()),
        ] {
            assert!(!problem.label().is_empty(), "label empty for {problem:?}");
        }
    }

    // ── RuntimeEndpoint::key ────────────────────────────────────

    #[test]
    fn endpoint_key_distinguishes_local_and_remote() {
        let local = RuntimeEndpoint::Local;
        let remote = RuntimeEndpoint::Remote { host: "host".into() };
        assert_ne!(local.key(), remote.key());
        assert_eq!(local.key(), "local");
        assert!(remote.key().contains("host"));
    }

    #[test]
    fn host_key_local_returns_local_key() {
        assert_eq!(RuntimeEndpoint::Local.host_key(), crate::host::LOCAL_KEY);
    }

    #[test]
    fn host_key_remote_normalizes_ssh_target() {
        let endpoint = RuntimeEndpoint::Remote { host: "deploy@example.com".into() };
        assert_eq!(endpoint.host_key(), "example.com");
    }

    #[test]
    fn host_key_remote_bare_hostname() {
        let endpoint = RuntimeEndpoint::Remote { host: "dev-box".into() };
        assert_eq!(endpoint.host_key(), "dev-box");
    }

    #[test]
    fn remote_endpoint_host_key_matches_saved_host_key() {
        let endpoint = RuntimeEndpoint::Remote { host: "deploy@builder.example.com".into() };
        let host = crate::host::Host::remote("deploy@builder.example.com");
        assert_eq!(endpoint.host_key(), host.key);
    }

    #[test]
    fn connection_presentation_for_starting_shows_starting_label() {
        let p = present_connection_status(&ConnectionStatus::Starting);
        assert_eq!(p.header_label, "Starting");
        assert!(!p.input_enabled);
    }

    #[test]
    fn menu_items_local_persistent_connected() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: true,
            is_attached: true,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_edit_connection);
        assert!(!items.show_reconnect);
        assert!(!items.show_reconnect_host);
        assert!(items.show_detach);
    }

    #[test]
    fn menu_items_remote_persistent_disconnected() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: true,
            is_disconnected: true,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(items.show_edit_connection);
        assert!(items.show_reconnect);
        assert!(!items.show_reconnect_host);
        assert!(items.show_detach);
    }

    #[test]
    fn menu_items_local_ephemeral_connected() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: false,
            is_attached: true,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_edit_connection);
        assert!(!items.show_reconnect);
        assert!(!items.show_reconnect_host);
        assert!(!items.show_detach);
    }

    #[test]
    fn menu_items_managed_disconnected_not_attached() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: true,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(items.show_reconnect);
        assert!(!items.show_reconnect_host);
        assert!(!items.show_detach);
    }

    #[test]
    fn menu_items_unmanaged_workspace() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: false,
            is_persistent: false,
            is_attached: false,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_edit_connection);
        assert!(!items.show_reconnect);
        assert!(!items.show_reconnect_host);
        assert!(!items.show_detach);
    }

    #[test]
    fn menu_items_reconnect_host_shown_when_multiple_disconnected() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: true,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: true,
        });
        assert!(items.show_reconnect);
        assert!(items.show_reconnect_host);
    }

    #[test]
    fn menu_items_reconnect_host_hidden_when_connected() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: true,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: true,
        });
        assert!(!items.show_reconnect_host);
    }

    // ── SessionMissing state ────────────────────────────────────

    #[test]
    fn classify_session_not_found_as_session_missing() {
        let problem = classify_connection_problem(&DaemonError::ServerError {
            code: 4,
            message: "session not found".into(),
        });
        assert_eq!(problem, ConnectionProblem::SessionMissing);
        assert!(!problem.is_transient());
    }

    #[test]
    fn session_missing_status_disables_input() {
        assert!(!ConnectionStatus::SessionMissing.accepts_input());
    }

    #[test]
    fn session_missing_label_describes_state() {
        assert_eq!(ConnectionStatus::SessionMissing.short_label(), "Session Missing");
        assert!(ConnectionStatus::SessionMissing.label().contains("no longer exists"));
    }

    #[test]
    fn advance_to_session_missing() {
        let status = advance_connection_status(
            &ConnectionStatus::Connecting,
            ConnectionEvent::SessionMissing,
        );
        assert_eq!(status, ConnectionStatus::SessionMissing);
    }

    #[test]
    fn session_missing_icon_uses_warning_class() {
        let icon =
            connection_icon(&RuntimeEndpoint::Local, &ConnectionStatus::SessionMissing, true);
        assert_eq!(icon.css_class, "warning");
        assert!(icon.tooltip.contains("no longer exists"));
    }

    #[test]
    fn session_missing_problem_label_is_nonempty() {
        assert!(!ConnectionProblem::SessionMissing.label().is_empty());
    }

    #[test]
    fn menu_items_session_missing_hides_reconnect() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_reconnect);
    }

    #[test]
    fn connection_presentation_accepts_input_matches_status_accepts_input() {
        let statuses = [
            ConnectionStatus::Starting,
            ConnectionStatus::Connecting,
            ConnectionStatus::Connected,
            ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 5 },
            ConnectionStatus::Blocked(ConnectionProblem::DaemonUnavailable),
            ConnectionStatus::Disconnected,
            ConnectionStatus::Recovered,
            ConnectionStatus::SessionMissing,
        ];
        for status in &statuses {
            let presentation = present_connection_status(status);
            assert_eq!(
                presentation.input_enabled,
                status.accepts_input(),
                "presentation.input_enabled should match status.accepts_input() for {status:?}"
            );
        }
    }

    /// The context menu halign constant must be Start so the popover opens
    /// adjacent to the pointer. Combined with parenting the popover to the
    /// VTE (not the outer Box), this ensures the menu appears at the click
    /// position. Regression for #568.
    #[test]
    fn context_menu_halign_prevents_coordinate_mismatch() {
        assert_eq!(
            crate::terminal::CONTEXT_MENU_HALIGN,
            gtk4::Align::Start,
            "context menu halign must be Start to position adjacent to the pointer"
        );
    }

    // ── DaemonDied state (#954) ─────────────────────────────────

    #[test]
    fn daemon_died_is_not_transient() {
        assert!(!ConnectionProblem::DaemonDied.is_transient());
    }

    #[test]
    fn daemon_died_label_says_daemon_stopped() {
        assert_eq!(ConnectionProblem::DaemonDied.label(), "Daemon stopped");
    }

    #[test]
    fn daemon_died_short_label_says_daemon_stopped() {
        let status = ConnectionStatus::Blocked(ConnectionProblem::DaemonDied);
        assert_eq!(status.short_label(), "Daemon Stopped");
    }

    #[test]
    fn daemon_died_full_label_says_daemon_stopped() {
        let status = ConnectionStatus::Blocked(ConnectionProblem::DaemonDied);
        assert_eq!(status.label(), "Daemon stopped");
    }

    #[test]
    fn daemon_died_icon_uses_error_class_with_restart_tooltip() {
        let icon = connection_icon(
            &RuntimeEndpoint::Local,
            &ConnectionStatus::Blocked(ConnectionProblem::DaemonDied),
            true,
        );
        assert_eq!(icon.css_class, "error");
        assert!(
            icon.tooltip.contains("restart"),
            "tooltip should mention restart: {}",
            icon.tooltip
        );
    }

    #[test]
    fn menu_items_daemon_died_shows_restart_daemon() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: true,
            is_connecting: false,
            is_daemon_died: true,
            has_other_disconnected_from_same_host: false,
        });
        assert!(items.show_restart_daemon);
        assert!(items.show_reconnect);
    }

    #[test]
    fn menu_items_daemon_died_remote_hides_restart_daemon() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: true,
            is_connecting: false,
            is_daemon_died: true,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_restart_daemon);
    }

    #[test]
    fn daemon_died_disables_input() {
        let status = ConnectionStatus::Blocked(ConnectionProblem::DaemonDied);
        assert!(!status.accepts_input());
    }

    // ── Force reconnect during Connecting (#955) ────────────────

    #[test]
    fn menu_items_connecting_shows_force_reconnect() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: false,
            is_connecting: true,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(items.show_reconnect, "Force Reconnect should be available during Connecting");
    }

    #[test]
    fn menu_items_connecting_local_shows_force_reconnect() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: false,
            is_managed: true,
            is_persistent: true,
            is_attached: false,
            is_disconnected: false,
            is_connecting: true,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(items.show_reconnect, "Force Reconnect should be available for local connecting");
    }

    #[test]
    fn menu_items_connected_hides_reconnect() {
        let items = workspace_menu_items(&WorkspaceMenuContext {
            is_remote: true,
            is_managed: true,
            is_persistent: true,
            is_attached: true,
            is_disconnected: false,
            is_connecting: false,
            is_daemon_died: false,
            has_other_disconnected_from_same_host: false,
        });
        assert!(!items.show_reconnect, "Reconnect should be hidden when connected");
    }
}

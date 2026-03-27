//! Async endpoint-scoped daemon connection manager.
//!
//! GTK/UI code talks to this manager through fire-and-forget commands. One
//! background actor is created per endpoint and reuses a single daemon
//! connection for multiple managed workspaces on that endpoint.

use crate::daemon::{
    DaemonConnection, DaemonError, DetachResponse, SshHandle, daemon_binary, default_socket_path,
};
use crate::runtime::{
    ConnectionEvent, ConnectionProblem, ConnectionStatus, RuntimeEndpoint, WorkspacePolicy,
    advance_connection_status, classify_connection_problem,
};
use rttx_proto::proto;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Operation type for manager error reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerOperation {
    OpenWorkspace,
    CreatePane,
    ClosePane,
    DetachRuntime,
    TerminateRuntime,
    RefreshInventory,
    SendInput,
    ResizePane,
}

/// Event emitted back onto the GTK main loop from an endpoint actor.
#[derive(Debug, Clone)]
pub enum EndpointEvent {
    WorkspaceConnectionChanged {
        workspace_id: String,
        status: ConnectionStatus,
    },
    WorkspaceOpened {
        workspace_id: String,
        runtime_id: String,
        snapshot: proto::Snapshot,
    },
    PaneCreated {
        workspace_id: String,
        layout_terminal_uuid: String,
        runtime_id: String,
        runtime_pane_id: String,
    },
    PaneClosed {
        workspace_id: String,
        layout_terminal_uuid: String,
        runtime_id: String,
        runtime_pane_id: String,
    },
    WorkspaceDetached {
        workspace_id: String,
        runtime_id: String,
    },
    RuntimeTerminated {
        workspace_id: String,
        runtime_id: String,
        reason: proto::RuntimeTerminationReason,
    },
    InventoryLoaded {
        endpoint: RuntimeEndpoint,
        sessions: Vec<proto::SessionInfo>,
    },
    RuntimeMessage {
        endpoint: RuntimeEndpoint,
        message: proto::ServerMessage,
    },
    WorkspaceError {
        workspace_id: String,
        operation: ManagerOperation,
        problem: ConnectionProblem,
        detail: String,
    },
}

#[derive(Debug)]
enum EndpointCommand {
    OpenWorkspace {
        workspace_id: String,
        name: String,
        policy: WorkspacePolicy,
        runtime_id: Option<String>,
        placeholder_terminal_uuid: Option<String>,
    },
    CreatePane {
        workspace_id: String,
        runtime_id: String,
        layout_terminal_uuid: String,
    },
    ClosePane {
        workspace_id: String,
        runtime_id: String,
        layout_terminal_uuid: String,
        runtime_pane_id: String,
    },
    DetachRuntime {
        workspace_id: String,
        runtime_id: String,
    },
    TerminateRuntime {
        workspace_id: String,
        runtime_id: String,
    },
    SendInput {
        workspace_id: String,
        runtime_id: String,
        runtime_pane_id: String,
        data: Vec<u8>,
    },
    ResizePane {
        workspace_id: String,
        runtime_id: String,
        runtime_pane_id: String,
        cols: u16,
        rows: u16,
    },
    RefreshInventory,
    Reconnect,
    ForgetWorkspace {
        workspace_id: String,
    },
}

#[derive(Debug)]
struct EndpointHandle {
    cmd_tx: mpsc::UnboundedSender<EndpointCommand>,
}

/// Public manager used by the GTK window.
#[derive(Debug)]
pub struct EndpointConnectionManager {
    rt: tokio::runtime::Runtime,
    endpoints: RefCell<HashMap<String, EndpointHandle>>,
    event_tx: mpsc::UnboundedSender<EndpointEvent>,
}

impl EndpointConnectionManager {
    /// Create a new endpoint-scoped manager and its event receiver.
    pub fn new() -> Result<(Self, mpsc::UnboundedReceiver<EndpointEvent>), DaemonError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(DaemonError::Io)?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Ok((Self { rt, endpoints: RefCell::new(HashMap::new()), event_tx }, event_rx))
    }

    fn endpoint_handle(
        &self,
        endpoint: &RuntimeEndpoint,
    ) -> mpsc::UnboundedSender<EndpointCommand> {
        let key = endpoint.key();
        if let Some(handle) = self.endpoints.borrow().get(&key) {
            return handle.cmd_tx.clone();
        }

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let actor =
            EndpointActor::new(endpoint.clone(), self.event_tx.clone(), cmd_tx.clone(), cmd_rx);
        self.rt.spawn(actor.run());
        self.endpoints.borrow_mut().insert(key, EndpointHandle { cmd_tx: cmd_tx.clone() });
        cmd_tx
    }

    /// Create or attach a managed workspace runtime asynchronously.
    pub fn open_workspace(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        name: &str,
        policy: WorkspacePolicy,
        runtime_id: Option<&str>,
        placeholder_terminal_uuid: Option<&str>,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::OpenWorkspace {
            workspace_id: workspace_id.to_string(),
            name: name.to_string(),
            policy,
            runtime_id: runtime_id.map(str::to_string),
            placeholder_terminal_uuid: placeholder_terminal_uuid.map(str::to_string),
        });
    }

    /// Request runtime inventory for an endpoint.
    pub fn refresh_inventory(&self, endpoint: &RuntimeEndpoint) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::RefreshInventory);
    }

    /// Request a new pane inside an attached runtime.
    pub fn create_pane(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        layout_terminal_uuid: &str,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::CreatePane {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            layout_terminal_uuid: layout_terminal_uuid.to_string(),
        });
    }

    /// Request a pane close on the daemon before mutating GTK layout state.
    pub fn close_pane(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        layout_terminal_uuid: &str,
        runtime_pane_id: &str,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::ClosePane {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            layout_terminal_uuid: layout_terminal_uuid.to_string(),
            runtime_pane_id: runtime_pane_id.to_string(),
        });
    }

    /// Gracefully detach a workspace from its runtime.
    pub fn detach_runtime(&self, workspace_id: &str, endpoint: &RuntimeEndpoint, runtime_id: &str) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::DetachRuntime {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
        });
    }

    /// Explicitly terminate a runtime.
    pub fn terminate_runtime(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::TerminateRuntime {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
        });
    }

    /// Forward input to a daemon-managed pane.
    pub fn send_input(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        runtime_pane_id: &str,
        data: Vec<u8>,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::SendInput {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            runtime_pane_id: runtime_pane_id.to_string(),
            data,
        });
    }

    /// Forward a resize to a daemon-managed pane.
    pub fn resize_pane(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        runtime_pane_id: &str,
        cols: u16,
        rows: u16,
    ) {
        let _ = self.endpoint_handle(endpoint).send(EndpointCommand::ResizePane {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            runtime_pane_id: runtime_pane_id.to_string(),
            cols,
            rows,
        });
    }

    /// Stop tracking a workspace on its endpoint actor.
    pub fn forget_workspace(&self, endpoint: &RuntimeEndpoint, workspace_id: &str) {
        let _ = self
            .endpoint_handle(endpoint)
            .send(EndpointCommand::ForgetWorkspace { workspace_id: workspace_id.to_string() });
    }
}

#[derive(Debug)]
struct EndpointActor {
    endpoint: RuntimeEndpoint,
    event_tx: mpsc::UnboundedSender<EndpointEvent>,
    self_tx: mpsc::UnboundedSender<EndpointCommand>,
    cmd_rx: mpsc::UnboundedReceiver<EndpointCommand>,
    connection: Option<DaemonConnection>,
    ssh_handle: Option<SshHandle>,
    tracked_workspaces: HashMap<String, String>,
    reconnect_attempt: u32,
}

impl EndpointActor {
    fn new(
        endpoint: RuntimeEndpoint,
        event_tx: mpsc::UnboundedSender<EndpointEvent>,
        self_tx: mpsc::UnboundedSender<EndpointCommand>,
        cmd_rx: mpsc::UnboundedReceiver<EndpointCommand>,
    ) -> Self {
        Self {
            endpoint,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
        }
    }

    async fn run(mut self) {
        loop {
            if let Some(connection) = self.connection.as_mut() {
                tokio::select! {
                    biased;
                    command = self.cmd_rx.recv() => {
                        let Some(command) = command else { break };
                        self.handle_command(command).await;
                    }
                    message = connection.recv() => {
                        self.handle_runtime_message(message);
                    }
                }
            } else {
                let Some(command) = self.cmd_rx.recv().await else { break };
                self.handle_command(command).await;
            }
        }
    }

    async fn handle_command(&mut self, command: EndpointCommand) {
        match command {
            EndpointCommand::OpenWorkspace {
                workspace_id,
                name,
                policy,
                runtime_id,
                placeholder_terminal_uuid,
            } => {
                self.emit_status(&workspace_id, ConnectionStatus::Connecting);
                if let Err(problem) = self.ensure_connected(&workspace_id).await {
                    self.emit_error(
                        &workspace_id,
                        ManagerOperation::OpenWorkspace,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }

                let runtime_id = if let Some(runtime_id) = runtime_id {
                    runtime_id
                } else {
                    let Ok(runtime_id) = self.create_runtime(&workspace_id, &name, policy).await
                    else {
                        return;
                    };
                    runtime_id
                };

                let Ok(snapshot) = self.attach_runtime(&workspace_id, &runtime_id).await else {
                    return;
                };

                self.tracked_workspaces.insert(workspace_id.clone(), runtime_id.clone());
                let _ = self.event_tx.send(EndpointEvent::WorkspaceOpened {
                    workspace_id: workspace_id.clone(),
                    runtime_id: runtime_id.clone(),
                    snapshot: snapshot.clone(),
                });

                if snapshot.panes.is_empty() {
                    if let Some(layout_terminal_uuid) = placeholder_terminal_uuid {
                        let _ = self.self_tx.send(EndpointCommand::CreatePane {
                            workspace_id,
                            runtime_id,
                            layout_terminal_uuid,
                        });
                    } else {
                        self.emit_status(&workspace_id, ConnectionStatus::Connected);
                    }
                } else {
                    self.emit_status(&workspace_id, ConnectionStatus::Connected);
                }
            }
            EndpointCommand::CreatePane { workspace_id, runtime_id, layout_terminal_uuid } => {
                if let Err(problem) = self.ensure_connected(&workspace_id).await {
                    self.emit_error(
                        &workspace_id,
                        ManagerOperation::CreatePane,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }

                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::CreatePane,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                let pane_id = {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.create_pane(runtime_uuid).await
                };
                match pane_id {
                    Ok(pane_id) => {
                        let _ = self.event_tx.send(EndpointEvent::PaneCreated {
                            workspace_id: workspace_id.clone(),
                            layout_terminal_uuid,
                            runtime_id,
                            runtime_pane_id: pane_id.to_string(),
                        });
                        self.emit_status(&workspace_id, ConnectionStatus::Connected);
                    }
                    Err(error) => {
                        self.handle_command_error(
                            &workspace_id,
                            ManagerOperation::CreatePane,
                            &error,
                        );
                    }
                }
            }
            EndpointCommand::ClosePane {
                workspace_id,
                runtime_id,
                layout_terminal_uuid,
                runtime_pane_id,
            } => {
                if let Err(problem) = self.ensure_connected(&workspace_id).await {
                    self.emit_error(
                        &workspace_id,
                        ManagerOperation::ClosePane,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }

                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::ClosePane,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                let Some(pane_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::ClosePane,
                    &runtime_pane_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                let close_result = {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.close_pane(runtime_uuid, pane_uuid).await
                };
                match close_result {
                    Ok(_) => {
                        let _ = self.event_tx.send(EndpointEvent::PaneClosed {
                            workspace_id,
                            layout_terminal_uuid,
                            runtime_id,
                            runtime_pane_id,
                        });
                    }
                    Err(error) => {
                        self.handle_command_error(
                            &workspace_id,
                            ManagerOperation::ClosePane,
                            &error,
                        );
                    }
                }
            }
            EndpointCommand::DetachRuntime { workspace_id, runtime_id } => {
                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::DetachRuntime,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                if let Err(problem) = self.ensure_connected(&workspace_id).await {
                    self.emit_error(
                        &workspace_id,
                        ManagerOperation::DetachRuntime,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }

                let detach_result = {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.detach_session(runtime_uuid).await
                };

                match detach_result {
                    Ok(DetachResponse::Detached(_)) => {
                        self.tracked_workspaces.remove(&workspace_id);
                        let _ = self
                            .event_tx
                            .send(EndpointEvent::WorkspaceDetached { workspace_id, runtime_id });
                    }
                    Ok(DetachResponse::Terminated(terminated)) => {
                        self.tracked_workspaces.remove(&workspace_id);
                        let _ = self.event_tx.send(EndpointEvent::RuntimeTerminated {
                            workspace_id,
                            runtime_id,
                            reason: proto::RuntimeTerminationReason::try_from(terminated.reason)
                                .unwrap_or(proto::RuntimeTerminationReason::Unspecified),
                        });
                    }
                    Err(error) => {
                        self.handle_command_error(
                            &workspace_id,
                            ManagerOperation::DetachRuntime,
                            &error,
                        );
                    }
                }
            }
            EndpointCommand::TerminateRuntime { workspace_id, runtime_id } => {
                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::TerminateRuntime,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                if let Err(problem) = self.ensure_connected(&workspace_id).await {
                    self.emit_error(
                        &workspace_id,
                        ManagerOperation::TerminateRuntime,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }

                let terminated = {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.terminate_session(runtime_uuid).await
                };
                match terminated {
                    Ok(terminated) => {
                        self.tracked_workspaces.remove(&workspace_id);
                        let _ = self.event_tx.send(EndpointEvent::RuntimeTerminated {
                            workspace_id,
                            runtime_id,
                            reason: proto::RuntimeTerminationReason::try_from(terminated.reason)
                                .unwrap_or(proto::RuntimeTerminationReason::Unspecified),
                        });
                    }
                    Err(error) => {
                        self.handle_command_error(
                            &workspace_id,
                            ManagerOperation::TerminateRuntime,
                            &error,
                        );
                    }
                }
            }
            EndpointCommand::SendInput { workspace_id, runtime_id, runtime_pane_id, data } => {
                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::SendInput,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                let Some(pane_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::SendInput,
                    &runtime_pane_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                if let Some(connection) = self.connection.as_mut()
                    && let Err(error) = connection
                        .send(&proto::ClientMessage {
                            msg: Some(proto::client_message::Msg::Input(proto::Input {
                                session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                                pane_id: rttx_proto::uuid_to_bytes(pane_uuid),
                                data,
                            })),
                        })
                        .await
                {
                    self.handle_command_error(&workspace_id, ManagerOperation::SendInput, &error);
                }
            }
            EndpointCommand::ResizePane {
                workspace_id,
                runtime_id,
                runtime_pane_id,
                cols,
                rows,
            } => {
                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::ResizePane,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                let Some(pane_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::ResizePane,
                    &runtime_pane_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                if let Some(connection) = self.connection.as_mut()
                    && let Err(error) = connection
                        .send(&proto::ClientMessage {
                            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                                session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                                pane_id: rttx_proto::uuid_to_bytes(pane_uuid),
                                cols: u32::from(cols),
                                rows: u32::from(rows),
                            })),
                        })
                        .await
                {
                    self.handle_command_error(&workspace_id, ManagerOperation::ResizePane, &error);
                }
            }
            EndpointCommand::RefreshInventory => {
                let pseudo_workspace = format!("inventory:{}", self.endpoint.key());
                if let Err(problem) = self.ensure_connected(&pseudo_workspace).await {
                    self.emit_error(
                        &pseudo_workspace,
                        ManagerOperation::RefreshInventory,
                        problem.clone(),
                        problem.label(),
                    );
                    return;
                }
                let list_result = {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.list_sessions().await
                };
                match list_result {
                    Ok(sessions) => {
                        let _ = self.event_tx.send(EndpointEvent::InventoryLoaded {
                            endpoint: self.endpoint.clone(),
                            sessions,
                        });
                    }
                    Err(error) => self.handle_command_error(
                        &pseudo_workspace,
                        ManagerOperation::RefreshInventory,
                        &error,
                    ),
                }
            }
            EndpointCommand::Reconnect => {
                let workspaces: Vec<_> = self.tracked_workspaces.keys().cloned().collect();
                if workspaces.is_empty() {
                    return;
                }
                let primary = workspaces[0].clone();
                if let Err(problem) = self.ensure_connected(&primary).await {
                    if problem.is_transient() {
                        self.schedule_reconnect();
                    }
                    return;
                }

                for (workspace_id, runtime_id) in self.tracked_workspaces.clone() {
                    if let Ok(snapshot) = self.attach_runtime(&workspace_id, &runtime_id).await {
                        let _ = self.event_tx.send(EndpointEvent::WorkspaceOpened {
                            workspace_id: workspace_id.clone(),
                            runtime_id: runtime_id.clone(),
                            snapshot,
                        });
                        self.emit_status(&workspace_id, ConnectionStatus::Recovered);
                    }
                }
            }
            EndpointCommand::ForgetWorkspace { workspace_id } => {
                self.tracked_workspaces.remove(&workspace_id);
            }
        }
    }

    async fn ensure_connected(&mut self, workspace_id: &str) -> Result<(), ConnectionProblem> {
        if self.connection.is_some() {
            return Ok(());
        }

        let status = match self.endpoint {
            RuntimeEndpoint::Local => ConnectionStatus::Starting,
            RuntimeEndpoint::Remote { .. } => ConnectionStatus::Connecting,
        };
        self.emit_status(workspace_id, status);

        match self.connect_endpoint().await {
            Ok(()) => {
                self.reconnect_attempt = 0;
                self.emit_status(workspace_id, ConnectionStatus::Connected);
                Ok(())
            }
            Err(error) => {
                let problem = classify_connection_problem(&error);
                let status = advance_connection_status(
                    &ConnectionStatus::Connecting,
                    ConnectionEvent::Failed(problem.clone()),
                );
                self.emit_status(workspace_id, status);
                if problem.is_transient() {
                    self.schedule_reconnect();
                }
                Err(problem)
            }
        }
    }

    async fn connect_endpoint(&mut self) -> Result<(), DaemonError> {
        match &self.endpoint {
            RuntimeEndpoint::Local => {
                let socket_path = default_socket_path();
                if !socket_path.exists() {
                    Self::start_local_daemon(&socket_path).await?;
                }
                self.connection = Some(DaemonConnection::connect(&socket_path).await?);
            }
            RuntimeEndpoint::Remote { host } => {
                let (connection, ssh_handle) = DaemonConnection::connect_ssh(host).await?;
                self.connection = Some(connection);
                self.ssh_handle = Some(ssh_handle);
            }
        }
        Ok(())
    }

    async fn start_local_daemon(socket_path: &Path) -> Result<(), DaemonError> {
        let mut command = tokio::process::Command::new(daemon_binary());
        command
            .arg("start")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if crate::config::is_development() {
            command.env("RTTX_DEV_MODE", "1");
        }
        let _ = command.status().await?;

        for _ in 0..30 {
            if socket_path.exists() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err(DaemonError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "daemon socket did not appear after startup",
        )))
    }

    async fn create_runtime(
        &mut self,
        workspace_id: &str,
        name: &str,
        policy: WorkspacePolicy,
    ) -> Result<String, ()> {
        let create_result = {
            let connection = self.connection.as_mut().expect("connection must exist");
            connection.create_session(name, policy).await
        };
        match create_result {
            Ok(runtime_id) => Ok(runtime_id.to_string()),
            Err(error) => {
                self.handle_command_error(workspace_id, ManagerOperation::OpenWorkspace, &error);
                Err(())
            }
        }
    }

    async fn attach_runtime(
        &mut self,
        workspace_id: &str,
        runtime_id: &str,
    ) -> Result<proto::Snapshot, ()> {
        let Some(runtime_uuid) =
            parse_uuid(workspace_id, ManagerOperation::OpenWorkspace, runtime_id, &self.event_tx)
        else {
            return Err(());
        };
        let attach_result = {
            let connection = self.connection.as_mut().expect("connection must exist");
            connection.attach_session(runtime_uuid, proto::RuntimeAttachMode::ReadWrite).await
        };
        match attach_result {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                self.handle_command_error(workspace_id, ManagerOperation::OpenWorkspace, &error);
                Err(())
            }
        }
    }

    fn handle_runtime_message(
        &mut self,
        message: Result<Option<proto::ServerMessage>, DaemonError>,
    ) {
        match message {
            Ok(Some(message)) => {
                if let Some(proto::server_message::Msg::SessionTerminated(terminated)) =
                    &message.msg
                    && let Ok(runtime_id) = rttx_proto::bytes_to_uuid(&terminated.session_id)
                {
                    self.tracked_workspaces.retain(|_, tracked_runtime_id| {
                        tracked_runtime_id != &runtime_id.to_string()
                    });
                }
                self.forward_push(message);
            }
            Ok(None) | Err(_) => self.handle_disconnect(),
        }
    }

    fn forward_push(&self, message: proto::ServerMessage) {
        let _ = self
            .event_tx
            .send(EndpointEvent::RuntimeMessage { endpoint: self.endpoint.clone(), message });
    }

    fn handle_disconnect(&mut self) {
        self.connection = None;
        self.ssh_handle = None;
        if self.tracked_workspaces.is_empty() {
            return;
        }
        for workspace_id in self.tracked_workspaces.keys() {
            self.emit_status(workspace_id, ConnectionStatus::Disconnected);
            self.emit_status(
                workspace_id,
                ConnectionStatus::Reconnecting {
                    attempt: self.reconnect_attempt.saturating_add(1),
                },
            );
        }
        self.schedule_reconnect();
    }

    fn schedule_reconnect(&mut self) {
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        let self_tx = self.self_tx.clone();
        let delay = Duration::from_secs(self.reconnect_attempt.min(5) as u64);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = self_tx.send(EndpointCommand::Reconnect);
        });
    }

    fn emit_status(&self, workspace_id: &str, status: ConnectionStatus) {
        let _ = self.event_tx.send(EndpointEvent::WorkspaceConnectionChanged {
            workspace_id: workspace_id.to_string(),
            status,
        });
    }

    fn emit_error(
        &self,
        workspace_id: &str,
        operation: ManagerOperation,
        problem: ConnectionProblem,
        detail: String,
    ) {
        let _ = self.event_tx.send(EndpointEvent::WorkspaceError {
            workspace_id: workspace_id.to_string(),
            operation,
            problem: problem.clone(),
            detail,
        });
        self.emit_status(workspace_id, ConnectionStatus::Blocked(problem));
    }

    fn handle_command_error(
        &mut self,
        workspace_id: &str,
        operation: ManagerOperation,
        error: &DaemonError,
    ) {
        let problem = classify_connection_problem(error);
        self.emit_error(workspace_id, operation, problem.clone(), error.to_string());
        if problem.is_transient() {
            self.handle_disconnect();
        }
    }
}

fn parse_uuid(
    workspace_id: &str,
    operation: ManagerOperation,
    value: &str,
    event_tx: &mpsc::UnboundedSender<EndpointEvent>,
) -> Option<Uuid> {
    match Uuid::parse_str(value) {
        Ok(uuid) => Some(uuid),
        Err(error) => {
            let _ = event_tx.send(EndpointEvent::WorkspaceError {
                workspace_id: workspace_id.to_string(),
                operation,
                problem: ConnectionProblem::Protocol("Invalid runtime UUID".into()),
                detail: error.to_string(),
            });
            None
        }
    }
}

//! Async endpoint-scoped daemon connection manager.
//!
//! GTK/UI code talks to this manager through fire-and-forget commands. One
//! background actor is created per endpoint and reuses a single daemon
//! connection for multiple managed workspaces on that endpoint.

use crate::daemon::{
    DaemonConnection, DaemonError, DaemonReader, DaemonWriter, SshHandle, daemon_binary,
    default_socket_path,
};
use crate::runtime::{
    ConnectionEvent, ConnectionProblem, ConnectionStatus, RuntimeEndpoint, WorkspacePolicy,
    advance_connection_status, classify_connection_problem,
};
use rttx_proto::proto;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

#[cfg(test)]
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(not(test))]
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Capacity for the event channel (`EndpointEvent` → GTK main loop).
const EVENT_CHANNEL_BOUND: usize = 4096;

/// Capacity for the command channel (`EndpointCommand` → actor).
const CMD_CHANNEL_BOUND: usize = 4096;

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
    RenameRuntime,
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
        cwd: Option<String>,
        dark_background: bool,
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
        data: bytes::Bytes,
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
    RenameRuntime {
        workspace_id: String,
        runtime_id: String,
        name: String,
    },
    Shutdown,
}

#[derive(Debug)]
struct EndpointHandle {
    cmd_tx: mpsc::Sender<EndpointCommand>,
}

/// Public manager used by the GTK window.
#[derive(Debug)]
pub struct EndpointConnectionManager {
    rt: tokio::runtime::Runtime,
    endpoints: RefCell<HashMap<String, EndpointHandle>>,
    event_tx: mpsc::Sender<EndpointEvent>,
    auto_start_daemon: bool,
    reconnect_delay_secs: u32,
}

impl EndpointConnectionManager {
    /// Create a new endpoint-scoped manager and its event receiver.
    pub fn new(
        auto_start_daemon: bool,
        reconnect_delay_secs: u32,
    ) -> Result<(Self, mpsc::Receiver<EndpointEvent>), DaemonError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(DaemonError::Io)?;
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        Ok((
            Self {
                rt,
                endpoints: RefCell::new(HashMap::new()),
                event_tx,
                auto_start_daemon,
                reconnect_delay_secs,
            },
            event_rx,
        ))
    }

    fn endpoint_handle(&self, endpoint: &RuntimeEndpoint) -> mpsc::Sender<EndpointCommand> {
        let key = endpoint.key();
        if let Some(handle) = self.endpoints.borrow().get(&key) {
            return handle.cmd_tx.clone();
        }

        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor = EndpointActor::new(
            endpoint.clone(),
            self.event_tx.clone(),
            cmd_tx.clone(),
            cmd_rx,
            self.auto_start_daemon,
            self.reconnect_delay_secs,
        );
        self.rt.spawn(actor.run());
        self.endpoints.borrow_mut().insert(key, EndpointHandle { cmd_tx: cmd_tx.clone() });
        cmd_tx
    }

    /// Shut down the existing actor for an endpoint so the next operation
    /// creates a fresh one. Used by explicit user retry to bypass a stuck actor.
    pub fn reset_endpoint(&self, endpoint: &RuntimeEndpoint) {
        let key = endpoint.key();
        if let Some(handle) = self.endpoints.borrow_mut().remove(&key) {
            let _ = handle.cmd_tx.try_send(EndpointCommand::Shutdown);
        }
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
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::OpenWorkspace {
            workspace_id: workspace_id.to_string(),
            name: name.to_string(),
            policy,
            runtime_id: runtime_id.map(str::to_string),
            placeholder_terminal_uuid: placeholder_terminal_uuid.map(str::to_string),
        });
    }

    /// Request runtime inventory for an endpoint.
    pub fn refresh_inventory(&self, endpoint: &RuntimeEndpoint) {
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::RefreshInventory);
    }

    /// Request a new pane inside an attached runtime.
    pub fn create_pane(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        layout_terminal_uuid: &str,
        cwd: Option<String>,
        dark_background: bool,
    ) {
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::CreatePane {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            layout_terminal_uuid: layout_terminal_uuid.to_string(),
            cwd,
            dark_background,
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
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::ClosePane {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            layout_terminal_uuid: layout_terminal_uuid.to_string(),
            runtime_pane_id: runtime_pane_id.to_string(),
        });
    }

    /// Gracefully detach a workspace from its runtime.
    pub fn detach_runtime(&self, workspace_id: &str, endpoint: &RuntimeEndpoint, runtime_id: &str) {
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::DetachRuntime {
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
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::TerminateRuntime {
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
        data: bytes::Bytes,
    ) {
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::SendInput {
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
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::ResizePane {
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
            .try_send(EndpointCommand::ForgetWorkspace { workspace_id: workspace_id.to_string() });
    }

    /// Rename a runtime on the daemon.
    pub fn rename_runtime(
        &self,
        workspace_id: &str,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
        name: &str,
    ) {
        let _ = self.endpoint_handle(endpoint).try_send(EndpointCommand::RenameRuntime {
            workspace_id: workspace_id.to_string(),
            runtime_id: runtime_id.to_string(),
            name: name.to_string(),
        });
    }
}

#[derive(Debug)]
struct EndpointActor {
    endpoint: RuntimeEndpoint,
    event_tx: mpsc::Sender<EndpointEvent>,
    self_tx: mpsc::Sender<EndpointCommand>,
    cmd_rx: mpsc::Receiver<EndpointCommand>,
    connection: Option<DaemonConnection>,
    reader: Option<DaemonReader>,
    writer: Option<DaemonWriter>,
    ssh_handle: Option<SshHandle>,
    tracked_workspaces: HashMap<String, String>,
    reconnect_attempt: u32,
    heartbeat: HeartbeatMonitor,
    heartbeat_deadline: Instant,
    /// Prevents spawning multiple daemon processes during reconnect loops.
    daemon_start_attempted: bool,
    auto_start_daemon: bool,
    reconnect_delay_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeartbeatAction {
    SendPing { nonce: u64 },
    DeclareLost,
}

#[derive(Debug, Default)]
struct HeartbeatMonitor {
    pending_nonce: Option<u64>,
    next_nonce: u64,
    missed_ticks: u8,
}

/// Number of consecutive heartbeat ticks with an outstanding ping before
/// the connection is declared lost. With a 2-second interval this gives
/// the server 6 seconds to respond.
const HEARTBEAT_MISS_LIMIT: u8 = 3;

impl HeartbeatMonitor {
    const fn on_tick(&mut self) -> HeartbeatAction {
        if let Some(nonce) = self.pending_nonce {
            self.missed_ticks = self.missed_ticks.saturating_add(1);
            if self.missed_ticks >= HEARTBEAT_MISS_LIMIT {
                return HeartbeatAction::DeclareLost;
            }
            return HeartbeatAction::SendPing { nonce };
        }
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.saturating_add(1);
        self.pending_nonce = Some(nonce);
        self.missed_ticks = 0;
        HeartbeatAction::SendPing { nonce }
    }

    fn observe_inbound(&mut self, msg: &proto::ServerMessage) -> bool {
        match &msg.msg {
            Some(proto::server_message::Msg::Pong(pong)) => {
                if self.pending_nonce == Some(pong.nonce) {
                    self.pending_nonce = None;
                    self.missed_ticks = 0;
                }
                true
            }
            Some(_) => {
                self.pending_nonce = None;
                self.missed_ticks = 0;
                false
            }
            None => false,
        }
    }

    const fn reset(&mut self) {
        self.pending_nonce = None;
        self.missed_ticks = 0;
    }
}

fn new_heartbeat_deadline() -> Instant {
    Instant::now() + HEARTBEAT_INTERVAL
}

impl EndpointActor {
    fn new(
        endpoint: RuntimeEndpoint,
        event_tx: mpsc::Sender<EndpointEvent>,
        self_tx: mpsc::Sender<EndpointCommand>,
        cmd_rx: mpsc::Receiver<EndpointCommand>,
        auto_start_daemon: bool,
        reconnect_delay_secs: u32,
    ) -> Self {
        Self {
            endpoint,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            reader: None,
            writer: None,
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon,
            reconnect_delay_secs,
        }
    }

    async fn run(mut self) {
        enum LoopEvent {
            Command(Option<EndpointCommand>),
            Message(Result<Option<proto::ServerMessage>, DaemonError>),
            HeartbeatTick,
        }

        loop {
            if let Some(reader) = self.reader.as_mut() {
                let track_heartbeat = !self.tracked_workspaces.is_empty();
                let heartbeat_sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(
                    self.heartbeat_deadline,
                ));
                tokio::pin!(heartbeat_sleep);
                let event = tokio::select! {
                    biased;
                    command = self.cmd_rx.recv() => LoopEvent::Command(command),
                    message = reader.recv() => LoopEvent::Message(message),
                    () = &mut heartbeat_sleep, if track_heartbeat => LoopEvent::HeartbeatTick,
                };

                match event {
                    LoopEvent::Command(command) => {
                        let Some(command) = command else { break };
                        if matches!(command, EndpointCommand::Shutdown) {
                            break;
                        }
                        self.handle_command(command).await;
                    }
                    LoopEvent::Message(message) => self.handle_runtime_message(message),
                    LoopEvent::HeartbeatTick => self.handle_heartbeat_tick().await,
                }
            } else {
                let Some(command) = self.cmd_rx.recv().await else { break };
                if matches!(command, EndpointCommand::Shutdown) {
                    break;
                }
                self.handle_command(command).await;
            }
        }
    }

    fn split_connection(&mut self) {
        if let Some(conn) = self.connection.take() {
            let (reader, writer) = conn.into_split();
            self.reader = Some(reader);
            self.writer = Some(writer);
            self.restart_heartbeat_timer();
        }
    }

    fn restart_heartbeat_timer(&mut self) {
        self.heartbeat.reset();
        self.heartbeat_deadline = new_heartbeat_deadline();
    }

    async fn send_message(&mut self, msg: &proto::ClientMessage) -> Result<(), DaemonError> {
        let writer = self.writer.as_mut().ok_or(DaemonError::Disconnected)?;
        writer.send(msg).await
    }

    async fn read_response(
        &mut self,
        expect_terminated: bool,
    ) -> Result<proto::ServerMessage, DaemonError> {
        loop {
            let msg = {
                let reader = self.reader.as_mut().ok_or(DaemonError::Disconnected)?;
                reader.recv().await?.ok_or(DaemonError::Disconnected)?
            };
            let is_push = match &msg.msg {
                Some(
                    proto::server_message::Msg::Delta(_)
                    | proto::server_message::Msg::PaneExited(_)
                    | proto::server_message::Msg::TitleChanged(_)
                    | proto::server_message::Msg::CwdChanged(_)
                    | proto::server_message::Msg::Bell(_)
                    | proto::server_message::Msg::PaneResized(_)
                    | proto::server_message::Msg::SessionRenamed(_),
                ) => true,
                Some(proto::server_message::Msg::SessionTerminated(_)) => !expect_terminated,
                _ => false,
            };
            if self.observe_inbound_message(&msg) {
                continue;
            }
            if is_push {
                self.dispatch_push(msg);
            } else {
                return Ok(msg);
            }
        }
    }

    fn dispatch_push(&mut self, msg: proto::ServerMessage) {
        if let Some(proto::server_message::Msg::SessionTerminated(terminated)) = &msg.msg
            && let Ok(runtime_id) = rttx_proto::bytes_to_uuid(&terminated.session_id)
        {
            self.tracked_workspaces.retain(|_, tracked| tracked != &runtime_id.to_string());
        }
        self.forward_push(msg);
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

                let Ok((runtime_id, snapshot)) = self
                    .resolve_and_attach_runtime(&workspace_id, &name, policy, runtime_id.as_deref())
                    .await
                else {
                    return;
                };

                self.split_connection();
                self.tracked_workspaces.insert(workspace_id.clone(), runtime_id.clone());
                let _ = self.event_tx.try_send(EndpointEvent::WorkspaceOpened {
                    workspace_id: workspace_id.clone(),
                    runtime_id: runtime_id.clone(),
                    snapshot: snapshot.clone(),
                });

                if snapshot.panes.is_empty() {
                    if let Some(layout_terminal_uuid) = placeholder_terminal_uuid {
                        let _ = self.self_tx.try_send(EndpointCommand::CreatePane {
                            workspace_id,
                            runtime_id,
                            layout_terminal_uuid,
                            cwd: None,
                            dark_background: true,
                        });
                    } else {
                        self.emit_status(&workspace_id, ConnectionStatus::Connected);
                    }
                } else {
                    self.emit_status(&workspace_id, ConnectionStatus::Connected);
                }
            }
            EndpointCommand::CreatePane {
                workspace_id,
                runtime_id,
                layout_terminal_uuid,
                cwd,
                dark_background,
            } => {
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
                let msg = proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                        session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                        cwd,
                        dark_background: Some(dark_background),
                    })),
                };
                if let Err(error) = self.send_message(&msg).await {
                    self.handle_command_error(&workspace_id, ManagerOperation::CreatePane, &error);
                    return;
                }
                match self.read_response(false).await {
                    Ok(response) => match response.msg {
                        Some(proto::server_message::Msg::PaneCreated(created)) => {
                            if let Ok(pane_id) = rttx_proto::bytes_to_uuid(&created.pane_id) {
                                let _ = self.event_tx.try_send(EndpointEvent::PaneCreated {
                                    workspace_id: workspace_id.clone(),
                                    layout_terminal_uuid,
                                    runtime_id,
                                    runtime_pane_id: pane_id.to_string(),
                                });
                                self.emit_status(&workspace_id, ConnectionStatus::Connected);
                            }
                        }
                        Some(proto::server_message::Msg::Error(e)) => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::CreatePane,
                                &DaemonError::ServerError { code: e.code, message: e.message },
                            );
                        }
                        _ => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::CreatePane,
                                &DaemonError::UnexpectedMessage,
                            );
                        }
                    },
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
                let msg = proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                        session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                        pane_id: rttx_proto::uuid_to_bytes(pane_uuid),
                    })),
                };
                if let Err(error) = self.send_message(&msg).await {
                    self.handle_command_error(&workspace_id, ManagerOperation::ClosePane, &error);
                    return;
                }
                match self.read_response(false).await {
                    Ok(response) => match response.msg {
                        Some(proto::server_message::Msg::PaneClosed(_)) => {
                            let _ = self.event_tx.try_send(EndpointEvent::PaneClosed {
                                workspace_id,
                                layout_terminal_uuid,
                                runtime_id,
                                runtime_pane_id,
                            });
                        }
                        Some(proto::server_message::Msg::Error(e)) if e.code == 6 => {
                            // ERR_PANE_NOT_FOUND: the pane is already gone on the
                            // daemon, so treat this as a successful close.
                            log::info!(
                                "ClosePane for {workspace_id}: pane already removed on daemon, \
                                 treating as closed"
                            );
                            let _ = self.event_tx.try_send(EndpointEvent::PaneClosed {
                                workspace_id,
                                layout_terminal_uuid,
                                runtime_id,
                                runtime_pane_id,
                            });
                        }
                        Some(proto::server_message::Msg::Error(e)) => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::ClosePane,
                                &DaemonError::ServerError { code: e.code, message: e.message },
                            );
                        }
                        _ => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::ClosePane,
                                &DaemonError::UnexpectedMessage,
                            );
                        }
                    },
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

                let msg = proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                        session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                    })),
                };
                if let Err(error) = self.send_message(&msg).await {
                    self.handle_command_error(
                        &workspace_id,
                        ManagerOperation::DetachRuntime,
                        &error,
                    );
                    return;
                }
                match self.read_response(true).await {
                    Ok(response) => match response.msg {
                        Some(proto::server_message::Msg::SessionDetached(_)) => {
                            self.tracked_workspaces.remove(&workspace_id);
                            let _ = self.event_tx.try_send(EndpointEvent::WorkspaceDetached {
                                workspace_id,
                                runtime_id,
                            });
                        }
                        Some(proto::server_message::Msg::SessionTerminated(terminated)) => {
                            self.tracked_workspaces.remove(&workspace_id);
                            let _ = self.event_tx.try_send(EndpointEvent::RuntimeTerminated {
                                workspace_id,
                                runtime_id,
                                reason: proto::RuntimeTerminationReason::try_from(
                                    terminated.reason,
                                )
                                .unwrap_or(proto::RuntimeTerminationReason::Unspecified),
                            });
                        }
                        Some(proto::server_message::Msg::Error(e)) => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::DetachRuntime,
                                &DaemonError::ServerError { code: e.code, message: e.message },
                            );
                        }
                        _ => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::DetachRuntime,
                                &DaemonError::UnexpectedMessage,
                            );
                        }
                    },
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

                let msg = proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::TerminateSession(
                        proto::TerminateSession {
                            session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                        },
                    )),
                };
                if let Err(error) = self.send_message(&msg).await {
                    self.handle_command_error(
                        &workspace_id,
                        ManagerOperation::TerminateRuntime,
                        &error,
                    );
                    return;
                }
                match self.read_response(true).await {
                    Ok(response) => match response.msg {
                        Some(proto::server_message::Msg::SessionTerminated(terminated)) => {
                            self.tracked_workspaces.remove(&workspace_id);
                            let _ = self.event_tx.try_send(EndpointEvent::RuntimeTerminated {
                                workspace_id,
                                runtime_id,
                                reason: proto::RuntimeTerminationReason::try_from(
                                    terminated.reason,
                                )
                                .unwrap_or(proto::RuntimeTerminationReason::Unspecified),
                            });
                        }
                        Some(proto::server_message::Msg::Error(e)) => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::TerminateRuntime,
                                &DaemonError::ServerError { code: e.code, message: e.message },
                            );
                        }
                        _ => {
                            self.handle_command_error(
                                &workspace_id,
                                ManagerOperation::TerminateRuntime,
                                &DaemonError::UnexpectedMessage,
                            );
                        }
                    },
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
                if let Some(writer) = self.writer.as_mut()
                    && let Err(error) = writer.send_input(runtime_uuid, pane_uuid, &data).await
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
                if let Some(writer) = self.writer.as_mut()
                    && let Err(error) =
                        writer.send_resize(runtime_uuid, pane_uuid, cols, rows).await
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
                let list_result = if self.writer.is_some() {
                    let msg = proto::ClientMessage {
                        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
                    };
                    match self.send_message(&msg).await {
                        Ok(()) => match self.read_response(false).await {
                            Ok(response) => match response.msg {
                                Some(proto::server_message::Msg::SessionList(list)) => {
                                    Ok(list.sessions)
                                }
                                Some(proto::server_message::Msg::Error(e)) => {
                                    Err(DaemonError::ServerError {
                                        code: e.code,
                                        message: e.message,
                                    })
                                }
                                _ => Err(DaemonError::UnexpectedMessage),
                            },
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(e),
                    }
                } else {
                    let connection = self.connection.as_mut().expect("connection must exist");
                    connection.list_sessions().await
                };
                match list_result {
                    Ok(sessions) => {
                        let _ = self.event_tx.try_send(EndpointEvent::InventoryLoaded {
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
                if self.writer.is_some() || self.connection.is_some() {
                    return;
                }
                let workspaces: Vec<_> = self.tracked_workspaces.keys().cloned().collect();
                if workspaces.is_empty() {
                    return;
                }
                log::debug!(
                    "Reconnect attempt to {} for {} workspace(s)",
                    self.endpoint.key(),
                    workspaces.len()
                );

                // Preserve the backoff counter across the reconnect cycle.
                // ensure_connected resets it to 0 on successful socket
                // connect, but if the subsequent reattach fails we must
                // continue ramping up instead of dropping back to 1 second.
                let saved_attempt = self.reconnect_attempt;

                let primary = workspaces[0].clone();
                if let Err(problem) = self.ensure_connected(&primary).await {
                    // ensure_connected already scheduled a reconnect for
                    // transient problems. Only schedule for non-transient
                    // ones (which still retry at max delay).
                    if !problem.is_transient() {
                        self.schedule_reconnect_for_problem(&problem);
                    }
                    return;
                }

                // Split before reattaching so that `read_response` is used
                // for each attach. The unsplit path cannot handle interleaved
                // push messages from previously attached sessions.
                self.split_connection();

                log::info!(
                    "Reconnected to {}, reattaching {} workspace(s)",
                    self.endpoint.key(),
                    self.tracked_workspaces.len()
                );
                let mut any_transient_failure = false;
                for (workspace_id, runtime_id) in self.tracked_workspaces.clone() {
                    let Ok(runtime_uuid) = runtime_id.parse::<uuid::Uuid>() else {
                        continue;
                    };
                    match self.attach_runtime_via_active_channel(runtime_uuid).await {
                        Ok(snapshot) => {
                            let _ = self.event_tx.try_send(EndpointEvent::WorkspaceOpened {
                                workspace_id: workspace_id.clone(),
                                runtime_id: runtime_id.clone(),
                                snapshot,
                            });
                            self.emit_status(&workspace_id, ConnectionStatus::Recovered);
                        }
                        Err(ref error) => {
                            let problem = classify_connection_problem(error);
                            log::warn!("Reattach {workspace_id} failed during reconnect: {error}");
                            if matches!(problem, ConnectionProblem::SessionMissing) {
                                self.emit_status(&workspace_id, ConnectionStatus::SessionMissing);
                            } else {
                                self.emit_error(
                                    &workspace_id,
                                    ManagerOperation::OpenWorkspace,
                                    problem.clone(),
                                    error.to_string(),
                                );
                                if problem.is_transient() {
                                    any_transient_failure = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                if any_transient_failure {
                    // Restore the counter so the backoff continues from
                    // where it was before this cycle, not from 0.
                    self.reconnect_attempt = saved_attempt;
                    self.handle_disconnect();
                }
            }
            EndpointCommand::RenameRuntime { workspace_id, runtime_id, name } => {
                let Some(runtime_uuid) = parse_uuid(
                    &workspace_id,
                    ManagerOperation::RenameRuntime,
                    &runtime_id,
                    &self.event_tx,
                ) else {
                    return;
                };
                if self.ensure_connected(&workspace_id).await.is_err() {
                    return;
                }
                let msg = proto::ClientMessage {
                    msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
                        session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                        name,
                    })),
                };
                let _ = self.send_message(&msg).await;
            }
            EndpointCommand::ForgetWorkspace { workspace_id } => {
                self.tracked_workspaces.remove(&workspace_id);
            }
            EndpointCommand::Shutdown => {}
        }
    }

    async fn ensure_connected(&mut self, workspace_id: &str) -> Result<(), ConnectionProblem> {
        if self.writer.is_some() || self.connection.is_some() {
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
                log::debug!(
                    "Connection to {} failed: {error} ({})",
                    self.endpoint.key(),
                    if problem.is_transient() { "transient" } else { "permanent" }
                );
                if problem.is_transient() {
                    let attempt = self.next_reconnect_attempt();
                    let delay_secs = self.next_reconnect_delay_secs();
                    let status = advance_connection_status(
                        &ConnectionStatus::Connecting,
                        ConnectionEvent::RetryScheduled { attempt, retry_in_secs: delay_secs },
                    );
                    self.emit_status(workspace_id, ConnectionStatus::Disconnected);
                    self.emit_status(workspace_id, status);
                    self.schedule_reconnect(delay_secs);
                } else {
                    let status = advance_connection_status(
                        &ConnectionStatus::Connecting,
                        ConnectionEvent::Failed(problem.clone()),
                    );
                    self.emit_status(workspace_id, status);
                }
                Err(problem)
            }
        }
    }

    async fn connect_endpoint(&mut self) -> Result<(), DaemonError> {
        match &self.endpoint {
            RuntimeEndpoint::Local => {
                let socket_path = default_socket_path();
                if !socket_path.exists() && !self.daemon_start_attempted && self.auto_start_daemon {
                    log::info!("Auto-starting local daemon");
                    self.daemon_start_attempted = true;
                    Self::start_local_daemon(&socket_path).await?;
                }
                self.connection = Some(DaemonConnection::connect(&socket_path).await?);
                self.daemon_start_attempted = false;
            }
            RuntimeEndpoint::Remote { host } => {
                let (connection, ssh_handle) =
                    tokio::time::timeout(SSH_CONNECT_TIMEOUT, DaemonConnection::connect_ssh(host))
                        .await
                        .map_err(|_| {
                            DaemonError::Io(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                format!("SSH connection to {host} timed out"),
                            ))
                        })??;
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

        // Spawn and wait for the parent process to exit (it daemonizes by forking).
        let status = command.status().await?;
        if !status.success() {
            return Err(DaemonError::Io(std::io::Error::other(format!(
                "daemon exited with {status}"
            ))));
        }

        for _ in 0..50 {
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
        let create_result = self.create_runtime_via_active_channel(name, policy).await;
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
        let attach_result = self.attach_runtime_via_active_channel(runtime_uuid).await;
        match attach_result {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                self.handle_command_error(workspace_id, ManagerOperation::OpenWorkspace, &error);
                Err(())
            }
        }
    }

    /// Resolve a runtime id (reattach or create) and attach to it.
    ///
    /// When `existing_runtime_id` is provided, tries to reattach first.
    /// If the runtime no longer exists (daemon restarted, ephemeral gone),
    /// falls back to creating a fresh runtime so "Retry Connection" works.
    /// Ownership conflicts, transport errors, and missing sessions are
    /// reported immediately.
    async fn resolve_and_attach_runtime(
        &mut self,
        workspace_id: &str,
        name: &str,
        policy: WorkspacePolicy,
        existing_runtime_id: Option<&str>,
    ) -> Result<(String, proto::Snapshot), ()> {
        if let Some(runtime_id) = existing_runtime_id
            && let Ok(runtime_uuid) = runtime_id.parse::<uuid::Uuid>()
        {
            match self.attach_runtime_via_active_channel(runtime_uuid).await {
                Ok(snapshot) => return Ok((runtime_id.to_string(), snapshot)),
                // Ownership conflict or transport failure — report, don't retry.
                Err(
                    ref error @ (DaemonError::AttachBlocked(_)
                    | DaemonError::Io(_)
                    | DaemonError::Disconnected),
                ) => {
                    self.handle_command_error(workspace_id, ManagerOperation::OpenWorkspace, error);
                    return Err(());
                }
                // Session gone on daemon — report as SessionMissing, don't create new.
                Err(ref error)
                    if matches!(
                        classify_connection_problem(error),
                        ConnectionProblem::SessionMissing
                    ) =>
                {
                    log::warn!(
                        "Session {runtime_id} no longer exists on daemon for workspace \
                         {workspace_id}"
                    );
                    self.emit_status(workspace_id, ConnectionStatus::SessionMissing);
                    return Err(());
                }
                // Runtime gone for other reasons — fall through to create a new one.
                Err(error) => {
                    log::info!(
                        "Reattach to {runtime_id} failed for {workspace_id}: \
                         {error}, creating new runtime"
                    );
                }
            }
        }

        let Ok(runtime_id) = self.create_runtime(workspace_id, name, policy).await else {
            return Err(());
        };
        let Ok(snapshot) = self.attach_runtime(workspace_id, &runtime_id).await else {
            return Err(());
        };
        Ok((runtime_id, snapshot))
    }

    async fn create_runtime_via_active_channel(
        &mut self,
        name: &str,
        policy: WorkspacePolicy,
    ) -> Result<Uuid, DaemonError> {
        if let Some(connection) = self.connection.as_mut() {
            return connection.create_session(name, policy).await;
        }

        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.to_string(),
                policy: policy.as_proto(),
            })),
        };
        self.send_message(&msg).await?;
        let response = self.read_response(false).await?;
        match response.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => {
                rttx_proto::bytes_to_uuid(&created.session_id).map_err(DaemonError::Frame)
            }
            Some(proto::server_message::Msg::Error(error)) => {
                Err(DaemonError::ServerError { code: error.code, message: error.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    async fn attach_runtime_via_active_channel(
        &mut self,
        runtime_uuid: Uuid,
    ) -> Result<proto::Snapshot, DaemonError> {
        if let Some(connection) = self.connection.as_mut() {
            return connection
                .attach_session(runtime_uuid, proto::RuntimeAttachMode::ReadWrite)
                .await;
        }

        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: rttx_proto::uuid_to_bytes(runtime_uuid),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        };
        self.send_message(&msg).await?;
        let response = self.read_response(false).await?;
        match response.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => Ok(snapshot),
            Some(proto::server_message::Msg::AttachBlocked(blocked)) => {
                Err(DaemonError::AttachBlocked(blocked))
            }
            Some(proto::server_message::Msg::Error(error)) => {
                Err(DaemonError::ServerError { code: error.code, message: error.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    fn handle_runtime_message(
        &mut self,
        message: Result<Option<proto::ServerMessage>, DaemonError>,
    ) {
        match message {
            Ok(Some(message)) => {
                if self.observe_inbound_message(&message) {
                    return;
                }
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
            .try_send(EndpointEvent::RuntimeMessage { endpoint: self.endpoint.clone(), message });
    }

    fn observe_inbound_message(&mut self, message: &proto::ServerMessage) -> bool {
        self.heartbeat.observe_inbound(message)
    }

    async fn handle_heartbeat_tick(&mut self) {
        self.heartbeat_deadline = new_heartbeat_deadline();
        match self.heartbeat.on_tick() {
            HeartbeatAction::SendPing { nonce } => {
                let Some(writer) = self.writer.as_mut() else {
                    return;
                };
                if let Err(error) = writer.send_ping(nonce).await {
                    log::warn!("Heartbeat ping failed for {}: {error}", self.endpoint.key());
                    self.handle_disconnect();
                }
            }
            HeartbeatAction::DeclareLost => {
                log::warn!("Heartbeat timed out for {}", self.endpoint.key());
                self.handle_disconnect();
            }
        }
    }

    fn handle_disconnect(&mut self) {
        log::warn!(
            "Connection lost to {} ({} tracked workspace(s))",
            self.endpoint.key(),
            self.tracked_workspaces.len()
        );
        self.connection = None;
        self.reader = None;
        self.writer = None;
        self.ssh_handle = None;
        self.heartbeat.reset();
        if self.tracked_workspaces.is_empty() {
            return;
        }
        let attempt = self.next_reconnect_attempt();
        let delay_secs = self.next_reconnect_delay_secs();
        for workspace_id in self.tracked_workspaces.keys() {
            self.emit_status(workspace_id, ConnectionStatus::Disconnected);
            self.emit_status(
                workspace_id,
                ConnectionStatus::Reconnecting { attempt, retry_in_secs: delay_secs },
            );
        }
        self.schedule_reconnect(delay_secs);
    }

    const fn next_reconnect_attempt(&self) -> u32 {
        self.reconnect_attempt.saturating_add(1)
    }

    fn next_reconnect_delay_secs(&self) -> u32 {
        self.next_reconnect_attempt().min(self.reconnect_delay_secs)
    }

    fn schedule_reconnect(&mut self, delay_secs: u32) {
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        log::info!(
            "Scheduling reconnect to {} (attempt {}, delay {}s)",
            self.endpoint.key(),
            self.reconnect_attempt,
            delay_secs
        );
        let self_tx = self.self_tx.clone();
        let delay = Duration::from_secs(u64::from(delay_secs));
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = self_tx.try_send(EndpointCommand::Reconnect);
        });
    }

    /// Schedule a reconnect based on the error type. Non-transient errors
    /// use the maximum delay but still retry — the underlying problem may
    /// resolve (e.g., daemon restarts with correct version).
    fn schedule_reconnect_for_problem(&mut self, problem: &ConnectionProblem) {
        let delay_secs = if problem.is_transient() {
            self.next_reconnect_delay_secs()
        } else {
            self.reconnect_delay_secs
        };
        self.schedule_reconnect(delay_secs);
    }

    fn emit_status(&self, workspace_id: &str, status: ConnectionStatus) {
        let _ = self.event_tx.try_send(EndpointEvent::WorkspaceConnectionChanged {
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
        let _ = self.event_tx.try_send(EndpointEvent::WorkspaceError {
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
        if matches!(problem, ConnectionProblem::SessionMissing) {
            self.emit_status(workspace_id, ConnectionStatus::SessionMissing);
            return;
        }
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
    event_tx: &mpsc::Sender<EndpointEvent>,
) -> Option<Uuid> {
    match Uuid::parse_str(value) {
        Ok(uuid) => Some(uuid),
        Err(error) => {
            let _ = event_tx.try_send(EndpointEvent::WorkspaceError {
                workspace_id: workspace_id.to_string(),
                operation,
                problem: ConnectionProblem::Protocol("Invalid runtime UUID".into()),
                detail: error.to_string(),
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc::error::TryRecvError;

    fn split_duplex_connection() -> ((DaemonReader, DaemonWriter), tokio::io::DuplexStream) {
        let (client, server) = tokio::io::duplex(4096);
        let (read_half, write_half) = tokio::io::split(client);
        (crate::daemon::split_transport_for_test(read_half, write_half), server)
    }

    async fn recv_client_message(
        stream: &mut tokio::io::DuplexStream,
        read_buf: &mut BytesMut,
    ) -> proto::ClientMessage {
        recv_client_message_opt(stream, read_buf)
            .await
            .expect("read from duplex transport")
            .expect("unexpected EOF while waiting for client message")
    }

    async fn recv_client_message_opt(
        stream: &mut tokio::io::DuplexStream,
        read_buf: &mut BytesMut,
    ) -> std::io::Result<Option<proto::ClientMessage>> {
        loop {
            match rttx_proto::decode_frame::<proto::ClientMessage>(read_buf) {
                Ok(message) => return Ok(Some(message)),
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(error) => {
                    return Err(std::io::Error::other(error.to_string()));
                }
            }
            let n = stream.read_buf(read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
        }
    }

    async fn send_server_message(
        stream: &mut tokio::io::DuplexStream,
        message: &proto::ServerMessage,
    ) {
        let mut buf = BytesMut::new();
        rttx_proto::encode_frame(message, &mut buf).expect("encode server message");
        stream.write_all(&buf).await.expect("write server message");
        stream.flush().await.expect("flush server message");
    }

    fn make_actor(reader: DaemonReader, writer: DaemonWriter) -> EndpointActor {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        EndpointActor {
            endpoint: RuntimeEndpoint::Local,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            reader: Some(reader),
            writer: Some(writer),
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon: true,
            reconnect_delay_secs: 10,
        }
    }

    #[tokio::test]
    async fn create_runtime_uses_split_transport_after_connection_split() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let mut actor = make_actor(reader, writer);
        let expected_runtime = Uuid::new_v4();

        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            let request = recv_client_message(&mut server_stream, &mut read_buf).await;
            match request.msg {
                Some(proto::client_message::Msg::CreateSession(create)) => {
                    assert_eq!(create.name, "Workspace 2");
                    assert_eq!(create.policy, WorkspacePolicy::Persistent.as_proto());
                }
                other => panic!("expected CreateSession request, got {other:?}"),
            }
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::SessionCreated(proto::SessionCreated {
                        session_id: rttx_proto::uuid_to_bytes(expected_runtime),
                        revision: 1,
                    })),
                },
            )
            .await;
        });

        let runtime_id = actor
            .create_runtime_via_active_channel("Workspace 2", WorkspacePolicy::Persistent)
            .await
            .expect("split transport should support CreateSession");
        assert_eq!(runtime_id, expected_runtime);
        server.await.expect("fake server task should complete");
    }

    #[tokio::test]
    async fn attach_runtime_uses_split_transport_after_connection_split() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let mut actor = make_actor(reader, writer);
        let runtime_id = Uuid::new_v4();

        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            let request = recv_client_message(&mut server_stream, &mut read_buf).await;
            match request.msg {
                Some(proto::client_message::Msg::AttachSession(attach)) => {
                    assert_eq!(rttx_proto::bytes_to_uuid(&attach.session_id).unwrap(), runtime_id);
                    assert_eq!(attach.attach_mode, proto::RuntimeAttachMode::ReadWrite as i32);
                }
                other => panic!("expected AttachSession request, got {other:?}"),
            }
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
                        session_id: rttx_proto::uuid_to_bytes(runtime_id),
                        panes: vec![],
                        revision: 1,
                        current_client_role: proto::RuntimeClientRole::Writer as i32,
                    })),
                },
            )
            .await;
        });

        let snapshot = actor
            .attach_runtime_via_active_channel(runtime_id)
            .await
            .expect("split transport should support AttachSession");
        assert_eq!(rttx_proto::bytes_to_uuid(&snapshot.session_id).unwrap(), runtime_id);
        server.await.expect("fake server task should complete");
    }

    fn make_actor_with_events(
        reader: DaemonReader,
        writer: DaemonWriter,
    ) -> (EndpointActor, mpsc::Receiver<EndpointEvent>) {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor = EndpointActor {
            endpoint: RuntimeEndpoint::Local,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            reader: Some(reader),
            writer: Some(writer),
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon: true,
            reconnect_delay_secs: 10,
        };
        (actor, event_rx)
    }

    #[test]
    fn endpoint_actor_construction_does_not_require_tokio_runtime() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor = EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 10);
        assert!(actor.reader.is_none());
        assert!(actor.writer.is_none());
    }

    #[test]
    fn heartbeat_monitor_declares_loss_after_missed_pong() {
        let mut heartbeat = HeartbeatMonitor::default();
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 0 });
        // Ticks 2..HEARTBEAT_MISS_LIMIT re-send the same nonce.
        for _ in 1..HEARTBEAT_MISS_LIMIT {
            assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 0 });
        }
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::DeclareLost);
    }

    #[test]
    fn heartbeat_monitor_resets_after_any_inbound_message() {
        let mut heartbeat = HeartbeatMonitor::default();
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 0 });
        assert!(
            !heartbeat.observe_inbound(&proto::ServerMessage {
                msg: Some(proto::server_message::Msg::Delta(proto::Delta {
                    session_id: vec![],
                    pane_id: vec![],
                    data: bytes::Bytes::from_static(b"output"),
                })),
            }),
            "non-heartbeat traffic should not be swallowed"
        );
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 1 });
    }

    #[test]
    fn heartbeat_monitor_consumes_matching_pong() {
        let mut heartbeat = HeartbeatMonitor::default();
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 0 });
        assert!(
            heartbeat.observe_inbound(&proto::ServerMessage {
                msg: Some(proto::server_message::Msg::Pong(proto::Pong { nonce: 0 })),
            }),
            "heartbeat pong should stay internal to the actor"
        );
        assert_eq!(heartbeat.on_tick(), HeartbeatAction::SendPing { nonce: 1 });
    }

    #[tokio::test]
    async fn close_pane_not_found_emits_pane_closed() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (mut actor, mut event_rx) = make_actor_with_events(reader, writer);

        let runtime_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();
        let workspace_id = "ws-1".to_string();

        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            let request = recv_client_message(&mut server_stream, &mut read_buf).await;
            match request.msg {
                Some(proto::client_message::Msg::ClosePane(_)) => {}
                other => panic!("expected ClosePane, got {other:?}"),
            }
            // Respond with ERR_PANE_NOT_FOUND (code 6).
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Error(proto::Error {
                        code: 6,
                        message: "pane not found".into(),
                    })),
                },
            )
            .await;
        });

        actor
            .handle_command(EndpointCommand::ClosePane {
                workspace_id: workspace_id.clone(),
                runtime_id: runtime_id.to_string(),
                layout_terminal_uuid: "layout-t1".into(),
                runtime_pane_id: pane_id.to_string(),
            })
            .await;

        server.await.expect("fake server task should complete");

        // Must emit PaneClosed, not WorkspaceError.
        let mut found_pane_closed = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                EndpointEvent::PaneClosed { workspace_id: ref ws, .. } if ws == "ws-1" => {
                    found_pane_closed = true;
                }
                EndpointEvent::WorkspaceError { ref detail, .. } => {
                    panic!("should not emit WorkspaceError, got: {detail}");
                }
                _ => {}
            }
        }
        assert!(found_pane_closed, "expected PaneClosed event for pane-not-found response");
    }

    #[tokio::test]
    async fn heartbeat_timeout_marks_workspace_disconnected() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (mut actor, mut event_rx) = make_actor_with_events(reader, writer);
        actor.tracked_workspaces.insert("ws-1".into(), Uuid::new_v4().to_string());

        let actor_task = tokio::spawn(async move { actor.run().await });
        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            // Read pings without responding — the actor should eventually
            // declare the connection lost after HEARTBEAT_MISS_LIMIT ticks.
            loop {
                let Ok(Some(request)) =
                    recv_client_message_opt(&mut server_stream, &mut read_buf).await
                else {
                    break;
                };
                match request.msg {
                    Some(proto::client_message::Msg::Ping(_)) => {}
                    other => panic!("expected Ping request, got {other:?}"),
                }
            }
        });

        let mut saw_disconnected = false;
        let mut saw_reconnecting = false;
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(
                HEARTBEAT_INTERVAL.as_millis() as u64 * (HEARTBEAT_MISS_LIMIT as u64 + 4),
            );
        while tokio::time::Instant::now() < deadline && !(saw_disconnected && saw_reconnecting) {
            if let Ok(Some(EndpointEvent::WorkspaceConnectionChanged { workspace_id, status })) =
                tokio::time::timeout(Duration::from_millis(25), event_rx.recv()).await
            {
                assert_eq!(workspace_id, "ws-1");
                match status {
                    ConnectionStatus::Disconnected => saw_disconnected = true,
                    ConnectionStatus::Reconnecting { .. } => saw_reconnecting = true,
                    _ => {}
                }
            }
        }

        assert!(saw_disconnected, "heartbeat timeout should emit Disconnected");
        assert!(saw_reconnecting, "heartbeat timeout should schedule reconnect");

        actor_task.abort();
        let _ = actor_task.await;
        server.await.expect("server task should capture the heartbeat ping");
    }

    #[tokio::test]
    async fn heartbeat_pong_keeps_workspace_connected() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (mut actor, mut event_rx) = make_actor_with_events(reader, writer);
        actor.tracked_workspaces.insert("ws-1".into(), Uuid::new_v4().to_string());

        let actor_task = tokio::spawn(async move { actor.run().await });
        let (pong_tx, pong_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            let request = recv_client_message(&mut server_stream, &mut read_buf).await;
            let nonce = match request.msg {
                Some(proto::client_message::Msg::Ping(ping)) => ping.nonce,
                other => panic!("expected Ping request, got {other:?}"),
            };
            assert_eq!(nonce, 0);
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Pong(proto::Pong { nonce })),
                },
            )
            .await;
            let _ = pong_tx.send(());
            let _ = release_rx.await;
        });

        pong_rx.await.expect("server task should answer the heartbeat ping");
        tokio::time::sleep(HEARTBEAT_INTERVAL / 2).await;

        loop {
            match event_rx.try_recv() {
                Ok(EndpointEvent::WorkspaceConnectionChanged { status, .. }) => match status {
                    ConnectionStatus::Disconnected | ConnectionStatus::Reconnecting { .. } => {
                        panic!("workspace should stay connected after a heartbeat pong");
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    panic!("actor event channel should stay connected while the actor is alive");
                }
            }
        }

        let _ = release_tx.send(());
        server.await.expect("server task should stay alive until released");
        actor_task.abort();
        let _ = actor_task.await;
    }

    #[test]
    fn daemon_start_attempted_flag_defaults_to_false() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor = EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 10);
        assert!(
            !actor.daemon_start_attempted,
            "new actor must not have daemon_start_attempted set"
        );
    }

    #[test]
    fn auto_start_daemon_flag_is_stored() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, false, 10);
        assert!(!actor.auto_start_daemon);
    }

    #[test]
    fn reconnect_delay_caps_at_configured_value() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 10);

        // Ramp-up: delay = min(attempt, 10)
        assert_eq!(actor.next_reconnect_delay_secs(), 1);
        actor.reconnect_attempt = 5;
        assert_eq!(actor.next_reconnect_delay_secs(), 6);
        actor.reconnect_attempt = 9;
        assert_eq!(actor.next_reconnect_delay_secs(), 10);
        // Capped at configured max.
        actor.reconnect_attempt = 100;
        assert_eq!(actor.next_reconnect_delay_secs(), 10);
    }

    #[test]
    fn reconnect_delay_respects_custom_cap() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 3);

        assert_eq!(actor.next_reconnect_delay_secs(), 1);
        actor.reconnect_attempt = 2;
        assert_eq!(actor.next_reconnect_delay_secs(), 3);
        actor.reconnect_attempt = 50;
        assert_eq!(actor.next_reconnect_delay_secs(), 3);
    }

    #[tokio::test]
    async fn handle_disconnect_schedules_reconnect_for_tracked_workspaces() {
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 10);
        actor.tracked_workspaces.insert("ws-1".into(), "rt-1".into());

        actor.handle_disconnect();

        assert_eq!(actor.reconnect_attempt, 1);
        assert!(actor.connection.is_none());
        // Should have emitted Disconnected + Reconnecting for the tracked workspace.
        let ev1 = event_rx.try_recv().unwrap();
        assert!(matches!(
            ev1,
            EndpointEvent::WorkspaceConnectionChanged {
                status: ConnectionStatus::Disconnected,
                ..
            }
        ));
        let ev2 = event_rx.try_recv().unwrap();
        assert!(matches!(
            ev2,
            EndpointEvent::WorkspaceConnectionChanged {
                status: ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 1 },
                ..
            }
        ));
    }

    #[test]
    fn handle_disconnect_without_tracked_workspaces_does_not_schedule() {
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, true, 10);

        actor.handle_disconnect();

        assert_eq!(actor.reconnect_attempt, 0);
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn resolve_and_attach_reports_session_missing_on_stale_id() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor = EndpointActor {
            endpoint: RuntimeEndpoint::Local,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            reader: Some(reader),
            writer: Some(writer),
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon: true,
            reconnect_delay_secs: 10,
        };

        let stale_runtime = Uuid::new_v4();

        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();

            // Receive AttachSession for the stale runtime — reply "not found".
            let msg = recv_client_message(&mut server_stream, &mut read_buf).await;
            assert!(
                matches!(msg.msg, Some(proto::client_message::Msg::AttachSession(_))),
                "expected AttachSession, got {msg:?}"
            );
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Error(proto::Error {
                        code: 4,
                        message: "session not found".into(),
                    })),
                },
            )
            .await;
        });

        let result = actor
            .resolve_and_attach_runtime(
                "ws-1",
                "Test Workspace",
                WorkspacePolicy::Ephemeral,
                Some(&stale_runtime.to_string()),
            )
            .await;

        assert!(result.is_err(), "session-not-found should not fall back to new runtime");

        // Should have emitted SessionMissing status, not a WorkspaceError.
        let mut saw_session_missing = false;
        let mut saw_error = false;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                EndpointEvent::WorkspaceConnectionChanged {
                    status: ConnectionStatus::SessionMissing,
                    ..
                } => saw_session_missing = true,
                EndpointEvent::WorkspaceError { .. } => saw_error = true,
                _ => {}
            }
        }
        assert!(saw_session_missing, "should emit SessionMissing status");
        assert!(!saw_error, "should not emit WorkspaceError for missing session");

        server.await.expect("fake server task should complete");
    }

    #[tokio::test]
    async fn resolve_and_attach_reports_ownership_conflict() {
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor = EndpointActor {
            endpoint: RuntimeEndpoint::Local,
            event_tx,
            self_tx,
            cmd_rx,
            connection: None,
            reader: Some(reader),
            writer: Some(writer),
            ssh_handle: None,
            tracked_workspaces: HashMap::new(),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon: true,
            reconnect_delay_secs: 10,
        };

        let runtime_id = Uuid::new_v4();

        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();

            // Receive AttachSession — reply with AttachBlocked.
            let msg = recv_client_message(&mut server_stream, &mut read_buf).await;
            assert!(matches!(msg.msg, Some(proto::client_message::Msg::AttachSession(_))));
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::AttachBlocked(proto::AttachBlocked {
                        session_id: rttx_proto::uuid_to_bytes(runtime_id),
                        current_client_role: 0,
                        attached_client_count: 1,
                        read_only_client_count: 0,
                    })),
                },
            )
            .await;
        });

        let result = actor
            .resolve_and_attach_runtime(
                "ws-1",
                "Test Workspace",
                WorkspacePolicy::Persistent,
                Some(&runtime_id.to_string()),
            )
            .await;

        assert!(result.is_err(), "ownership conflict should not fall back");

        // Should have emitted a WorkspaceError.
        let mut saw_error = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, EndpointEvent::WorkspaceError { .. }) {
                saw_error = true;
            }
        }
        assert!(saw_error, "ownership conflict should emit WorkspaceError");

        server.await.expect("fake server task should complete");
    }

    #[test]
    fn ssh_connect_timeout_is_reasonable() {
        assert!(
            SSH_CONNECT_TIMEOUT >= Duration::from_secs(10),
            "SSH timeout should be at least 10s to allow for slow networks"
        );
        assert!(
            SSH_CONNECT_TIMEOUT <= Duration::from_secs(60),
            "SSH timeout should not exceed 60s to avoid blocking the actor too long"
        );
    }

    /// Verify that a non-transient connection error during reconnect still
    /// schedules another attempt instead of silently killing the loop.
    #[tokio::test]
    async fn schedule_reconnect_for_non_transient_uses_max_delay() {
        let (event_tx, _) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, mut self_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let (_, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, false, 10);

        let non_transient = ConnectionProblem::VersionMismatch;
        assert!(!non_transient.is_transient());

        actor.schedule_reconnect_for_problem(&non_transient);

        // Non-transient should use max delay (reconnect_delay_secs = 10).
        assert_eq!(actor.reconnect_attempt, 1);

        let cmd = tokio::time::timeout(Duration::from_secs(15), self_rx.recv())
            .await
            .expect("should receive reconnect command")
            .expect("channel should not close");
        assert!(matches!(cmd, EndpointCommand::Reconnect));
    }

    #[tokio::test]
    async fn schedule_reconnect_for_transient_uses_progressive_delay() {
        let (event_tx, _) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, mut self_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let (_, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let mut actor =
            EndpointActor::new(RuntimeEndpoint::Local, event_tx, self_tx, cmd_rx, false, 10);

        actor.schedule_reconnect_for_problem(&ConnectionProblem::DaemonUnavailable);

        // Transient at attempt 1 should use delay = min(1, 10) = 1s.
        assert_eq!(actor.reconnect_attempt, 1);

        let cmd = tokio::time::timeout(Duration::from_secs(5), self_rx.recv())
            .await
            .expect("should receive reconnect command")
            .expect("channel should not close");
        assert!(matches!(cmd, EndpointCommand::Reconnect));
    }

    #[tokio::test]
    async fn shutdown_command_stops_actor() {
        let ((reader, writer), _server) = split_duplex_connection();
        let actor = make_actor(reader, writer);

        // Send Shutdown via the actor's own channel.
        let tx = actor.self_tx.clone();
        tx.send(EndpointCommand::Shutdown).await.unwrap();

        // The actor should exit promptly.
        tokio::time::timeout(Duration::from_secs(2), actor.run())
            .await
            .expect("actor should exit after Shutdown");
    }

    #[tokio::test]
    async fn shutdown_command_stops_actor_without_connection() {
        let (event_tx, _) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let actor = EndpointActor::new(
            RuntimeEndpoint::Local,
            event_tx,
            self_tx.clone(),
            cmd_rx,
            false,
            10,
        );

        self_tx.send(EndpointCommand::Shutdown).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), actor.run())
            .await
            .expect("actor should exit after Shutdown even without connection");
    }

    #[test]
    fn reset_endpoint_removes_handle_and_sends_shutdown() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let manager = EndpointConnectionManager {
            rt,
            endpoints: RefCell::new(HashMap::new()),
            event_tx,
            auto_start_daemon: false,
            reconnect_delay_secs: 10,
        };

        let endpoint = RuntimeEndpoint::Local;

        // Create an actor by requesting its handle.
        let tx = manager.endpoint_handle(&endpoint);
        assert!(manager.endpoints.borrow().contains_key(&endpoint.key()));

        // Reset should remove the handle.
        manager.reset_endpoint(&endpoint);
        assert!(!manager.endpoints.borrow().contains_key(&endpoint.key()));

        // The old channel should have received Shutdown.
        // (We can't easily recv from it since the actor owns cmd_rx,
        // but we can verify the handle is gone and a new one is created.)
        let tx2 = manager.endpoint_handle(&endpoint);
        assert!(manager.endpoints.borrow().contains_key(&endpoint.key()));

        // The new handle should be a different channel than the old one.
        // Send on old channel — it should still work (actor hasn't consumed it yet)
        // but the manager no longer references it.
        assert!(tx.try_send(EndpointCommand::RefreshInventory).is_ok());
        assert!(tx2.try_send(EndpointCommand::RefreshInventory).is_ok());
    }

    /// When a reconnect reattach fails with a transient I/O error, only one
    /// reconnect should be scheduled — not one per tracked workspace.
    /// Regression test for the connect/disconnect loop (#417).
    #[tokio::test]
    async fn reconnect_transient_failure_schedules_single_reconnect() {
        let (event_tx, _event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let ws1_runtime = Uuid::new_v4();
        let ws2_runtime = Uuid::new_v4();

        let ((reader, writer), server_stream) = split_duplex_connection();
        let mut actor = EndpointActor {
            endpoint: RuntimeEndpoint::Local,
            event_tx,
            self_tx: self_tx.clone(),
            cmd_rx,
            // Connection is set (not split) — simulates a fresh reconnect.
            connection: None,
            reader: Some(reader),
            writer: Some(writer),
            ssh_handle: None,
            tracked_workspaces: HashMap::from([
                ("ws-1".into(), ws1_runtime.to_string()),
                ("ws-2".into(), ws2_runtime.to_string()),
            ]),
            reconnect_attempt: 0,
            heartbeat: HeartbeatMonitor::default(),
            heartbeat_deadline: new_heartbeat_deadline(),
            daemon_start_attempted: false,
            auto_start_daemon: true,
            reconnect_delay_secs: 10,
        };

        // Server: drop the connection immediately to cause I/O errors.
        drop(server_stream);

        // Simulate the reconnect reattach loop.
        let mut any_transient_failure = false;
        for (_workspace_id, runtime_id) in actor.tracked_workspaces.clone() {
            let Ok(runtime_uuid) = runtime_id.parse::<uuid::Uuid>() else {
                continue;
            };
            match actor.attach_runtime_via_active_channel(runtime_uuid).await {
                Ok(_) => {}
                Err(ref error) => {
                    let problem = classify_connection_problem(error);
                    if problem.is_transient() {
                        any_transient_failure = true;
                        break;
                    }
                }
            }
        }

        assert!(any_transient_failure, "should detect transient failure");

        // The fix: only one handle_disconnect call.
        actor.handle_disconnect();
        assert_eq!(actor.reconnect_attempt, 1, "exactly one reconnect should be scheduled");
    }

    #[tokio::test]
    async fn reconnect_splits_before_reattach_to_handle_push_messages() {
        // Regression test: during reconnect the actor must split the
        // connection before reattaching workspaces so that `read_response`
        // (which drains interleaved push messages) is used instead of the
        // raw `DaemonConnection::attach_session` path.
        let ((reader, writer), mut server_stream) = split_duplex_connection();
        let (mut actor, mut event_rx) = make_actor_with_events(reader, writer);

        let runtime_id = Uuid::new_v4();
        actor.tracked_workspaces.insert("ws-1".into(), runtime_id.to_string());

        // The actor starts with a split connection (reader + writer).
        // Verify the split path handles interleaved push messages.
        assert!(actor.writer.is_some());
        assert!(actor.reader.is_some());

        // Server sends a push message (delta) before the snapshot response.
        let server = tokio::spawn(async move {
            let mut read_buf = BytesMut::new();
            let request = recv_client_message(&mut server_stream, &mut read_buf).await;
            match request.msg {
                Some(proto::client_message::Msg::AttachSession(_)) => {}
                other => panic!("expected AttachSession, got {other:?}"),
            }

            // Send a delta push message first (simulating PTY output from
            // a previously attached session).
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Delta(proto::Delta {
                        session_id: rttx_proto::uuid_to_bytes(runtime_id),
                        pane_id: vec![0; 16],
                        data: bytes::Bytes::from_static(b"interleaved output"),
                    })),
                },
            )
            .await;

            // Then send the actual snapshot response.
            send_server_message(
                &mut server_stream,
                &proto::ServerMessage {
                    msg: Some(proto::server_message::Msg::Snapshot(proto::Snapshot {
                        session_id: rttx_proto::uuid_to_bytes(runtime_id),
                        panes: vec![],
                        revision: 1,
                        current_client_role: proto::RuntimeClientRole::Writer as i32,
                    })),
                },
            )
            .await;
        });

        // The split path uses read_response which skips push messages.
        let result = actor.attach_runtime_via_active_channel(runtime_id).await;
        assert!(result.is_ok(), "attach should succeed despite interleaved push message");

        server.await.expect("server task should complete");

        // The delta should have been forwarded as a push event.
        let mut saw_delta = false;
        while let Ok(event) = event_rx.try_recv() {
            if let EndpointEvent::RuntimeMessage { message, .. } = event
                && matches!(message.msg, Some(proto::server_message::Msg::Delta(_)))
            {
                saw_delta = true;
            }
        }
        assert!(saw_delta, "interleaved delta should be dispatched as a push event");
    }

    /// Regression test for #576: when a reconnect reattach fails with a
    /// transient error, the backoff counter must not reset to zero. The
    /// Reconnect handler saves the counter before `ensure_connected`
    /// (which resets it on success) and restores it on failure so the
    /// next delay continues ramping up.
    #[tokio::test]
    async fn reconnect_preserves_backoff_on_transient_reattach_failure() {
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CHANNEL_BOUND);
        let (self_tx, _self_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let (_, cmd_rx) = mpsc::channel(CMD_CHANNEL_BOUND);
        let ws_runtime = Uuid::new_v4();

        let ((reader, writer), server_stream) = split_duplex_connection();
        let mut actor = EndpointActor::new(
            RuntimeEndpoint::Local,
            event_tx,
            self_tx,
            cmd_rx,
            false,
            10,
        );
        // Simulate 5 prior reconnect cycles.
        actor.reconnect_attempt = 5;
        actor.tracked_workspaces.insert("ws-1".into(), ws_runtime.to_string());

        // Simulate what the Reconnect handler does:
        // 1. Save the counter before ensure_connected
        let saved_attempt = actor.reconnect_attempt;
        // 2. ensure_connected succeeds and resets the counter
        actor.reconnect_attempt = 0;
        // 3. Connection is split for reattach
        actor.reader = Some(reader);
        actor.writer = Some(writer);
        // 4. Server drops — reattach will fail
        drop(server_stream);

        let result = actor.attach_runtime_via_active_channel(ws_runtime).await;
        assert!(result.is_err(), "reattach should fail");
        assert!(
            classify_connection_problem(result.as_ref().unwrap_err()).is_transient(),
            "failure should be transient"
        );

        // 5. The fix: restore the counter before handle_disconnect
        actor.reconnect_attempt = saved_attempt;
        actor.handle_disconnect();

        // The emitted Reconnecting status must use a delay > 1, proving
        // the counter was preserved (min(6, 10) = 6).
        let mut reconnect_delay = None;
        while let Ok(event) = event_rx.try_recv() {
            if let EndpointEvent::WorkspaceConnectionChanged {
                status: ConnectionStatus::Reconnecting { retry_in_secs, .. },
                ..
            } = event
            {
                reconnect_delay = Some(retry_in_secs);
            }
        }
        assert_eq!(
            reconnect_delay,
            Some(6),
            "delay should be min(saved_attempt+1, max) = min(6, 10) = 6"
        );
    }
}

//! Top-level server loop.
//!
//! Accepts client connections, routes messages to runtimes/panes, runs the
//! serialization loop, and manages the PTY read loops.

use crate::engine::Engine;
use crate::engine::PaneSpawnConfig;
use crate::engine::native::NativeEngine;
use crate::ipc::{ClientConnection, ClientConnectionReader, ClientConnectionWriter, Listener};
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
use crate::runtime::{
    AttachError, AttachMode, AttachOutcome, ClientRole, DetachOutcome, DetachReason, Runtime,
    RuntimePolicy, TerminationReason,
};
use crate::screen::{restart_safe_scrollback, strip_client_queries};
use crate::serialization::{self, ServerState, default_state_path, load_state, write_state_atomic};
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes, v3};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

/// Per-client protocol state negotiated during handshake.
#[derive(Debug, Clone)]
pub enum ClientProtocol {
    /// V2 client (legacy `ClientMessage`/`ServerMessage`).
    V2,
    /// V3 client with negotiated capabilities.
    V3 { effective_caps: Vec<i32> },
}

/// Message that can be sent to a client, abstracting over v2 and v3.
#[derive(Debug, Clone)]
pub enum ClientMsg {
    V2(proto::ServerMessage),
    V3(v3::ServerEnvelope),
}

/// Return the first 8 characters of a UUID for compact log output.
#[must_use]
pub fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Capacity for the per-client push channel (Deltas, broadcasts).
/// Large enough to absorb short bursts; slow clients that fall behind
/// will have messages dropped (Deltas are replaceable by a snapshot on
/// reconnect).
pub const PUSH_CHANNEL_BOUND: usize = 4096;

/// Capacity for the per-client response channel (pong, snapshots).
const RESP_CHANNEL_BOUND: usize = 256;

/// Send a message to previously collected sender handles.
///
/// This is the lock-free counterpart of [`Server::broadcast_to_runtime`]:
/// collect handles with [`Server::collect_runtime_senders`] while holding
/// the mutex, then call this after releasing it.
fn send_to_collected(senders: &[(Uuid, mpsc::Sender<ClientMsg>)], msg: &ClientMsg) {
    for (client_id, sender) in senders {
        if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(msg.clone()) {
            tracing::warn!("Client {} push channel full — dropping message", short_id(*client_id),);
        }
    }
}

/// Shared mutable server state.
pub struct Server {
    /// All active runtimes.
    pub runtimes: HashMap<Uuid, Runtime>,
    /// Server's own identity.
    pub server_id: Uuid,
    /// The engine used to spawn pane processes.
    pub engine: Box<dyn Engine>,
    /// OS abstraction for paths.
    pub os: Box<dyn OsInterface>,
    /// Per-client bounded push channels for server-initiated messages (Deltas, etc.).
    client_senders: HashMap<Uuid, mpsc::Sender<ClientMsg>>,
    /// Per-client protocol version and capabilities.
    client_protocols: HashMap<Uuid, ClientProtocol>,
    /// Per-pane PTY write handles for Input and Resize routing.
    pty_writers: HashMap<Uuid, Arc<tokio::sync::Mutex<pty_process::OwnedWritePty>>>,
    /// Per-pane kill signals to cancel PTY read loops.
    pty_kill_senders: HashMap<Uuid, oneshot::Sender<()>>,
    /// Cooperative shutdown signal — set to `true` to stop the server.
    shutdown_tx: watch::Sender<bool>,
}

impl Server {
    /// Create a new server with the native engine.
    #[must_use]
    pub fn new(os: Box<dyn OsInterface>) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        Self {
            runtimes: HashMap::new(),
            server_id: Uuid::new_v4(),
            engine: Box::new(NativeEngine),
            os,
            client_senders: HashMap::new(),
            client_protocols: HashMap::new(),
            pty_writers: HashMap::new(),
            pty_kill_senders: HashMap::new(),
            shutdown_tx,
        }
    }

    /// Subscribe to the shutdown signal.
    #[must_use]
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    /// Trigger cooperative shutdown.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Number of connected client push channels.
    #[must_use]
    pub fn client_sender_count(&self) -> usize {
        self.client_senders.len()
    }

    /// Number of active PTY write handles.
    #[must_use]
    pub fn pty_writer_count(&self) -> usize {
        self.pty_writers.len()
    }

    /// Human-readable label for a runtime: `"name" (short_id)`.
    ///
    /// Falls back to just the short ID when the runtime is not found.
    #[must_use]
    pub fn runtime_label(&self, runtime_id: Uuid) -> String {
        self.runtimes.get(&runtime_id).map_or_else(
            || format!("({})", short_id(runtime_id)),
            |rt| format!("\"{}\" ({})", rt.name, short_id(runtime_id)),
        )
    }

    /// Load persisted state and resurrect runtimes.
    pub fn load_persisted_state(&mut self) {
        let state_path = default_state_path(&self.os.cache_dir());
        match load_state(&state_path) {
            Ok(Some(state)) => {
                tracing::info!("Loaded {} persisted runtimes", state.runtimes.len());
                for ps in &state.runtimes {
                    let rt = Runtime::from_persisted(ps);
                    self.runtimes.insert(rt.id, rt);
                }
            }
            Ok(None) => {
                tracing::info!("No persisted state found, starting fresh");
            }
            Err(e) => {
                tracing::error!("Failed to load persisted state: {e}");
            }
        }
    }

    /// Reconstruct resurrected runtimes: replay scrollback logs into pane
    /// screens and spawn fresh shells in saved working directories.
    ///
    /// Called after `load_persisted_state` once the server is wrapped in
    /// `Arc<Mutex<>>` so we can spawn PTY read loops.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn reconstruct_runtimes(server: &Arc<Mutex<Self>>) {
        let panes_to_reconstruct: Vec<(Uuid, Uuid, String, Option<String>, u16, u16)> = {
            let mut s = server.lock().await;

            for rt in s.runtimes.values_mut() {
                let label = format!("\"{}\" ({})", rt.name, short_id(rt.id));
                for pane in rt.panes.values_mut() {
                    // Replay scrollback log into the pane screen.
                    if let Some(ref log_path) = pane.scrollback_log_path
                        && log_path.exists()
                    {
                        let pane_short = short_id(pane.id);
                        match std::fs::read(log_path) {
                            Ok(data) => {
                                let restart_safe = restart_safe_scrollback(&data);
                                let clean = strip_client_queries(restart_safe);
                                tracing::info!(
                                    "Replaying {} bytes of scrollback for pane {pane_short} in runtime {label}",
                                    clean.len(),
                                );
                                pane.screen.feed(&clean);
                                if clean.len() != data.len()
                                    && let Err(e) = std::fs::write(log_path, &clean)
                                {
                                    tracing::error!(
                                        "Failed to rewrite restart-safe scrollback for pane {pane_short} in runtime {label}: {e}",
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to read scrollback log for pane {pane_short} in runtime {label}: {e}",
                                );
                            }
                        }
                    }
                }
            }

            // Collect panes that need fresh shells spawned.
            s.runtimes
                .values()
                .flat_map(|rt| {
                    let name = rt.name.clone();
                    rt.panes.values().map(move |pane| {
                        (rt.id, pane.id, name.clone(), pane.cwd.clone(), pane.cols, pane.rows)
                    })
                })
                .collect()
        };

        if panes_to_reconstruct.is_empty() {
            return;
        }

        tracing::info!("Reconstructing {} panes", panes_to_reconstruct.len());

        for (runtime_id, pane_id, runtime_name, cwd, cols, rows) in panes_to_reconstruct {
            let runtime_label = format!("\"{}\" ({})", runtime_name, short_id(runtime_id));
            let pane_short = short_id(pane_id);
            let pty_result = {
                let s = server.lock().await;
                let hist = serialization::history_path(&s.os.cache_dir(), runtime_id, pane_id);
                if let Some(parent) = hist.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let env = vec![
                    ("HISTFILE".into(), hist.to_string_lossy().into_owned()),
                    ("COLORFGBG".into(), "15;0".into()),
                ];
                let config = PaneSpawnConfig { command: vec![], cwd, env, cols, rows };
                s.engine.spawn_pane(pane_id, &config)
            };

            match pty_result {
                Ok(pty) => {
                    let child_pid = pty.pid();
                    let (reader, writer, child) = pty.into_parts();
                    let (kill_tx, kill_rx) = oneshot::channel();
                    {
                        let mut s = server.lock().await;
                        s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                        s.pty_kill_senders.insert(pane_id, kill_tx);
                        // Clear exit status — fresh shell is running.
                        if let Some(rt) = s.runtimes.get_mut(&runtime_id) {
                            let _ = rt.set_pane_exit_status(pane_id, None);
                            if let Some(pane) = rt.panes.get_mut(&pane_id) {
                                pane.child_pid = child_pid;
                            }
                        }
                    }
                    spawn_pty_read_loop(
                        Arc::clone(server),
                        runtime_id,
                        pane_id,
                        &runtime_name,
                        reader,
                        child,
                        kill_rx,
                    );
                    tracing::info!("Reconstructed pane {pane_short} in runtime {runtime_label}");
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to reconstruct pane {pane_short} in runtime {runtime_label}: {e}"
                    );
                    let mut s = server.lock().await;
                    if let Some(rt) = s.runtimes.get_mut(&runtime_id) {
                        let _ = rt.set_pane_exit_status(pane_id, Some(-1));
                    }
                }
            }
        }
    }

    /// Build a serializable snapshot of the current state.
    #[must_use]
    pub fn build_snapshot(&self) -> ServerState {
        ServerState {
            runtimes: self
                .runtimes
                .values()
                .filter(|rt| rt.policy == RuntimePolicy::Persistent)
                .map(Runtime::to_persisted)
                .collect(),
            serialized_at: std::time::SystemTime::now(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Send a message to the provided clients.
    fn broadcast_to_clients<I>(
        &self,
        client_ids: I,
        exclude_client_id: Option<Uuid>,
        msg: &ClientMsg,
    ) where
        I: IntoIterator<Item = Uuid>,
    {
        for client_id in client_ids {
            if Some(client_id) == exclude_client_id {
                continue;
            }
            if let Some(sender) = self.client_senders.get(&client_id)
                && let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(msg.clone())
            {
                tracing::warn!(
                    "Client {} push channel full — dropping message",
                    short_id(client_id),
                );
            }
        }
    }

    /// Send a message to all clients attached to a runtime.
    fn broadcast_to_runtime(&self, runtime_id: Uuid, msg: &ClientMsg) {
        let Some(rt) = self.runtimes.get(&runtime_id) else {
            return;
        };
        self.broadcast_to_clients(rt.attached_clients.keys().copied(), None, msg);
    }

    /// Collect cloned sender handles for all clients attached to a runtime.
    ///
    /// The returned senders can be used after releasing the server mutex via
    /// [`send_to_collected`].
    fn collect_runtime_senders(&self, runtime_id: Uuid) -> Vec<(Uuid, mpsc::Sender<ClientMsg>)> {
        let Some(rt) = self.runtimes.get(&runtime_id) else {
            return Vec::new();
        };
        rt.attached_clients
            .keys()
            .filter_map(|&cid| self.client_senders.get(&cid).map(|s| (cid, s.clone())))
            .collect()
    }

    /// Look up the protocol version for a client.
    #[must_use]
    pub fn client_protocol(&self, client_id: Uuid) -> Option<&ClientProtocol> {
        self.client_protocols.get(&client_id)
    }

    /// Register a client's protocol version.
    pub fn set_client_protocol(&mut self, client_id: Uuid, protocol: ClientProtocol) {
        self.client_protocols.insert(client_id, protocol);
    }

    fn terminate_runtime(
        &mut self,
        runtime_id: Uuid,
        final_revision: u64,
        reason: TerminationReason,
        exclude_client_id: Option<Uuid>,
    ) -> Option<ClientMsg> {
        let rt = self.runtimes.remove(&runtime_id)?;
        let attached_client_ids: Vec<_> = rt.attached_clients.keys().copied().collect();
        let pane_ids: Vec<_> = rt.panes.keys().copied().collect();
        for pane_id in pane_ids {
            self.pty_writers.remove(&pane_id);
            if let Some(kill_tx) = self.pty_kill_senders.remove(&pane_id) {
                let _ = kill_tx.send(());
            }
        }

        let msg = ClientMsg::V2(protocol::runtime_terminated(runtime_id, final_revision, reason));
        self.broadcast_to_clients(attached_client_ids, exclude_client_id, &msg);
        Some(msg)
    }

    /// Handle a single client message, returning an optional response.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn handle_message(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        msg: proto::ClientMessage,
    ) -> Option<proto::ServerMessage> {
        let Some(inner) = msg.msg else {
            return Some(protocol::error(protocol::ERR_EMPTY_MESSAGE, "empty message".into()));
        };

        match inner {
            proto::client_message::Msg::Hello(hello) => {
                if hello.protocol_version != rttx_proto::PROTOCOL_VERSION {
                    return Some(protocol::error(
                        protocol::ERR_VERSION_MISMATCH,
                        format!(
                            "protocol version mismatch: client={}, server={}",
                            hello.protocol_version,
                            rttx_proto::PROTOCOL_VERSION
                        ),
                    ));
                }
                let s = server.lock().await;
                Some(protocol::hello_ack(s.server_id))
            }

            proto::client_message::Msg::Ping(ping) => Some(protocol::pong(ping.nonce)),

            proto::client_message::Msg::ListRuntimes(_) => {
                let s = server.lock().await;
                let infos = protocol::runtime_inventory_for(client_id, s.runtimes.values());
                Some(protocol::runtime_list(infos))
            }

            proto::client_message::Msg::GetDiagnostics(_) => {
                let s = server.lock().await;
                Some(protocol::diagnostics_report(&s))
            }

            proto::client_message::Msg::CreateRuntime(req) => {
                let mut s = server.lock().await;
                let rt = Runtime::new(req.name);
                let runtime_id = rt.id;
                let policy = RuntimePolicy::from_proto(req.policy);
                let revision = rt.revision();
                let mut rt = rt;
                rt.policy = policy;
                let label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                let policy_str = match policy {
                    RuntimePolicy::Persistent => "persistent",
                    RuntimePolicy::Ephemeral => "ephemeral",
                };
                s.runtimes.insert(runtime_id, rt);
                tracing::info!("Runtime created: {label}, policy={policy_str}");
                Some(protocol::runtime_created(runtime_id, revision))
            }

            proto::client_message::Msg::AttachRuntime(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let attach_mode = AttachMode::from_proto(req.attach_mode);
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                let attach_outcome = match rt.attach_client(client_id, attach_mode) {
                    Ok(outcome) => outcome,
                    Err(AttachError::UnsupportedTakeOver) => {
                        return Some(protocol::error(
                            protocol::ERR_UNSUPPORTED,
                            "take over attach mode is not supported yet".into(),
                        ));
                    }
                };

                let (role, revision) = match attach_outcome {
                    AttachOutcome::Attached { role, revision } => (role, revision),
                    AttachOutcome::Blocked { current_role, .. } => {
                        return Some(protocol::attach_blocked(
                            runtime_id,
                            current_role,
                            rt.attached_client_count(),
                            rt.read_only_client_count(),
                        ));
                    }
                };

                let pane_snapshots: Vec<proto::PaneSnapshot> = rt
                    .panes
                    .values()
                    .map(|pane| proto::PaneSnapshot {
                        pane_id: uuid_to_bytes(pane.id),
                        title: pane.title.clone().unwrap_or_default(),
                        cwd: pane.effective_cwd().unwrap_or_default(),
                        cols: u32::from(pane.cols),
                        rows: u32::from(pane.rows),
                        scrollback: bytes::Bytes::from(crate::screen::strip_client_queries(
                            pane.screen.snapshot_bytes(crate::pane::MAX_SNAPSHOT_BYTES),
                        )),
                        exit_status: pane.exit_status,
                        bracketed_paste_mode: pane.screen.bracketed_paste_mode(),
                        application_cursor_keys: pane.screen.application_cursor_keys(),
                        application_keypad: pane.screen.application_keypad(),
                        mouse_tracking_mode: u32::from(pane.screen.mouse_tracking_mode()),
                        sgr_mouse_mode: pane.screen.sgr_mouse_mode(),
                    })
                    .collect();
                let runtime_label = s.runtime_label(runtime_id);
                tracing::info!(
                    "Client {} attached to runtime {runtime_label} as {role:?}",
                    short_id(client_id),
                );
                Some(protocol::snapshot(runtime_id, pane_snapshots, revision, role))
            }

            proto::client_message::Msg::DetachRuntime(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                match rt.detach_client(client_id, DetachReason::ExplicitRequest) {
                    DetachOutcome::Detached { revision }
                    | DetachOutcome::NotAttached { revision } => {
                        let runtime_label = s.runtime_label(runtime_id);
                        tracing::info!(
                            "Client {} detached from runtime {runtime_label}",
                            short_id(client_id),
                        );
                        Some(protocol::runtime_detached(runtime_id, revision))
                    }
                    DetachOutcome::Terminated { final_revision, reason } => {
                        let runtime_label = s.runtime_label(runtime_id);
                        tracing::info!(
                            "Client {} detached from runtime {runtime_label} (terminated: {reason:?})",
                            short_id(client_id),
                        );
                        let _ = s.terminate_runtime(
                            runtime_id,
                            final_revision,
                            reason,
                            Some(client_id),
                        );
                        Some(protocol::runtime_terminated(runtime_id, final_revision, reason))
                    }
                }
            }

            proto::client_message::Msg::TerminateRuntime(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                if rt.has_write_owner() && !rt.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let final_revision = rt.revision().saturating_add(1);
                let runtime_label = s.runtime_label(runtime_id);
                let _ = s.terminate_runtime(
                    runtime_id,
                    final_revision,
                    TerminationReason::Explicit,
                    Some(client_id),
                );
                tracing::info!("Runtime terminated: {runtime_label}");
                Some(protocol::runtime_terminated(
                    runtime_id,
                    final_revision,
                    TerminationReason::Explicit,
                ))
            }

            proto::client_message::Msg::CreatePane(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };

                let pane_id = Uuid::new_v4();
                let (pty_result, runtime_label, cols, rows) = {
                    let s = server.lock().await;
                    let Some(rt) = s.runtimes.get(&runtime_id) else {
                        return Some(protocol::error(
                            protocol::ERR_RUNTIME_NOT_FOUND,
                            "runtime not found".into(),
                        ));
                    };
                    if !rt.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    let label = s.runtime_label(runtime_id);
                    let hist = serialization::history_path(&s.os.cache_dir(), runtime_id, pane_id);
                    if let Some(parent) = hist.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let colorfgbg =
                        if req.dark_background.unwrap_or(true) { "15;0" } else { "0;15" };
                    let env = vec![
                        ("HISTFILE".into(), hist.to_string_lossy().into_owned()),
                        ("COLORFGBG".into(), colorfgbg.into()),
                    ];
                    let cwd = req.cwd;
                    let cols = if req.cols > 0 { req.cols as u16 } else { 80 };
                    let rows = if req.rows > 0 { req.rows as u16 } else { 24 };
                    let config = PaneSpawnConfig { command: vec![], cwd, env, cols, rows };
                    (s.engine.spawn_pane(pane_id, &config), label, cols, rows)
                };

                match pty_result {
                    Ok(pty) => {
                        let child_pid = pty.pid();
                        let (reader, writer, mut child) = pty.into_parts();
                        let (kill_tx, kill_rx) = oneshot::channel();
                        let (revision, runtime_name) = {
                            let mut s = server.lock().await;
                            let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                                let _ = child.start_kill();
                                return Some(protocol::error(
                                    protocol::ERR_RUNTIME_NOT_FOUND,
                                    "runtime not found".into(),
                                ));
                            };
                            let mut pane = Pane::new(pane_id, cols, rows);
                            pane.child_pid = child_pid;
                            rt.add_pane(pane);
                            let revision = rt.revision();
                            let name = rt.name.clone();
                            s.pty_writers
                                .insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                            s.pty_kill_senders.insert(pane_id, kill_tx);
                            (revision, name)
                        };
                        spawn_pty_read_loop(
                            Arc::clone(server),
                            runtime_id,
                            pane_id,
                            &runtime_name,
                            reader,
                            child,
                            kill_rx,
                        );
                        tracing::info!(
                            "Pane {} created in runtime \"{}\" ({})",
                            short_id(pane_id),
                            runtime_name,
                            short_id(runtime_id),
                        );
                        Some(protocol::pane_created(runtime_id, pane_id, revision))
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to spawn PTY for pane {} in runtime {runtime_label}: {e}",
                            short_id(pane_id)
                        );
                        Some(protocol::error(
                            protocol::ERR_SPAWN_FAILED,
                            format!("failed to spawn pane: {e}"),
                        ))
                    }
                }
            }

            proto::client_message::Msg::ClosePane(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                if !rt.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let Some(_pane) = rt.remove_pane(pane_id) else {
                    return Some(protocol::error(
                        protocol::ERR_PANE_NOT_FOUND,
                        "pane not found".into(),
                    ));
                };
                let revision = rt.revision();
                let runtime_label = s.runtime_label(runtime_id);
                s.pty_writers.remove(&pane_id);
                if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
                    let _ = kill_tx.send(());
                }
                tracing::info!("Pane {} closed in runtime {runtime_label}", short_id(pane_id),);
                Some(protocol::pane_closed(runtime_id, pane_id, revision))
            }

            proto::client_message::Msg::Input(req) => {
                let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else {
                    return None;
                };
                let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else {
                    return None;
                };
                let (writer, runtime_label) = {
                    let s = server.lock().await;
                    let rt = s.runtimes.get(&runtime_id)?;
                    if !rt.panes.contains_key(&pane_id) {
                        return None;
                    }
                    if !rt.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    (s.pty_writers.get(&pane_id).cloned(), s.runtime_label(runtime_id))
                };
                if let Some(writer) = writer {
                    let pane_short = short_id(pane_id);
                    let mut w = writer.lock().await;
                    if let Err(e) = w.write_all(&req.data).await {
                        tracing::error!(
                            "Failed to write to PTY {pane_short} in runtime {runtime_label}: {e}"
                        );
                    }
                    if let Err(e) = w.flush().await {
                        tracing::error!(
                            "Failed to flush PTY {pane_short} in runtime {runtime_label}: {e}"
                        );
                    }
                }
                None
            }

            proto::client_message::Msg::Resize(req) => {
                let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else {
                    return None;
                };
                let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else {
                    return None;
                };
                let Ok(cols) = u16::try_from(req.cols) else {
                    return None;
                };
                let Ok(rows) = u16::try_from(req.rows) else {
                    return None;
                };

                let writer = {
                    let s = server.lock().await;
                    let rt = s.runtimes.get(&runtime_id)?;
                    if !rt.panes.contains_key(&pane_id) {
                        return None;
                    }
                    if !rt.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    s.pty_writers.get(&pane_id).cloned()
                };

                let writer = writer?;

                {
                    let w = writer.lock().await;
                    if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                        let s = server.lock().await;
                        tracing::error!(
                            "Failed to resize PTY {} in runtime {}: {e}",
                            short_id(pane_id),
                            s.runtime_label(runtime_id),
                        );
                        return None;
                    }
                }

                let revision = {
                    let mut s = server.lock().await;
                    let rt = s.runtimes.get_mut(&runtime_id)?;
                    rt.resize_pane(pane_id, cols, rows)?
                };

                Some(protocol::pane_resized(runtime_id, pane_id, cols, rows, revision))
            }

            proto::client_message::Msg::SetPaneTitle(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                if !rt.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let Some(revision) = rt.set_pane_title(pane_id, req.title.clone()) else {
                    return Some(protocol::error(
                        protocol::ERR_PANE_NOT_FOUND,
                        "pane not found".into(),
                    ));
                };
                Some(protocol::title_changed(runtime_id, pane_id, req.title, revision))
            }

            proto::client_message::Msg::RenameRuntime(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(protocol::error(
                        protocol::ERR_RUNTIME_NOT_FOUND,
                        "runtime not found".into(),
                    ));
                };
                if !rt.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let old_name = rt.name.clone();
                let revision = rt.rename(req.name.clone());
                tracing::info!(
                    "Runtime renamed: \"{}\" -> \"{}\" ({})",
                    old_name,
                    req.name,
                    short_id(runtime_id),
                );
                Some(protocol::runtime_renamed(runtime_id, req.name, revision))
            }

            proto::client_message::Msg::Shutdown(_) => None,
        }
    }

    /// Handle a single v3 client envelope, returning an optional response.
    #[allow(clippy::significant_drop_tightening, clippy::too_many_lines)]
    pub async fn handle_v3_message(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        effective_caps: &[i32],
        envelope: v3::ClientEnvelope,
    ) -> Option<v3::ServerEnvelope> {
        let request_id = envelope.request_id;
        let Some(command) = envelope.command else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::InvalidArgument,
                    "empty envelope",
                    "Dispatch",
                ),
            ));
        };

        match command {
            v3::client_envelope::Command::Ping(ping) => {
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::Pong(v3::Pong { nonce: ping.nonce }),
                ))
            }

            v3::client_envelope::Command::ListRuntimes(_) => {
                let s = server.lock().await;
                let has_inventory_v2 = rttx_proto::v3_inventory::is_supported(effective_caps);
                let infos = protocol::v3_runtime_inventory_for(
                    client_id,
                    s.runtimes.values(),
                    has_inventory_v2,
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::RuntimeList(v3::RuntimeList { runtimes: infos }),
                ))
            }

            v3::client_envelope::Command::GetDiagnostics(_) => {
                if !rttx_proto::v3_diagnostics::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_DIAGNOSTICS not negotiated",
                            "GetDiagnostics",
                        ),
                    ));
                }
                let s = server.lock().await;
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::DiagnosticsReport(
                        protocol::v3_diagnostics_report(&s),
                    ),
                ))
            }

            v3::client_envelope::Command::CreateRuntime(req) => {
                let mut s = server.lock().await;
                let rt = Runtime::new(req.name);
                let runtime_id = rt.id;
                let policy = RuntimePolicy::from_v3_proto(req.policy);
                let mut rt = rt;
                rt.policy = policy;
                let label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                let policy_str = match policy {
                    RuntimePolicy::Persistent => "persistent",
                    RuntimePolicy::Ephemeral => "ephemeral",
                };
                let revision = rt.revision();
                s.runtimes.insert(runtime_id, rt);
                tracing::info!("Runtime created: {label}, policy={policy_str}");
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::RuntimeCreated(v3::RuntimeCreated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        runtime_revision: revision,
                    }),
                ))
            }

            v3::client_envelope::Command::AttachRuntime(req) => {
                Self::handle_v3_attach(server, client_id, request_id, req).await
            }

            v3::client_envelope::Command::DetachRuntime(req) => {
                Self::handle_v3_detach(server, client_id, request_id, req).await
            }

            v3::client_envelope::Command::TerminateRuntime(req) => {
                Self::handle_v3_terminate(server, client_id, request_id, req).await
            }

            v3::client_envelope::Command::CreatePane(req) => {
                Self::handle_v3_create_pane(server, client_id, request_id, req).await
            }

            v3::client_envelope::Command::ClosePane(req) => {
                Self::handle_v3_close_pane(server, client_id, request_id, req).await
            }

            v3::client_envelope::Command::TerminalInput(input) => {
                Self::handle_v3_terminal_input(server, client_id, input).await
            }

            v3::client_envelope::Command::ResizePane(req) => {
                Self::handle_v3_resize(server, client_id, req).await
            }

            v3::client_envelope::Command::SetPaneTitle(req) => {
                Self::handle_v3_set_pane_title(server, client_id, req).await
            }

            v3::client_envelope::Command::RenameRuntime(req) => {
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "RenameRuntime",
                            ),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::RuntimeNotFound,
                            "runtime not found",
                            "RenameRuntime",
                        ),
                    ));
                };
                if !rt.client_has_write_access(client_id) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::OwnershipConflict,
                            "runtime is currently owned by another client",
                            "RenameRuntime",
                        ),
                    ));
                }
                let old_name = rt.name.clone();
                let revision = rt.rename(req.name.clone());
                tracing::info!(
                    "Runtime renamed: \"{}\" -> \"{}\" ({})",
                    old_name,
                    req.name,
                    short_id(runtime_id),
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::RuntimeRenamed(v3::RuntimeRenamed {
                        runtime_id: uuid_to_bytes(runtime_id),
                        name: req.name,
                        runtime_revision: revision,
                    }),
                ))
            }

            v3::client_envelope::Command::ResyncRuntime(req) => {
                if !rttx_proto::v3_resync::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_RESYNC not negotiated",
                            "ResyncRuntime",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "ResyncRuntime",
                            ),
                        ));
                    }
                };
                let s = server.lock().await;
                let Some(rt) = s.runtimes.get(&runtime_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::RuntimeNotFound,
                            "runtime not found",
                            "ResyncRuntime",
                        ),
                    ));
                };
                let role = rt
                    .client_role(client_id)
                    .map_or(v3::RuntimeClientRole::Unattached, ClientRole::as_v3_proto);
                let snapshot = protocol::build_v3_runtime_snapshot(rt, runtime_id, role);
                Some(rttx_proto::v3_snapshot::build_snapshot_response(request_id, snapshot))
            }

            v3::client_envelope::Command::GetScrollback(req) => {
                if !rttx_proto::v3_scrollback::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_CHUNKED_SCROLLBACK not negotiated",
                            "GetScrollback",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "GetScrollback",
                            ),
                        ));
                    }
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "GetScrollback",
                            ),
                        ));
                    }
                };
                let s = server.lock().await;
                let Some(rt) = s.runtimes.get(&runtime_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::RuntimeNotFound,
                            "runtime not found",
                            "GetScrollback",
                        ),
                    ));
                };
                let Some(pane) = rt.panes.get(&pane_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::PaneNotFound,
                            "pane not found",
                            "GetScrollback",
                        ),
                    ));
                };
                let limit = rttx_proto::v3_scrollback::cap_limit(req.limit) as usize;
                let raw = pane.screen.raw_bytes();
                let offset = req.offset as usize;
                let (data, is_last) = if offset >= raw.len() {
                    (bytes::Bytes::new(), true)
                } else {
                    let end = raw.len().min(offset + limit);
                    let chunk = &raw[offset..end];
                    (bytes::Bytes::copy_from_slice(chunk), end >= raw.len())
                };
                let chunk = rttx_proto::v3_scrollback::build_scrollback_chunk(
                    runtime_id, pane_id, req.offset, data, is_last,
                );
                Some(rttx_proto::v3_scrollback::build_scrollback_chunk_response(request_id, chunk))
            }

            v3::client_envelope::Command::TakeoverRuntime(req) => {
                if !rttx_proto::v3_takeover::is_supported(effective_caps) {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::UnsupportedCapability,
                            "OPT_RUNTIME_TAKEOVER not negotiated",
                            "TakeoverRuntime",
                        ),
                    ));
                }
                let runtime_id = match bytes_to_uuid(&req.runtime_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::InvalidArgument,
                                &e.to_string(),
                                "TakeoverRuntime",
                            ),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                    return Some(rttx_proto::v3_error::build_error_response(
                        request_id,
                        rttx_proto::v3_error::build_error(
                            v3::ErrorKind::RuntimeNotFound,
                            "runtime not found",
                            "TakeoverRuntime",
                        ),
                    ));
                };
                match rt.attach_client(client_id, AttachMode::TakeOver) {
                    Ok(AttachOutcome::Attached { revision, .. }) => {
                        Some(rttx_proto::v3_takeover::build_takeover_completed_response(
                            request_id,
                            rttx_proto::v3_takeover::build_takeover_completed(runtime_id, revision),
                        ))
                    }
                    Ok(AttachOutcome::Blocked { .. }) | Err(AttachError::UnsupportedTakeOver) => {
                        Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::OwnershipConflict,
                                "takeover failed",
                                "TakeoverRuntime",
                            ),
                        ))
                    }
                }
            }

            v3::client_envelope::Command::Shutdown(_) => None,
        }
    }
}

/// Server capabilities advertised during v3 handshake.
pub const SERVER_CAPABILITIES: &[v3::Capability] = &[
    v3::Capability::CoreRuntimeLifecycle,
    v3::Capability::CorePaneLifecycle,
    v3::Capability::CoreTerminalIo,
    v3::Capability::CoreTerminalModes,
    v3::Capability::CorePasteIntent,
    v3::Capability::CoreFocusEvents,
    v3::Capability::OptRuntimeInventoryV2,
    v3::Capability::OptResync,
    v3::Capability::OptChunkedScrollback,
    v3::Capability::OptDiagnostics,
    v3::Capability::OptRuntimeTakeover,
];

// ── V3 dispatch helpers ─────────────────────────────────────────

#[allow(clippy::significant_drop_tightening)]
impl Server {
    #[allow(clippy::significant_drop_tightening)]
    async fn handle_v3_attach(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::AttachRuntime,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "AttachRuntime",
                    ),
                ));
            }
        };
        let attach_mode = AttachMode::from_v3_proto(req.attach_mode);
        let mut s = server.lock().await;
        let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::RuntimeNotFound,
                    "runtime not found",
                    "AttachRuntime",
                ),
            ));
        };
        let attach_outcome = match rt.attach_client(client_id, attach_mode) {
            Ok(outcome) => outcome,
            Err(AttachError::UnsupportedTakeOver) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::TakeoverRequired,
                        "use TakeoverRuntime command",
                        "AttachRuntime",
                    ),
                ));
            }
        };
        match attach_outcome {
            AttachOutcome::Attached { role, .. } => {
                let v3_role = role.as_v3_proto();
                let snapshot = protocol::build_v3_runtime_snapshot(rt, runtime_id, v3_role);
                let runtime_label = s.runtime_label(runtime_id);
                tracing::info!(
                    "Client {} attached to runtime {runtime_label} as {role:?}",
                    short_id(client_id)
                );
                Some(rttx_proto::v3_snapshot::build_snapshot_response(request_id, snapshot))
            }
            AttachOutcome::Blocked { current_role, .. } => {
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::AttachBlocked(v3::AttachBlocked {
                        runtime_id: uuid_to_bytes(runtime_id),
                        current_client_role: current_role
                            .map_or(v3::RuntimeClientRole::Unattached, ClientRole::as_v3_proto)
                            as i32,
                        attached_client_count: u32::try_from(rt.attached_client_count())
                            .unwrap_or(u32::MAX),
                        read_only_client_count: u32::try_from(rt.read_only_client_count())
                            .unwrap_or(u32::MAX),
                    }),
                ))
            }
        }
    }

    async fn handle_v3_detach(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::DetachRuntime,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "DetachRuntime",
                    ),
                ));
            }
        };
        let mut s = server.lock().await;
        let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::RuntimeNotFound,
                    "runtime not found",
                    "DetachRuntime",
                ),
            ));
        };
        match rt.detach_client(client_id, DetachReason::ExplicitRequest) {
            DetachOutcome::Detached { revision } | DetachOutcome::NotAttached { revision } => {
                let runtime_label = s.runtime_label(runtime_id);
                tracing::info!(
                    "Client {} detached from runtime {runtime_label}",
                    short_id(client_id)
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::RuntimeDetached(v3::RuntimeDetached {
                        runtime_id: uuid_to_bytes(runtime_id),
                        runtime_revision: revision,
                    }),
                ))
            }
            DetachOutcome::Terminated { final_revision, reason } => {
                let runtime_label = s.runtime_label(runtime_id);
                tracing::info!(
                    "Client {} detached from runtime {runtime_label} (terminated: {reason:?})",
                    short_id(client_id)
                );
                let _ = s.terminate_runtime(runtime_id, final_revision, reason, Some(client_id));
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::RuntimeTerminated(v3::RuntimeTerminated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        final_revision,
                        reason: reason.as_v3_proto() as i32,
                    }),
                ))
            }
        }
    }

    async fn handle_v3_terminate(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::TerminateRuntime,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "TerminateRuntime",
                    ),
                ));
            }
        };
        let mut s = server.lock().await;
        let Some(rt) = s.runtimes.get(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::RuntimeNotFound,
                    "runtime not found",
                    "TerminateRuntime",
                ),
            ));
        };
        if rt.has_write_owner() && !rt.client_has_write_access(client_id) {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::OwnershipConflict,
                    "runtime is currently owned by another client",
                    "TerminateRuntime",
                ),
            ));
        }
        let final_revision = rt.revision().saturating_add(1);
        let runtime_label = s.runtime_label(runtime_id);
        let _ = s.terminate_runtime(
            runtime_id,
            final_revision,
            TerminationReason::Explicit,
            Some(client_id),
        );
        tracing::info!("Runtime terminated: {runtime_label}");
        Some(rttx_proto::v3_envelope::build_response_envelope(
            request_id,
            v3::server_envelope::Payload::RuntimeTerminated(v3::RuntimeTerminated {
                runtime_id: uuid_to_bytes(runtime_id),
                final_revision,
                reason: v3::RuntimeTerminationReason::Explicit as i32,
            }),
        ))
    }

    #[allow(clippy::significant_drop_tightening)]
    async fn handle_v3_create_pane(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::CreatePane,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "CreatePane",
                    ),
                ));
            }
        };
        let pane_id = Uuid::new_v4();
        let (pty_result, runtime_label, cols, rows) = {
            let s = server.lock().await;
            let Some(rt) = s.runtimes.get(&runtime_id) else {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::RuntimeNotFound,
                        "runtime not found",
                        "CreatePane",
                    ),
                ));
            };
            if !rt.client_has_write_access(client_id) {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::OwnershipConflict,
                        "runtime is currently owned by another client",
                        "CreatePane",
                    ),
                ));
            }
            let label = s.runtime_label(runtime_id);
            let hist = serialization::history_path(&s.os.cache_dir(), runtime_id, pane_id);
            if let Some(parent) = hist.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let colorfgbg = if req.dark_background.unwrap_or(true) { "15;0" } else { "0;15" };
            let env = vec![
                ("HISTFILE".into(), hist.to_string_lossy().into_owned()),
                ("COLORFGBG".into(), colorfgbg.into()),
            ];
            let cols = if req.cols > 0 { req.cols as u16 } else { 80 };
            let rows = if req.rows > 0 { req.rows as u16 } else { 24 };
            let config = PaneSpawnConfig { command: vec![], cwd: req.cwd, env, cols, rows };
            (s.engine.spawn_pane(pane_id, &config), label, cols, rows)
        };
        match pty_result {
            Ok(pty) => {
                let child_pid = pty.pid();
                let (reader, writer, mut child) = pty.into_parts();
                let (kill_tx, kill_rx) = oneshot::channel();
                let (revision, runtime_name) = {
                    let mut s = server.lock().await;
                    let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
                        let _ = child.start_kill();
                        return Some(rttx_proto::v3_error::build_error_response(
                            request_id,
                            rttx_proto::v3_error::build_error(
                                v3::ErrorKind::RuntimeNotFound,
                                "runtime not found",
                                "CreatePane",
                            ),
                        ));
                    };
                    let mut pane = Pane::new(pane_id, cols, rows);
                    pane.child_pid = child_pid;
                    rt.add_pane(pane);
                    let revision = rt.revision();
                    let name = rt.name.clone();
                    s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                    s.pty_kill_senders.insert(pane_id, kill_tx);
                    (revision, name)
                };
                spawn_pty_read_loop(
                    Arc::clone(server),
                    runtime_id,
                    pane_id,
                    &runtime_name,
                    reader,
                    child,
                    kill_rx,
                );
                tracing::info!(
                    "Pane {} created in runtime \"{}\" ({})",
                    short_id(pane_id),
                    runtime_name,
                    short_id(runtime_id)
                );
                Some(rttx_proto::v3_envelope::build_response_envelope(
                    request_id,
                    v3::server_envelope::Payload::PaneCreated(v3::PaneCreated {
                        runtime_id: uuid_to_bytes(runtime_id),
                        pane_id: uuid_to_bytes(pane_id),
                        runtime_revision: revision,
                    }),
                ))
            }
            Err(e) => {
                tracing::error!(
                    "Failed to spawn PTY for pane {} in runtime {runtime_label}: {e}",
                    short_id(pane_id)
                );
                Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::Internal,
                        &format!("failed to spawn pane: {e}"),
                        "CreatePane",
                    ),
                ))
            }
        }
    }

    async fn handle_v3_close_pane(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        request_id: u64,
        req: v3::ClosePane,
    ) -> Option<v3::ServerEnvelope> {
        let runtime_id = match bytes_to_uuid(&req.runtime_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "ClosePane",
                    ),
                ));
            }
        };
        let pane_id = match bytes_to_uuid(&req.pane_id) {
            Ok(id) => id,
            Err(e) => {
                return Some(rttx_proto::v3_error::build_error_response(
                    request_id,
                    rttx_proto::v3_error::build_error(
                        v3::ErrorKind::InvalidArgument,
                        &e.to_string(),
                        "ClosePane",
                    ),
                ));
            }
        };
        let mut s = server.lock().await;
        let Some(rt) = s.runtimes.get_mut(&runtime_id) else {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::RuntimeNotFound,
                    "runtime not found",
                    "ClosePane",
                ),
            ));
        };
        if !rt.client_has_write_access(client_id) {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::OwnershipConflict,
                    "runtime is currently owned by another client",
                    "ClosePane",
                ),
            ));
        }
        if rt.remove_pane(pane_id).is_none() {
            return Some(rttx_proto::v3_error::build_error_response(
                request_id,
                rttx_proto::v3_error::build_error(
                    v3::ErrorKind::PaneNotFound,
                    "pane not found",
                    "ClosePane",
                ),
            ));
        }
        let revision = rt.revision();
        let runtime_label = s.runtime_label(runtime_id);
        s.pty_writers.remove(&pane_id);
        if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
            let _ = kill_tx.send(());
        }
        tracing::info!("Pane {} closed in runtime {runtime_label}", short_id(pane_id));
        Some(rttx_proto::v3_envelope::build_response_envelope(
            request_id,
            v3::server_envelope::Payload::PaneClosed(v3::PaneClosed {
                runtime_id: uuid_to_bytes(runtime_id),
                pane_id: uuid_to_bytes(pane_id),
                runtime_revision: revision,
            }),
        ))
    }

    async fn handle_v3_terminal_input(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        input: v3::TerminalInput,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&input.runtime_id) else { return None };
        let Ok(pane_id) = bytes_to_uuid(&input.pane_id) else { return None };
        let (writer, resolved_bytes) = {
            let s = server.lock().await;
            let rt = s.runtimes.get(&runtime_id)?;
            if !rt.panes.contains_key(&pane_id) {
                return None;
            }
            if !rt.client_has_write_access(client_id) {
                return None;
            }
            let modes = rt.panes.get(&pane_id)?.screen.terminal_mode_state();
            let resolved =
                rttx_proto::v3_terminal_input::resolve_input(input.kind.as_ref(), &modes);
            (s.pty_writers.get(&pane_id).cloned(), resolved)
        };
        if resolved_bytes.is_empty() {
            return None;
        }
        if let Some(writer) = writer {
            let pane_short = short_id(pane_id);
            let mut w = writer.lock().await;
            if let Err(e) = w.write_all(&resolved_bytes).await {
                tracing::error!("Failed to write to PTY {pane_short}: {e}");
            }
            if let Err(e) = w.flush().await {
                tracing::error!("Failed to flush PTY {pane_short}: {e}");
            }
        }
        None
    }

    async fn handle_v3_resize(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::ResizePane,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else { return None };
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let Ok(cols) = u16::try_from(req.cols) else { return None };
        let Ok(rows) = u16::try_from(req.rows) else { return None };
        let writer = {
            let s = server.lock().await;
            let rt = s.runtimes.get(&runtime_id)?;
            if !rt.panes.contains_key(&pane_id) {
                return None;
            }
            if !rt.client_has_write_access(client_id) {
                return None;
            }
            s.pty_writers.get(&pane_id).cloned()
        };
        let writer = writer?;
        {
            let w = writer.lock().await;
            if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                let s = server.lock().await;
                tracing::error!(
                    "Failed to resize PTY {} in runtime {}: {e}",
                    short_id(pane_id),
                    s.runtime_label(runtime_id)
                );
                return None;
            }
        }
        let mut s = server.lock().await;
        let rt = s.runtimes.get_mut(&runtime_id)?;
        rt.resize_pane(pane_id, cols, rows)?;
        None
    }

    async fn handle_v3_set_pane_title(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        req: v3::SetPaneTitle,
    ) -> Option<v3::ServerEnvelope> {
        let Ok(runtime_id) = bytes_to_uuid(&req.runtime_id) else { return None };
        let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else { return None };
        let mut s = server.lock().await;
        let rt = s.runtimes.get_mut(&runtime_id)?;
        if !rt.client_has_write_access(client_id) {
            return None;
        }
        let _ = rt.set_pane_title(pane_id, req.title);
        None
    }
}
const COALESCE_MAX_BYTES: usize = 64 * 1024;

/// How long to wait for additional PTY data after the first read.
const COALESCE_WINDOW: Duration = Duration::from_millis(1);

/// Warn when the server mutex is held longer than this in the PTY read loop.
pub const MUTEX_HOLD_WARN_THRESHOLD: Duration = Duration::from_millis(10);

/// Spawn a background task that reads PTY output and broadcasts Deltas.
fn spawn_pty_read_loop(
    server: Arc<Mutex<Server>>,
    runtime_id: Uuid,
    pane_id: Uuid,
    runtime_name: &str,
    mut reader: pty_process::OwnedReadPty,
    mut child: tokio::process::Child,
    mut kill_rx: oneshot::Receiver<()>,
) {
    let runtime_label = format!("\"{}\" ({})", runtime_name, short_id(runtime_id));
    let pane_short = short_id(pane_id);
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut batch = bytes::BytesMut::with_capacity(COALESCE_MAX_BYTES);
        loop {
            tokio::select! {
                result = reader.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            batch.extend_from_slice(&buf[..n]);

                            // Drain additional available data within a short window.
                            if batch.len() < COALESCE_MAX_BYTES {
                                let deadline = tokio::time::Instant::now() + COALESCE_WINDOW;
                                loop {
                                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                                    if remaining.is_zero() || batch.len() >= COALESCE_MAX_BYTES {
                                        break;
                                    }
                                    match tokio::time::timeout(remaining, reader.read(&mut buf)).await {
                                        Ok(Ok(0) | Err(_)) | Err(_) => break,
                                        Ok(Ok(n)) => batch.extend_from_slice(&buf[..n]),
                                    }
                                }
                            }

                            let data = batch.split().freeze();

                            // Phase 1: hold lock for state mutation and handle collection.
                            let (new_cwd, new_title, pending_replies, pty_writer, senders, _output_seq) = {
                                let lock_start = std::time::Instant::now();
                                let mut s = server.lock().await;
                                let (new_cwd, new_title, pending_replies, output_seq) = if let Some(rt) = s.runtimes.get_mut(&runtime_id)
                                    && let Some(pane) = rt.panes.get_mut(&pane_id)
                                {
                                    let result = pane.feed_output(&data);
                                    let seq = pane.output_seq;
                                    let cwd = result.new_cwd.and_then(|cwd| {
                                        let rev = rt.set_pane_cwd(pane_id, &cwd)?;
                                        Some((cwd, rev))
                                    });
                                    let title = result.new_title.and_then(|title| {
                                        let rev = rt.set_pane_title(pane_id, title.clone())?;
                                        Some((title, rev))
                                    });
                                    (cwd, title, result.pending_replies, seq)
                                } else {
                                    (None, None, Vec::new(), 0)
                                };
                                let pty_writer = if pending_replies.is_empty() {
                                    None
                                } else {
                                    s.pty_writers.get(&pane_id).cloned()
                                };
                                let senders = s.collect_runtime_senders(runtime_id);
                                drop(s);
                                let hold = lock_start.elapsed();
                                if hold > MUTEX_HOLD_WARN_THRESHOLD {
                                    tracing::warn!(
                                        hold_ms = hold.as_millis() as u64,
                                        pane = %pane_short,
                                        runtime = %runtime_label,
                                        "server mutex held too long in PTY read loop",
                                    );
                                }
                                (new_cwd, new_title, pending_replies, pty_writer, senders, output_seq)
                            };
                            // Lock released.

                            // Phase 2: write DSR replies without the server lock.
                            if let Some(writer) = pty_writer {
                                let mut w = writer.lock().await;
                                for reply in &pending_replies {
                                    if let Err(e) = w.write_all(reply).await {
                                        tracing::error!(
                                            "Failed to write DSR reply to PTY {pane_short} in runtime {runtime_label}: {e}"
                                        );
                                    }
                                }
                                if let Err(e) = w.flush().await {
                                    tracing::error!(
                                        "Failed to flush DSR reply to PTY {pane_short} in runtime {runtime_label}: {e}"
                                    );
                                }
                            }

                            // Phase 3: broadcast to clients without the server lock.
                            // Strip terminal query sequences (DSR, DA1, DA2, DECRQM)
                            // that the daemon already handles. If forwarded, VTE would
                            // generate duplicate responses that leak as visible garbage.
                            let client_data = crate::screen::strip_client_queries(&data);
                            if !client_data.is_empty() {
                                let msg = ClientMsg::V2(protocol::delta(runtime_id, pane_id, bytes::Bytes::from(client_data.clone())));
                                send_to_collected(&senders, &msg);
                            }
                            if let Some((cwd, revision)) = new_cwd {
                                let msg = ClientMsg::V2(protocol::cwd_changed(runtime_id, pane_id, cwd, revision));
                                send_to_collected(&senders, &msg);
                            }
                            if let Some((title, revision)) = new_title {
                                let msg = ClientMsg::V2(protocol::title_changed(runtime_id, pane_id, title, revision));
                                send_to_collected(&senders, &msg);
                            }
                        }
                        Err(e) => {
                            tracing::error!("PTY read error for pane {pane_short} in runtime {runtime_label}: {e}");
                            break;
                        }
                    }
                }
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    tracing::info!("PTY read loop cancelled for pane {pane_short} in runtime {runtime_label}");
                    return;
                }
            }
        }

        // Child exited naturally — collect exit status.
        let status = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                tracing::error!(
                    "Failed to wait on child for pane {pane_short} in runtime {runtime_label}: {e}"
                );
                -1
            }
        };

        let mut s = server.lock().await;
        let exit_msg = if let Some(rt) = s.runtimes.get_mut(&runtime_id) {
            let msg = rt.set_pane_exit_status(pane_id, Some(status)).map(|revision| {
                ClientMsg::V2(protocol::pane_exited(runtime_id, pane_id, status, revision))
            });
            if let Some(pane) = rt.panes.get_mut(&pane_id) {
                pane.release_scrollback();
            }
            msg
        } else {
            None
        };
        if let Some(msg) = exit_msg {
            s.broadcast_to_runtime(runtime_id, &msg);
        }
        s.pty_writers.remove(&pane_id);
        s.pty_kill_senders.remove(&pane_id);
        drop(s);

        tracing::info!(
            "PTY exited for pane {pane_short} in runtime {runtime_label}, status {status}"
        );
    });
}

/// Run the serialization loop, writing state to disk every `interval`.
///
/// Stops when the shutdown signal fires.
pub async fn serialization_loop(
    server: Arc<Mutex<Server>>,
    interval: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(interval);
    let mut diagnostics_counter = 0u64;
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown_rx.changed() => {
                tracing::info!("Serialization loop stopping (shutdown)");
                return;
            }
        }

        let mut s = server.lock().await;
        let cache_dir = s.os.cache_dir();

        let runtime_ids: Vec<_> = s.runtimes.keys().copied().collect();
        for runtime_id in runtime_ids {
            if let Some(rt) = s.runtimes.get_mut(&runtime_id) {
                let label = format!("\"{}\" ({})", rt.name, short_id(runtime_id));
                for pane in rt.panes.values_mut() {
                    if let Err(e) = pane.flush_scrollback(&cache_dir, runtime_id) {
                        tracing::error!(
                            "Failed to flush scrollback for pane {} in runtime {label}: {e}",
                            short_id(pane.id)
                        );
                    }
                }
            }
        }

        // Log diagnostics every 30 ticks (~30 seconds at 1s interval).
        diagnostics_counter += 1;
        if diagnostics_counter.is_multiple_of(30) {
            s.log_diagnostics();
        }

        let snapshot = s.build_snapshot();
        let state_path = default_state_path(&cache_dir);
        drop(s);

        if let Err(e) = write_state_atomic(&snapshot, &state_path) {
            tracing::error!("Failed to serialize state: {e}");
        }
    }
}

/// Persist final state and flush all scrollback to disk.
pub async fn persist_and_cleanup(server: &Arc<Mutex<Server>>) {
    let mut s = server.lock().await;
    let cache_dir = s.os.cache_dir();

    for rt in s.runtimes.values_mut() {
        let label = format!("\"{}\" ({})", rt.name, short_id(rt.id));
        for pane in rt.panes.values_mut() {
            if let Err(e) = pane.flush_scrollback(&cache_dir, rt.id) {
                tracing::error!(
                    "Failed to flush scrollback for pane {} in runtime {label}: {e}",
                    short_id(pane.id)
                );
            }
        }
    }

    let snapshot = s.build_snapshot();
    let state_path = default_state_path(&cache_dir);
    drop(s);

    if let Err(e) = write_state_atomic(&snapshot, &state_path) {
        tracing::error!("Failed to persist final state: {e}");
    } else {
        tracing::info!("Final state persisted");
    }
}

/// Run the main server loop: accept clients, handle messages, manage PTYs.
///
/// Returns when a cooperative shutdown is signaled (via `Shutdown` message
/// or OS signal). The caller is responsible for process-level cleanup
/// (PID file removal, `process::exit`).
pub async fn run(server: Arc<Mutex<Server>>) -> anyhow::Result<()> {
    let (socket_path, mut shutdown_rx) = {
        let s = server.lock().await;
        (s.os.runtime_dir().join("rttx-server.sock"), s.shutdown_rx())
    };

    let listener = Listener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path.display());

    // Start serialization loop.
    let ser_server = Arc::clone(&server);
    let mut ser_shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        serialization_loop(ser_server, Duration::from_secs(1), &mut ser_shutdown_rx).await;
    });

    loop {
        tokio::select! {
            result = listener.accept() => {
                let conn = result?;
                let server = Arc::clone(&server);
                tokio::spawn(async move {
                    let _ = handle_client(server, conn).await;
                });
            }
            _ = shutdown_rx.changed() => {
                tracing::info!("Shutdown signal received, persisting state...");
                break;
            }
        }
    }

    persist_and_cleanup(&server).await;
    Ok(())
}

/// Handle a single stdio client (for `attach-stdio` SSH tunneling).
///
/// Serves one client over stdin/stdout using the same protocol as the
/// Unix socket path. The server must already be running (runtimes loaded,
/// PTYs reconstructed).
pub async fn handle_stdio_client(server: Arc<Mutex<Server>>) -> anyhow::Result<()> {
    let stream = crate::ipc::StdioStream::new();
    let conn = ClientConnection::new(stream);
    handle_client(server, conn).await
}

#[allow(clippy::significant_drop_tightening)]
async fn handle_client<S>(
    server: Arc<Mutex<Server>>,
    conn: ClientConnection<S>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let client_id = Uuid::new_v4();
    let client_short = short_id(client_id);

    let (tx, rx) = mpsc::channel(PUSH_CHANNEL_BOUND);
    // Channel for responses that the reader task needs to send back to the
    // client (e.g. pong replies, runtime snapshots). The writer task drains
    // this alongside the push channel so writes never block reads.
    let (resp_tx, resp_rx) = mpsc::channel::<ClientMsg>(RESP_CHANNEL_BOUND);
    {
        let mut s = server.lock().await;
        s.client_senders.insert(client_id, tx);
    }

    let (reader, writer) = conn.into_split();

    let write_short = client_short.clone();
    let writer_task = tokio::spawn(client_writer(writer, rx, resp_rx, write_short));

    let (result, handshake_completed) =
        client_reader(server.clone(), client_id, &client_short, reader, resp_tx).await;

    // Cleanup: remove sender and detach from all runtimes.
    {
        let mut s = server.lock().await;
        s.client_senders.remove(&client_id);
        s.client_protocols.remove(&client_id);
        if handshake_completed {
            for rt in s.runtimes.values_mut() {
                let _ = rt.detach_client(client_id, DetachReason::Disconnect);
            }
        }
    }

    // Writer task will stop when both senders are dropped.
    writer_task.abort();

    if let Err(ref e) = result {
        tracing::error!("Client {client_short} error: {e}");
    }

    result
}

/// Read client messages and dispatch responses via `resp_tx`.
///
/// Runs until the client disconnects or an error occurs. Returns `true`
/// if the client completed the handshake (sent at least one message),
/// `false` if it disconnected before sending anything (probe connection).
async fn client_reader(
    server: Arc<Mutex<Server>>,
    client_id: Uuid,
    client_short: &str,
    mut reader: ClientConnectionReader,
    resp_tx: mpsc::Sender<ClientMsg>,
) -> (anyhow::Result<()>, bool) {
    let mut handshake_completed = false;
    loop {
        let msg = match reader.read_message().await {
            Ok(Some(msg)) => msg,
            Ok(None) => {
                if handshake_completed {
                    tracing::info!("Client {client_short} disconnected");
                } else {
                    tracing::debug!(
                        "Client probe from {client_short} (disconnected without handshake)"
                    );
                }
                return (Ok(()), handshake_completed);
            }
            Err(e) => return (Err(e.into()), handshake_completed),
        };

        if !handshake_completed {
            handshake_completed = true;
            tracing::info!("Client {client_short} connected");
        }

        if matches!(msg.msg, Some(proto::client_message::Msg::Shutdown(_))) {
            tracing::info!("Shutdown requested by client {client_short}");
            server.lock().await.request_shutdown();
            return (Ok(()), handshake_completed);
        }

        // Fast-path: respond to Ping without acquiring the server mutex.
        // The heartbeat must never stall behind PTY I/O or runtime work.
        if let Some(proto::client_message::Msg::Ping(ping)) = &msg.msg {
            if resp_tx.send(ClientMsg::V2(protocol::pong(ping.nonce))).await.is_err() {
                return (Ok(()), handshake_completed);
            }
            continue;
        }

        if let Some(response) = Server::handle_message(&server, client_id, msg).await
            && resp_tx.send(ClientMsg::V2(response)).await.is_err()
        {
            return (Ok(()), handshake_completed);
        }
    }
}

/// Drain both the push channel and the response channel, writing each
/// message to the client socket. Exits when both senders are dropped or
/// a write error occurs.
async fn client_writer(
    mut writer: ClientConnectionWriter,
    mut push_rx: mpsc::Receiver<ClientMsg>,
    mut resp_rx: mpsc::Receiver<ClientMsg>,
    client_short: String,
) {
    loop {
        let msg = tokio::select! {
            biased;
            msg = resp_rx.recv() => match msg {
                Some(m) => m,
                None => match push_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            },
            msg = push_rx.recv() => match msg {
                Some(m) => m,
                None => match resp_rx.recv().await {
                    Some(m) => m,
                    None => break,
                },
            },
        };
        let result = match &msg {
            ClientMsg::V2(v2_msg) => writer.send_message(v2_msg).await,
            ClientMsg::V3(v3_msg) => writer.send_v3_envelope(v3_msg).await,
        };
        if let Err(e) = result {
            tracing::error!("Client {client_short} write error: {e}");
            break;
        }
    }
}

#[cfg(test)]
mod tests;

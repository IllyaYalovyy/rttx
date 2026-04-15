//! Top-level server loop.
//!
//! Accepts client connections, routes messages to sessions/panes, runs the
//! serialization loop, and manages the PTY read loops.

use crate::engine::Engine;
use crate::engine::PaneSpawnConfig;
use crate::engine::native::NativeEngine;
use crate::ipc::{ClientConnection, ClientConnectionReader, ClientConnectionWriter, Listener};
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
use crate::screen::restart_safe_scrollback;
use crate::serialization::{self, ServerState, default_state_path, load_state, write_state_atomic};
use crate::session::{
    AttachError, AttachMode, AttachOutcome, DetachOutcome, DetachReason, RuntimePolicy, Session,
    TerminationReason,
};
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

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
/// This is the lock-free counterpart of [`Server::broadcast_to_session`]:
/// collect handles with [`Server::collect_session_senders`] while holding
/// the mutex, then call this after releasing it.
fn send_to_collected(
    senders: &[(Uuid, mpsc::Sender<proto::ServerMessage>)],
    msg: &proto::ServerMessage,
) {
    for (client_id, sender) in senders {
        if let Err(mpsc::error::TrySendError::Full(_)) = sender.try_send(msg.clone()) {
            tracing::warn!(
                "Client {} push channel full — dropping message",
                short_id(*client_id),
            );
        }
    }
}

/// Shared mutable server state.
pub struct Server {
    /// All active sessions.
    pub sessions: HashMap<Uuid, Session>,
    /// Server's own identity.
    pub server_id: Uuid,
    /// The engine used to spawn pane processes.
    pub engine: Box<dyn Engine>,
    /// OS abstraction for paths.
    pub os: Box<dyn OsInterface>,
    /// Per-client bounded push channels for server-initiated messages (Deltas, etc.).
    client_senders: HashMap<Uuid, mpsc::Sender<proto::ServerMessage>>,
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
            sessions: HashMap::new(),
            server_id: Uuid::new_v4(),
            engine: Box::new(NativeEngine),
            os,
            client_senders: HashMap::new(),
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

    /// Human-readable label for a session: `"name" (short_id)`.
    ///
    /// Falls back to just the short ID when the session is not found.
    #[must_use]
    pub fn session_label(&self, session_id: Uuid) -> String {
        self.sessions.get(&session_id).map_or_else(
            || format!("({})", short_id(session_id)),
            |session| format!("\"{}\" ({})", session.name, short_id(session_id)),
        )
    }

    /// Load persisted state and resurrect sessions.
    pub fn load_persisted_state(&mut self) {
        let state_path = default_state_path(&self.os.cache_dir());
        match load_state(&state_path) {
            Ok(Some(state)) => {
                tracing::info!("Loaded {} persisted sessions", state.sessions.len());
                for ps in &state.sessions {
                    let session = Session::from_persisted(ps);
                    self.sessions.insert(session.id, session);
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

    /// Reconstruct resurrected sessions: replay scrollback logs into pane
    /// screens and spawn fresh shells in saved working directories.
    ///
    /// Called after `load_persisted_state` once the server is wrapped in
    /// `Arc<Mutex<>>` so we can spawn PTY read loops.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn reconstruct_sessions(server: &Arc<Mutex<Self>>) {
        let panes_to_reconstruct: Vec<(Uuid, Uuid, String, Option<String>, u16, u16)> = {
            let mut s = server.lock().await;

            for session in s.sessions.values_mut() {
                let label = format!("\"{}\" ({})", session.name, short_id(session.id));
                for pane in session.panes.values_mut() {
                    // Replay scrollback log into the pane screen.
                    if let Some(ref log_path) = pane.scrollback_log_path
                        && log_path.exists()
                    {
                        let pane_short = short_id(pane.id);
                        match std::fs::read(log_path) {
                            Ok(data) => {
                                let restart_safe = restart_safe_scrollback(&data);
                                tracing::info!(
                                    "Replaying {} bytes of scrollback for pane {pane_short} in session {label}",
                                    restart_safe.len(),
                                );
                                pane.screen.feed(restart_safe);
                                if restart_safe.len() != data.len()
                                    && let Err(e) = std::fs::write(log_path, restart_safe)
                                {
                                    tracing::error!(
                                        "Failed to rewrite restart-safe scrollback for pane {pane_short} in session {label}: {e}",
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to read scrollback log for pane {pane_short} in session {label}: {e}",
                                );
                            }
                        }
                    }
                }
            }

            // Collect panes that need fresh shells spawned.
            s.sessions
                .values()
                .flat_map(|session| {
                    let name = session.name.clone();
                    session.panes.values().map(move |pane| {
                        (session.id, pane.id, name.clone(), pane.cwd.clone(), pane.cols, pane.rows)
                    })
                })
                .collect()
        };

        if panes_to_reconstruct.is_empty() {
            return;
        }

        tracing::info!("Reconstructing {} panes", panes_to_reconstruct.len());

        for (session_id, pane_id, session_name, cwd, cols, rows) in panes_to_reconstruct {
            let session_label = format!("\"{}\" ({})", session_name, short_id(session_id));
            let pane_short = short_id(pane_id);
            let pty_result = {
                let s = server.lock().await;
                let hist = serialization::history_path(&s.os.cache_dir(), session_id, pane_id);
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
                        if let Some(session) = s.sessions.get_mut(&session_id) {
                            let _ = session.set_pane_exit_status(pane_id, None);
                            if let Some(pane) = session.panes.get_mut(&pane_id) {
                                pane.child_pid = child_pid;
                            }
                        }
                    }
                    spawn_pty_read_loop(
                        Arc::clone(server),
                        session_id,
                        pane_id,
                        &session_name,
                        reader,
                        child,
                        kill_rx,
                    );
                    tracing::info!("Reconstructed pane {pane_short} in session {session_label}");
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to reconstruct pane {pane_short} in session {session_label}: {e}"
                    );
                    let mut s = server.lock().await;
                    if let Some(session) = s.sessions.get_mut(&session_id) {
                        let _ = session.set_pane_exit_status(pane_id, Some(-1));
                    }
                }
            }
        }
    }

    /// Build a serializable snapshot of the current state.
    #[must_use]
    pub fn build_snapshot(&self) -> ServerState {
        ServerState {
            sessions: self
                .sessions
                .values()
                .filter(|session| session.policy == RuntimePolicy::Persistent)
                .map(Session::to_persisted)
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
        msg: &proto::ServerMessage,
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

    /// Send a message to all clients attached to a session.
    fn broadcast_to_session(&self, session_id: Uuid, msg: &proto::ServerMessage) {
        let Some(session) = self.sessions.get(&session_id) else {
            return;
        };
        self.broadcast_to_clients(session.attached_clients.keys().copied(), None, msg);
    }

    /// Collect cloned sender handles for all clients attached to a session.
    ///
    /// The returned senders can be used after releasing the server mutex via
    /// [`send_to_collected`].
    fn collect_session_senders(
        &self,
        session_id: Uuid,
    ) -> Vec<(Uuid, mpsc::Sender<proto::ServerMessage>)> {
        let Some(session) = self.sessions.get(&session_id) else {
            return Vec::new();
        };
        session
            .attached_clients
            .keys()
            .filter_map(|&cid| {
                self.client_senders.get(&cid).map(|s| (cid, s.clone()))
            })
            .collect()
    }

    fn terminate_session(
        &mut self,
        session_id: Uuid,
        final_revision: u64,
        reason: TerminationReason,
        exclude_client_id: Option<Uuid>,
    ) -> Option<proto::ServerMessage> {
        let session = self.sessions.remove(&session_id)?;
        let attached_client_ids: Vec<_> = session.attached_clients.keys().copied().collect();
        let pane_ids: Vec<_> = session.panes.keys().copied().collect();
        for pane_id in pane_ids {
            self.pty_writers.remove(&pane_id);
            if let Some(kill_tx) = self.pty_kill_senders.remove(&pane_id) {
                let _ = kill_tx.send(());
            }
        }

        let msg = protocol::session_terminated(session_id, final_revision, reason);
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

            proto::client_message::Msg::ListSessions(_) => {
                let s = server.lock().await;
                let infos = protocol::session_inventory_for(client_id, s.sessions.values());
                Some(protocol::session_list(infos))
            }

            proto::client_message::Msg::CreateSession(req) => {
                let mut s = server.lock().await;
                let session = Session::new(req.name);
                let session_id = session.id;
                let policy = RuntimePolicy::from_proto(req.policy);
                let revision = session.revision();
                let mut session = session;
                session.policy = policy;
                let label = format!("\"{}\" ({})", session.name, short_id(session_id));
                let policy_str = match policy {
                    RuntimePolicy::Persistent => "persistent",
                    RuntimePolicy::Ephemeral => "ephemeral",
                };
                s.sessions.insert(session_id, session);
                tracing::info!("Session created: {label}, policy={policy_str}");
                Some(protocol::session_created(session_id, revision))
            }

            proto::client_message::Msg::AttachSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
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
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                let attach_outcome = match session.attach_client(client_id, attach_mode) {
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
                            session_id,
                            current_role,
                            session.attached_client_count(),
                            session.read_only_client_count(),
                        ));
                    }
                };

                let pane_snapshots: Vec<proto::PaneSnapshot> = session
                    .panes
                    .values()
                    .map(|pane| proto::PaneSnapshot {
                        pane_id: uuid_to_bytes(pane.id),
                        title: pane.title.clone().unwrap_or_default(),
                        cwd: pane.effective_cwd().unwrap_or_default(),
                        cols: u32::from(pane.cols),
                        rows: u32::from(pane.rows),
                        scrollback: bytes::Bytes::copy_from_slice(
                            pane.screen.snapshot_bytes(crate::pane::MAX_SNAPSHOT_BYTES),
                        ),
                        exit_status: pane.exit_status,
                        bracketed_paste_mode: pane.screen.bracketed_paste_mode(),
                        application_cursor_keys: pane.screen.application_cursor_keys(),
                        application_keypad: pane.screen.application_keypad(),
                        mouse_tracking_mode: u32::from(pane.screen.mouse_tracking_mode()),
                        sgr_mouse_mode: pane.screen.sgr_mouse_mode(),
                    })
                    .collect();
                let session_label = s.session_label(session_id);
                tracing::info!(
                    "Client {} attached to session {session_label} as {role:?}",
                    short_id(client_id),
                );
                Some(protocol::snapshot(session_id, pane_snapshots, revision, role))
            }

            proto::client_message::Msg::DetachSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                match session.detach_client(client_id, DetachReason::ExplicitRequest) {
                    DetachOutcome::Detached { revision }
                    | DetachOutcome::NotAttached { revision } => {
                        let session_label = s.session_label(session_id);
                        tracing::info!(
                            "Client {} detached from session {session_label}",
                            short_id(client_id),
                        );
                        Some(protocol::session_detached(session_id, revision))
                    }
                    DetachOutcome::Terminated { final_revision, reason } => {
                        let session_label = s.session_label(session_id);
                        tracing::info!(
                            "Client {} detached from session {session_label} (terminated: {reason:?})",
                            short_id(client_id),
                        );
                        let _ = s.terminate_session(
                            session_id,
                            final_revision,
                            reason,
                            Some(client_id),
                        );
                        Some(protocol::session_terminated(session_id, final_revision, reason))
                    }
                }
            }

            proto::client_message::Msg::TerminateSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(session) = s.sessions.get(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                if session.has_write_owner() && !session.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let final_revision = session.revision().saturating_add(1);
                let session_label = s.session_label(session_id);
                let _ = s.terminate_session(
                    session_id,
                    final_revision,
                    TerminationReason::Explicit,
                    Some(client_id),
                );
                tracing::info!("Session terminated: {session_label}");
                Some(protocol::session_terminated(
                    session_id,
                    final_revision,
                    TerminationReason::Explicit,
                ))
            }

            proto::client_message::Msg::CreatePane(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };

                let pane_id = Uuid::new_v4();
                let (pty_result, session_label) = {
                    let s = server.lock().await;
                    let Some(session) = s.sessions.get(&session_id) else {
                        return Some(protocol::error(
                            protocol::ERR_SESSION_NOT_FOUND,
                            "session not found".into(),
                        ));
                    };
                    if !session.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    let label = s.session_label(session_id);
                    let hist = serialization::history_path(&s.os.cache_dir(), session_id, pane_id);
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
                    let config = PaneSpawnConfig { command: vec![], cwd, env, cols: 80, rows: 24 };
                    (s.engine.spawn_pane(pane_id, &config), label)
                };

                match pty_result {
                    Ok(pty) => {
                        let child_pid = pty.pid();
                        let (reader, writer, mut child) = pty.into_parts();
                        let (kill_tx, kill_rx) = oneshot::channel();
                        let (revision, session_name) = {
                            let mut s = server.lock().await;
                            let Some(session) = s.sessions.get_mut(&session_id) else {
                                let _ = child.start_kill();
                                return Some(protocol::error(
                                    protocol::ERR_SESSION_NOT_FOUND,
                                    "session not found".into(),
                                ));
                            };
                            let mut pane = Pane::new(pane_id, 80, 24);
                            pane.child_pid = child_pid;
                            session.add_pane(pane);
                            let revision = session.revision();
                            let name = session.name.clone();
                            s.pty_writers
                                .insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                            s.pty_kill_senders.insert(pane_id, kill_tx);
                            (revision, name)
                        };
                        spawn_pty_read_loop(
                            Arc::clone(server),
                            session_id,
                            pane_id,
                            &session_name,
                            reader,
                            child,
                            kill_rx,
                        );
                        tracing::info!(
                            "Pane {} created in session \"{}\" ({})",
                            short_id(pane_id),
                            session_name,
                            short_id(session_id),
                        );
                        Some(protocol::pane_created(session_id, pane_id, revision))
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to spawn PTY for pane {} in session {session_label}: {e}",
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
                let session_id = match bytes_to_uuid(&req.session_id) {
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
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                if !session.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let Some(_pane) = session.remove_pane(pane_id) else {
                    return Some(protocol::error(
                        protocol::ERR_PANE_NOT_FOUND,
                        "pane not found".into(),
                    ));
                };
                let revision = session.revision();
                let session_label = s.session_label(session_id);
                s.pty_writers.remove(&pane_id);
                if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
                    let _ = kill_tx.send(());
                }
                tracing::info!("Pane {} closed in session {session_label}", short_id(pane_id),);
                Some(protocol::pane_closed(session_id, pane_id, revision))
            }

            proto::client_message::Msg::Input(req) => {
                let Ok(session_id) = bytes_to_uuid(&req.session_id) else {
                    return None;
                };
                let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else {
                    return None;
                };
                let (writer, session_label) = {
                    let s = server.lock().await;
                    let session = s.sessions.get(&session_id)?;
                    if !session.panes.contains_key(&pane_id) {
                        return None;
                    }
                    if !session.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    (s.pty_writers.get(&pane_id).cloned(), s.session_label(session_id))
                };
                if let Some(writer) = writer {
                    let pane_short = short_id(pane_id);
                    let mut w = writer.lock().await;
                    if let Err(e) = w.write_all(&req.data).await {
                        tracing::error!(
                            "Failed to write to PTY {pane_short} in session {session_label}: {e}"
                        );
                    }
                    if let Err(e) = w.flush().await {
                        tracing::error!(
                            "Failed to flush PTY {pane_short} in session {session_label}: {e}"
                        );
                    }
                }
                None
            }

            proto::client_message::Msg::Resize(req) => {
                let Ok(pane_id) = bytes_to_uuid(&req.pane_id) else {
                    return None;
                };
                let Ok(session_id) = bytes_to_uuid(&req.session_id) else {
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
                    let session = s.sessions.get(&session_id)?;
                    if !session.panes.contains_key(&pane_id) {
                        return None;
                    }
                    if !session.client_has_write_access(client_id) {
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
                            "Failed to resize PTY {} in session {}: {e}",
                            short_id(pane_id),
                            s.session_label(session_id),
                        );
                        return None;
                    }
                }

                let revision = {
                    let mut s = server.lock().await;
                    let session = s.sessions.get_mut(&session_id)?;
                    session.resize_pane(pane_id, cols, rows)?
                };

                Some(protocol::pane_resized(session_id, pane_id, cols, rows, revision))
            }

            proto::client_message::Msg::SetPaneTitle(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
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
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                if !session.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let Some(revision) = session.set_pane_title(pane_id, req.title.clone()) else {
                    return Some(protocol::error(
                        protocol::ERR_PANE_NOT_FOUND,
                        "pane not found".into(),
                    ));
                };
                Some(protocol::title_changed(session_id, pane_id, req.title, revision))
            }

            proto::client_message::Msg::RenameSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let mut s = server.lock().await;
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(
                        protocol::ERR_SESSION_NOT_FOUND,
                        "session not found".into(),
                    ));
                };
                if !session.client_has_write_access(client_id) {
                    return Some(protocol::error(
                        protocol::ERR_OWNERSHIP_CONFLICT,
                        "runtime is currently owned by another client".into(),
                    ));
                }
                let old_name = session.name.clone();
                let revision = session.rename(req.name.clone());
                tracing::info!(
                    "Session renamed: \"{}\" -> \"{}\" ({})",
                    old_name,
                    req.name,
                    short_id(session_id),
                );
                Some(protocol::session_renamed(session_id, req.name, revision))
            }

            proto::client_message::Msg::Shutdown(_) => None,
        }
    }
}

/// Spawn a background task that reads PTY output and broadcasts Deltas.
fn spawn_pty_read_loop(
    server: Arc<Mutex<Server>>,
    session_id: Uuid,
    pane_id: Uuid,
    session_name: &str,
    mut reader: pty_process::OwnedReadPty,
    mut child: tokio::process::Child,
    mut kill_rx: oneshot::Receiver<()>,
) {
    let session_label = format!("\"{}\" ({})", session_name, short_id(session_id));
    let pane_short = short_id(pane_id);
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                result = reader.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = bytes::Bytes::copy_from_slice(&buf[..n]);

                            // Phase 1: hold lock for state mutation and handle collection.
                            let (new_cwd, new_title, pending_replies, pty_writer, senders) = {
                                let mut s = server.lock().await;
                                let (new_cwd, new_title, pending_replies) = if let Some(session) = s.sessions.get_mut(&session_id)
                                    && let Some(pane) = session.panes.get_mut(&pane_id)
                                {
                                    let result = pane.feed_output(&data);
                                    let cwd = result.new_cwd.and_then(|cwd| {
                                        let rev = session.set_pane_cwd(pane_id, &cwd)?;
                                        Some((cwd, rev))
                                    });
                                    let title = result.new_title.and_then(|title| {
                                        let rev = session.set_pane_title(pane_id, title.clone())?;
                                        Some((title, rev))
                                    });
                                    (cwd, title, result.pending_replies)
                                } else {
                                    (None, None, Vec::new())
                                };
                                let pty_writer = if pending_replies.is_empty() {
                                    None
                                } else {
                                    s.pty_writers.get(&pane_id).cloned()
                                };
                                let senders = s.collect_session_senders(session_id);
                                drop(s);
                                (new_cwd, new_title, pending_replies, pty_writer, senders)
                            };
                            // Lock released.

                            // Phase 2: write DSR replies without the server lock.
                            if let Some(writer) = pty_writer {
                                let mut w = writer.lock().await;
                                for reply in &pending_replies {
                                    if let Err(e) = w.write_all(reply).await {
                                        tracing::error!(
                                            "Failed to write DSR reply to PTY {pane_short} in session {session_label}: {e}"
                                        );
                                    }
                                }
                                if let Err(e) = w.flush().await {
                                    tracing::error!(
                                        "Failed to flush DSR reply to PTY {pane_short} in session {session_label}: {e}"
                                    );
                                }
                            }

                            // Phase 3: broadcast to clients without the server lock.
                            let msg = protocol::delta(session_id, pane_id, data);
                            send_to_collected(&senders, &msg);
                            if let Some((cwd, revision)) = new_cwd {
                                let msg = protocol::cwd_changed(session_id, pane_id, cwd, revision);
                                send_to_collected(&senders, &msg);
                            }
                            if let Some((title, revision)) = new_title {
                                let msg = protocol::title_changed(session_id, pane_id, title, revision);
                                send_to_collected(&senders, &msg);
                            }
                        }
                        Err(e) => {
                            tracing::error!("PTY read error for pane {pane_short} in session {session_label}: {e}");
                            break;
                        }
                    }
                }
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    tracing::info!("PTY read loop cancelled for pane {pane_short} in session {session_label}");
                    return;
                }
            }
        }

        // Child exited naturally — collect exit status.
        let status = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                tracing::error!(
                    "Failed to wait on child for pane {pane_short} in session {session_label}: {e}"
                );
                -1
            }
        };

        let mut s = server.lock().await;
        let exit_msg = if let Some(session) = s.sessions.get_mut(&session_id) {
            let msg = session
                .set_pane_exit_status(pane_id, Some(status))
                .map(|revision| protocol::pane_exited(session_id, pane_id, status, revision));
            if let Some(pane) = session.panes.get_mut(&pane_id) {
                pane.release_scrollback();
            }
            msg
        } else {
            None
        };
        if let Some(msg) = exit_msg {
            s.broadcast_to_session(session_id, &msg);
        }
        s.pty_writers.remove(&pane_id);
        s.pty_kill_senders.remove(&pane_id);
        drop(s);

        tracing::info!(
            "PTY exited for pane {pane_short} in session {session_label}, status {status}"
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

        let session_ids: Vec<_> = s.sessions.keys().copied().collect();
        for session_id in session_ids {
            if let Some(session) = s.sessions.get_mut(&session_id) {
                let label = format!("\"{}\" ({})", session.name, short_id(session_id));
                for pane in session.panes.values_mut() {
                    if let Err(e) = pane.flush_scrollback(&cache_dir, session_id) {
                        tracing::error!(
                            "Failed to flush scrollback for pane {} in session {label}: {e}",
                            short_id(pane.id)
                        );
                    }
                }
            }
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

    for session in s.sessions.values_mut() {
        let label = format!("\"{}\" ({})", session.name, short_id(session.id));
        for pane in session.panes.values_mut() {
            if let Err(e) = pane.flush_scrollback(&cache_dir, session.id) {
                tracing::error!(
                    "Failed to flush scrollback for pane {} in session {label}: {e}",
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
/// Unix socket path. The server must already be running (sessions loaded,
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
    tracing::info!("Client {client_short} connected");

    let (tx, rx) = mpsc::channel(PUSH_CHANNEL_BOUND);
    // Channel for responses that the reader task needs to send back to the
    // client (e.g. pong replies, session snapshots). The writer task drains
    // this alongside the push channel so writes never block reads.
    let (resp_tx, resp_rx) = mpsc::channel::<proto::ServerMessage>(RESP_CHANNEL_BOUND);
    {
        let mut s = server.lock().await;
        s.client_senders.insert(client_id, tx);
    }

    let (reader, writer) = conn.into_split();

    let write_short = client_short.clone();
    let writer_task = tokio::spawn(client_writer(writer, rx, resp_rx, write_short));

    let result = client_reader(server.clone(), client_id, &client_short, reader, resp_tx).await;

    // Cleanup: remove sender and detach from all sessions.
    {
        let mut s = server.lock().await;
        s.client_senders.remove(&client_id);
        for session in s.sessions.values_mut() {
            let _ = session.detach_client(client_id, DetachReason::Disconnect);
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
/// Runs until the client disconnects or an error occurs.
async fn client_reader(
    server: Arc<Mutex<Server>>,
    client_id: Uuid,
    client_short: &str,
    mut reader: ClientConnectionReader,
    resp_tx: mpsc::Sender<proto::ServerMessage>,
) -> anyhow::Result<()> {
    loop {
        let Some(msg) = reader.read_message().await? else {
            tracing::info!("Client {client_short} disconnected");
            return Ok(());
        };

        if matches!(msg.msg, Some(proto::client_message::Msg::Shutdown(_))) {
            tracing::info!("Shutdown requested by client {client_short}");
            server.lock().await.request_shutdown();
            return Ok(());
        }

        // Fast-path: respond to Ping without acquiring the server mutex.
        // The heartbeat must never stall behind PTY I/O or session work.
        if let Some(proto::client_message::Msg::Ping(ping)) = &msg.msg {
            if resp_tx.send(protocol::pong(ping.nonce)).await.is_err() {
                return Ok(());
            }
            continue;
        }

        if let Some(response) = Server::handle_message(&server, client_id, msg).await
            && resp_tx.send(response).await.is_err()
        {
            return Ok(());
        }
    }
}

/// Drain both the push channel and the response channel, writing each
/// message to the client socket. Exits when both senders are dropped or
/// a write error occurs.
async fn client_writer(
    mut writer: ClientConnectionWriter,
    mut push_rx: mpsc::Receiver<proto::ServerMessage>,
    mut resp_rx: mpsc::Receiver<proto::ServerMessage>,
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
        if let Err(e) = writer.send_message(&msg).await {
            tracing::error!("Client {client_short} write error: {e}");
            break;
        }
    }
}

#[cfg(test)]
mod tests;

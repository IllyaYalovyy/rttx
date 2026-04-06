//! Top-level server loop.
//!
//! Accepts client connections, routes messages to sessions/panes, runs the
//! serialization loop, and manages the PTY read loops.

use crate::engine::Engine;
use crate::engine::PaneSpawnConfig;
use crate::engine::native::NativeEngine;
use crate::ipc::{ClientConnection, Listener};
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
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
    /// Per-client push channels for server-initiated messages (Deltas, etc.).
    client_senders: HashMap<Uuid, mpsc::UnboundedSender<proto::ServerMessage>>,
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
        let panes_to_reconstruct: Vec<(Uuid, Uuid, Option<String>, u16, u16)> = {
            let mut s = server.lock().await;

            for session in s.sessions.values_mut() {
                for pane in session.panes.values_mut() {
                    // Replay scrollback log into the pane screen.
                    if let Some(ref log_path) = pane.scrollback_log_path
                        && log_path.exists()
                    {
                        match std::fs::read(log_path) {
                            Ok(data) => {
                                tracing::info!(
                                    "Replaying {} bytes of scrollback for pane {}",
                                    data.len(),
                                    pane.id
                                );
                                pane.screen.feed(&data);
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to read scrollback log for pane {}: {e}",
                                    pane.id
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
                    session.panes.values().map(move |pane| {
                        (session.id, pane.id, pane.cwd.clone(), pane.cols, pane.rows)
                    })
                })
                .collect()
        };

        if panes_to_reconstruct.is_empty() {
            return;
        }

        tracing::info!("Reconstructing {} panes", panes_to_reconstruct.len());

        for (session_id, pane_id, cwd, cols, rows) in panes_to_reconstruct {
            let pty_result = {
                let s = server.lock().await;
                let hist = serialization::history_path(&s.os.cache_dir(), session_id, pane_id);
                if let Some(parent) = hist.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let env = vec![("HISTFILE".into(), hist.to_string_lossy().into_owned())];
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
                        reader,
                        child,
                        kill_rx,
                    );
                    tracing::info!("Reconstructed pane {pane_id} in session {session_id}");
                }
                Err(e) => {
                    tracing::error!("Failed to reconstruct pane {pane_id}: {e}");
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
            if let Some(sender) = self.client_senders.get(&client_id) {
                let _ = sender.send(msg.clone());
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
                s.sessions.insert(session_id, session);
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
                        scrollback: pane.screen.raw_bytes().to_vec(),
                        exit_status: pane.exit_status,
                    })
                    .collect();
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
                        Some(protocol::session_detached(session_id, revision))
                    }
                    DetachOutcome::Terminated { final_revision, reason } => {
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
                let _ = s.terminate_session(
                    session_id,
                    final_revision,
                    TerminationReason::Explicit,
                    Some(client_id),
                );
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
                let pty_result = {
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
                    let hist = serialization::history_path(&s.os.cache_dir(), session_id, pane_id);
                    if let Some(parent) = hist.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let env = vec![("HISTFILE".into(), hist.to_string_lossy().into_owned())];
                    let cwd = req.cwd;
                    let config = PaneSpawnConfig { command: vec![], cwd, env, cols: 80, rows: 24 };
                    s.engine.spawn_pane(pane_id, &config)
                };

                match pty_result {
                    Ok(pty) => {
                        let child_pid = pty.pid();
                        let (reader, writer, mut child) = pty.into_parts();
                        let (kill_tx, kill_rx) = oneshot::channel();
                        let revision = {
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
                            s.pty_writers
                                .insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                            s.pty_kill_senders.insert(pane_id, kill_tx);
                            revision
                        };
                        spawn_pty_read_loop(
                            Arc::clone(server),
                            session_id,
                            pane_id,
                            reader,
                            child,
                            kill_rx,
                        );
                        Some(protocol::pane_created(session_id, pane_id, revision))
                    }
                    Err(e) => {
                        tracing::error!("Failed to spawn PTY for pane {pane_id}: {e}");
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
                s.pty_writers.remove(&pane_id);
                if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
                    let _ = kill_tx.send(());
                }
                Some(protocol::pane_closed(session_id, pane_id, revision))
            }

            proto::client_message::Msg::Input(req) => {
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
                let writer = {
                    let s = server.lock().await;
                    let Some(session) = s.sessions.get(&session_id) else {
                        return Some(protocol::error(
                            protocol::ERR_SESSION_NOT_FOUND,
                            "session not found".into(),
                        ));
                    };
                    if !session.panes.contains_key(&pane_id) {
                        return Some(protocol::error(
                            protocol::ERR_PANE_NOT_FOUND,
                            "pane not found".into(),
                        ));
                    }
                    if !session.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    s.pty_writers.get(&pane_id).cloned()
                };
                if let Some(writer) = writer {
                    let mut w = writer.lock().await;
                    if let Err(e) = w.write_all(&req.data).await {
                        tracing::error!("Failed to write to PTY {pane_id}: {e}");
                    }
                    if let Err(e) = w.flush().await {
                        tracing::error!("Failed to flush PTY {pane_id}: {e}");
                    }
                }
                None
            }

            proto::client_message::Msg::Resize(req) => {
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => {
                        return Some(protocol::error(
                            protocol::ERR_INVALID_PARAMETER,
                            e.to_string(),
                        ));
                    }
                };
                let Ok(cols) = u16::try_from(req.cols) else {
                    return Some(protocol::error(
                        protocol::ERR_INVALID_PARAMETER,
                        "cols out of range".into(),
                    ));
                };
                let Ok(rows) = u16::try_from(req.rows) else {
                    return Some(protocol::error(
                        protocol::ERR_INVALID_PARAMETER,
                        "rows out of range".into(),
                    ));
                };

                let writer = {
                    let s = server.lock().await;
                    let Some(session) = s.sessions.get(&session_id) else {
                        return Some(protocol::error(
                            protocol::ERR_SESSION_NOT_FOUND,
                            "session not found".into(),
                        ));
                    };
                    if !session.panes.contains_key(&pane_id) {
                        return Some(protocol::error(
                            protocol::ERR_PANE_NOT_FOUND,
                            "pane not found".into(),
                        ));
                    }
                    if !session.client_has_write_access(client_id) {
                        return Some(protocol::error(
                            protocol::ERR_OWNERSHIP_CONFLICT,
                            "runtime is currently owned by another client".into(),
                        ));
                    }
                    let Some(writer) = s.pty_writers.get(&pane_id) else {
                        return Some(protocol::error(
                            protocol::ERR_PANE_NOT_RUNNING,
                            "pane is not running".into(),
                        ));
                    };
                    Arc::clone(writer)
                };

                {
                    let w = writer.lock().await;
                    if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                        tracing::error!("Failed to resize PTY {pane_id}: {e}");
                        return Some(protocol::error(
                            protocol::ERR_PANE_NOT_RUNNING,
                            format!("failed to resize pane: {e}"),
                        ));
                    }
                }

                let revision = {
                    let mut s = server.lock().await;
                    let Some(session) = s.sessions.get_mut(&session_id) else {
                        return Some(protocol::error(
                            protocol::ERR_SESSION_NOT_FOUND,
                            "session not found".into(),
                        ));
                    };
                    let Some(revision) = session.resize_pane(pane_id, cols, rows) else {
                        return Some(protocol::error(
                            protocol::ERR_PANE_NOT_FOUND,
                            "pane not found".into(),
                        ));
                    };
                    revision
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

            proto::client_message::Msg::Shutdown(_) => None,
        }
    }
}

/// Spawn a background task that reads PTY output and broadcasts Deltas.
fn spawn_pty_read_loop(
    server: Arc<Mutex<Server>>,
    session_id: Uuid,
    pane_id: Uuid,
    mut reader: pty_process::OwnedReadPty,
    mut child: tokio::process::Child,
    mut kill_rx: oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                result = reader.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = buf[..n].to_vec();
                            let mut s = server.lock().await;
                            if let Some(session) = s.sessions.get_mut(&session_id)
                                && let Some(pane) = session.panes.get_mut(&pane_id)
                            {
                                pane.feed_output(&data);
                            }
                            let msg = protocol::delta(session_id, pane_id, data);
                            s.broadcast_to_session(session_id, &msg);
                        }
                        Err(e) => {
                            tracing::error!("PTY read error for pane {pane_id}: {e}");
                            break;
                        }
                    }
                }
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    tracing::info!("PTY read loop cancelled for pane {pane_id}");
                    return;
                }
            }
        }

        // Child exited naturally — collect exit status.
        let status = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                tracing::error!("Failed to wait on child for pane {pane_id}: {e}");
                -1
            }
        };

        let mut s = server.lock().await;
        if let Some(session) = s.sessions.get_mut(&session_id)
            && let Some(revision) = session.set_pane_exit_status(pane_id, Some(status))
        {
            let msg = protocol::pane_exited(session_id, pane_id, status, revision);
            s.broadcast_to_session(session_id, &msg);
        }
        s.pty_writers.remove(&pane_id);
        s.pty_kill_senders.remove(&pane_id);
        drop(s);

        tracing::info!("PTY exited for pane {pane_id}, status {status}");
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
                for pane in session.panes.values_mut() {
                    if let Err(e) = pane.flush_scrollback(&cache_dir, session_id) {
                        tracing::error!("Failed to flush scrollback for pane {}: {e}", pane.id);
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
        for pane in session.panes.values_mut() {
            if let Err(e) = pane.flush_scrollback(&cache_dir, session.id) {
                tracing::error!("Failed to flush scrollback for pane {}: {e}", pane.id);
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
                    if let Err(e) = handle_client(server, conn).await {
                        tracing::error!("Client error: {e}");
                    }
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
    mut conn: ClientConnection<S>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let client_id = Uuid::new_v4();
    tracing::info!("Client {client_id} connected");

    let (tx, mut rx) = mpsc::unbounded_channel();
    {
        let mut s = server.lock().await;
        s.client_senders.insert(client_id, tx);
    }

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                msg_result = conn.read_message() => {
                    let Some(msg) = msg_result? else {
                        tracing::info!("Client {client_id} disconnected");
                        break;
                    };

                    if matches!(msg.msg, Some(proto::client_message::Msg::Shutdown(_))) {
                        tracing::info!("Shutdown requested by client {client_id}");
                        let s = server.lock().await;
                        s.request_shutdown();
                        break;
                    }

                    if let Some(response) = Server::handle_message(&server, client_id, msg).await {
                        conn.send_message(&response).await?;
                    }
                }
                Some(push_msg) = rx.recv() => {
                    conn.send_message(&push_msg).await?;
                }
            }
        }
        Ok(())
    }
    .await;

    // Cleanup: remove sender and detach from all sessions.
    {
        let mut s = server.lock().await;
        s.client_senders.remove(&client_id);
        for session in s.sessions.values_mut() {
            let _ = session.detach_client(client_id, DetachReason::Disconnect);
        }
    }

    result
}

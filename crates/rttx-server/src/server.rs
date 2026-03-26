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
use crate::serialization::{ServerState, default_state_path, load_state, write_state_atomic};
use crate::session::Session;
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot};
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
}

impl Server {
    /// Create a new server with the native engine.
    #[must_use]
    pub fn new(os: Box<dyn OsInterface>) -> Self {
        Self {
            sessions: HashMap::new(),
            server_id: Uuid::new_v4(),
            engine: Box::new(NativeEngine),
            os,
            client_senders: HashMap::new(),
            pty_writers: HashMap::new(),
            pty_kill_senders: HashMap::new(),
        }
    }

    /// Load persisted state and resurrect sessions.
    pub fn load_persisted_state(&mut self) {
        let state_path = default_state_path(&self.os.cache_dir());
        match load_state(&state_path) {
            Ok(Some(state)) => {
                log::info!("Loaded {} persisted sessions", state.sessions.len());
                for ps in &state.sessions {
                    let session = Session::from_persisted(ps);
                    self.sessions.insert(session.id, session);
                }
            }
            Ok(None) => {
                log::info!("No persisted state found, starting fresh");
            }
            Err(e) => {
                log::error!("Failed to load persisted state: {e}");
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
                                log::info!(
                                    "Replaying {} bytes of scrollback for pane {}",
                                    data.len(),
                                    pane.id
                                );
                                pane.screen.feed(&data);
                            }
                            Err(e) => {
                                log::error!(
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

        log::info!("Reconstructing {} panes", panes_to_reconstruct.len());

        for (session_id, pane_id, cwd, cols, rows) in panes_to_reconstruct {
            let pty_result = {
                let s = server.lock().await;
                let config = PaneSpawnConfig { command: vec![], cwd, cols, rows };
                s.engine.spawn_pane(pane_id, &config)
            };

            match pty_result {
                Ok(pty) => {
                    let (reader, writer, child) = pty.into_parts();
                    let (kill_tx, kill_rx) = oneshot::channel();
                    {
                        let mut s = server.lock().await;
                        s.pty_writers.insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                        s.pty_kill_senders.insert(pane_id, kill_tx);
                        // Clear exit status — fresh shell is running.
                        if let Some(session) = s.sessions.get_mut(&session_id)
                            && let Some(pane) = session.panes.get_mut(&pane_id)
                        {
                            pane.exit_status = None;
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
                    log::info!("Reconstructed pane {pane_id} in session {session_id}");
                }
                Err(e) => {
                    log::error!("Failed to reconstruct pane {pane_id}: {e}");
                    let mut s = server.lock().await;
                    if let Some(session) = s.sessions.get_mut(&session_id)
                        && let Some(pane) = session.panes.get_mut(&pane_id)
                    {
                        pane.set_exited(-1);
                    }
                }
            }
        }
    }

    /// Build a serializable snapshot of the current state.
    #[must_use]
    pub fn build_snapshot(&self) -> ServerState {
        ServerState {
            sessions: self.sessions.values().map(Session::to_persisted).collect(),
            serialized_at: std::time::SystemTime::now(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Send a message to all clients attached to a session.
    fn broadcast_to_session(&self, session_id: Uuid, msg: &proto::ServerMessage) {
        let Some(session) = self.sessions.get(&session_id) else {
            return;
        };
        for client_id in &session.attached_clients {
            if let Some(sender) = self.client_senders.get(client_id) {
                let _ = sender.send(msg.clone());
            }
        }
    }

    /// Handle a single client message, returning an optional response.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn handle_message(
        server: &Arc<Mutex<Self>>,
        client_id: Uuid,
        msg: proto::ClientMessage,
    ) -> Option<proto::ServerMessage> {
        let Some(inner) = msg.msg else {
            return Some(protocol::error(1, "empty message".into()));
        };

        match inner {
            proto::client_message::Msg::Hello(hello) => {
                if hello.protocol_version != rttx_proto::PROTOCOL_VERSION {
                    return Some(protocol::error(
                        2,
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
                let infos: Vec<proto::SessionInfo> = s
                    .sessions
                    .values()
                    .map(|session| proto::SessionInfo {
                        id: uuid_to_bytes(session.id),
                        name: session.name.clone(),
                        pane_count: session.panes.len() as u32,
                        has_attached_client: session.has_attached_clients(),
                    })
                    .collect();
                Some(protocol::session_list(infos))
            }

            proto::client_message::Msg::CreateSession(req) => {
                let mut s = server.lock().await;
                let session = Session::new(req.name);
                let session_id = session.id;
                s.sessions.insert(session_id, session);
                Some(protocol::session_created(session_id))
            }

            proto::client_message::Msg::AttachSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let mut s = server.lock().await;
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(4, "session not found".into()));
                };
                session.attach_client(client_id);

                let pane_snapshots: Vec<proto::PaneSnapshot> = session
                    .panes
                    .values()
                    .map(|pane| proto::PaneSnapshot {
                        pane_id: uuid_to_bytes(pane.id),
                        title: pane.title.clone().unwrap_or_default(),
                        cwd: pane.cwd.clone().unwrap_or_default(),
                        cols: u32::from(pane.cols),
                        rows: u32::from(pane.rows),
                        scrollback: pane.screen.raw_bytes().to_vec(),
                        exit_status: pane.exit_status,
                    })
                    .collect();
                Some(protocol::snapshot(session_id, pane_snapshots))
            }

            proto::client_message::Msg::DetachSession(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let mut s = server.lock().await;
                if let Some(session) = s.sessions.get_mut(&session_id) {
                    session.detach_client(client_id);
                }
                None
            }

            proto::client_message::Msg::CreatePane(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };

                let pane_id = Uuid::new_v4();
                let pty_result = {
                    let mut s = server.lock().await;
                    let Some(session) = s.sessions.get_mut(&session_id) else {
                        return Some(protocol::error(4, "session not found".into()));
                    };
                    let pane = Pane::new(pane_id, 80, 24);
                    session.add_pane(pane);

                    let config = PaneSpawnConfig { command: vec![], cwd: None, cols: 80, rows: 24 };
                    s.engine.spawn_pane(pane_id, &config)
                };

                match pty_result {
                    Ok(pty) => {
                        let (reader, writer, child) = pty.into_parts();
                        let (kill_tx, kill_rx) = oneshot::channel();
                        {
                            let mut s = server.lock().await;
                            s.pty_writers
                                .insert(pane_id, Arc::new(tokio::sync::Mutex::new(writer)));
                            s.pty_kill_senders.insert(pane_id, kill_tx);
                        }
                        spawn_pty_read_loop(
                            Arc::clone(server),
                            session_id,
                            pane_id,
                            reader,
                            child,
                            kill_rx,
                        );
                    }
                    Err(e) => {
                        log::error!("Failed to spawn PTY for pane {pane_id}: {e}");
                    }
                }

                Some(protocol::pane_created(session_id, pane_id))
            }

            proto::client_message::Msg::ClosePane(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let mut s = server.lock().await;
                if let Some(session) = s.sessions.get_mut(&session_id) {
                    session.remove_pane(pane_id);
                }
                s.pty_writers.remove(&pane_id);
                if let Some(kill_tx) = s.pty_kill_senders.remove(&pane_id) {
                    let _ = kill_tx.send(());
                }
                Some(protocol::pane_closed(session_id, pane_id))
            }

            proto::client_message::Msg::Input(req) => {
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let writer = {
                    let s = server.lock().await;
                    s.pty_writers.get(&pane_id).cloned()
                };
                if let Some(writer) = writer {
                    let mut w = writer.lock().await;
                    if let Err(e) = w.write_all(&req.data).await {
                        log::error!("Failed to write to PTY {pane_id}: {e}");
                    }
                    if let Err(e) = w.flush().await {
                        log::error!("Failed to flush PTY {pane_id}: {e}");
                    }
                }
                None
            }

            proto::client_message::Msg::Resize(req) => {
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let cols = req.cols as u16;
                let rows = req.rows as u16;

                let writer = {
                    let mut s = server.lock().await;
                    if let Some(session) = s.sessions.get_mut(&session_id)
                        && let Some(pane) = session.panes.get_mut(&pane_id)
                    {
                        pane.cols = cols;
                        pane.rows = rows;
                    }
                    s.pty_writers.get(&pane_id).cloned()
                };
                if let Some(writer) = writer {
                    let w = writer.lock().await;
                    if let Err(e) = w.resize(pty_process::Size::new(rows, cols)) {
                        log::error!("Failed to resize PTY {pane_id}: {e}");
                    }
                }
                None
            }

            proto::client_message::Msg::SetPaneTitle(req) => {
                let session_id = match bytes_to_uuid(&req.session_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let pane_id = match bytes_to_uuid(&req.pane_id) {
                    Ok(id) => id,
                    Err(e) => return Some(protocol::error(3, e.to_string())),
                };
                let mut s = server.lock().await;
                if let Some(session) = s.sessions.get_mut(&session_id)
                    && let Some(pane) = session.panes.get_mut(&pane_id)
                {
                    pane.title = Some(req.title.clone());
                }
                Some(protocol::title_changed(session_id, pane_id, req.title))
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
                            log::error!("PTY read error for pane {pane_id}: {e}");
                            break;
                        }
                    }
                }
                _ = &mut kill_rx => {
                    let _ = child.start_kill();
                    log::info!("PTY read loop cancelled for pane {pane_id}");
                    return;
                }
            }
        }

        // Child exited naturally — collect exit status.
        let status = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(e) => {
                log::error!("Failed to wait on child for pane {pane_id}: {e}");
                -1
            }
        };

        let mut s = server.lock().await;
        if let Some(session) = s.sessions.get_mut(&session_id)
            && let Some(pane) = session.panes.get_mut(&pane_id)
        {
            pane.set_exited(status);
        }
        let msg = protocol::pane_exited(session_id, pane_id, status);
        s.broadcast_to_session(session_id, &msg);
        s.pty_writers.remove(&pane_id);
        s.pty_kill_senders.remove(&pane_id);
        drop(s);

        log::info!("PTY exited for pane {pane_id}, status {status}");
    });
}

/// Run the serialization loop, writing state to disk every `interval`.
pub async fn serialization_loop(server: Arc<Mutex<Server>>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        let mut s = server.lock().await;
        let cache_dir = s.os.cache_dir();

        // Flush scrollback for all panes in all sessions.
        let session_ids: Vec<_> = s.sessions.keys().copied().collect();
        for session_id in session_ids {
            if let Some(session) = s.sessions.get_mut(&session_id) {
                for pane in session.panes.values_mut() {
                    if let Err(e) = pane.flush_scrollback(&cache_dir, session_id) {
                        log::error!("Failed to flush scrollback for pane {}: {e}", pane.id);
                    }
                }
            }
        }

        let snapshot = s.build_snapshot();
        let state_path = default_state_path(&cache_dir);
        drop(s);

        if let Err(e) = write_state_atomic(&snapshot, &state_path) {
            log::error!("Failed to serialize state: {e}");
        }
    }
}

/// Run the main server loop: accept clients, handle messages, manage PTYs.
pub async fn run(server: Arc<Mutex<Server>>) -> anyhow::Result<()> {
    let socket_path = {
        let s = server.lock().await;
        s.os.runtime_dir().join("rttx-server.sock")
    };

    let listener = Listener::bind(&socket_path)?;
    log::info!("Listening on {}", socket_path.display());

    // Start serialization loop.
    let ser_server = Arc::clone(&server);
    tokio::spawn(async move {
        serialization_loop(ser_server, Duration::from_secs(1)).await;
    });

    loop {
        let conn = listener.accept().await?;
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            if let Err(e) = handle_client(server, conn).await {
                log::error!("Client error: {e}");
            }
        });
    }
}

#[allow(clippy::significant_drop_tightening)]
async fn handle_client(
    server: Arc<Mutex<Server>>,
    mut conn: ClientConnection,
) -> anyhow::Result<()> {
    let client_id = Uuid::new_v4();
    log::info!("Client {client_id} connected");

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
                        log::info!("Client {client_id} disconnected");
                        break;
                    };

                    // Check for shutdown.
                    if matches!(msg.msg, Some(proto::client_message::Msg::Shutdown(_))) {
                        log::info!("Shutdown requested by client {client_id}");
                        let s = server.lock().await;
                        let snapshot = s.build_snapshot();
                        let state_path = default_state_path(&s.os.cache_dir());
                        drop(s);
                        let _ = write_state_atomic(&snapshot, &state_path);
                        std::process::exit(0);
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
            session.detach_client(client_id);
        }
    }

    result
}

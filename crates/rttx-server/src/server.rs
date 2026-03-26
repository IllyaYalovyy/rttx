//! Top-level server loop.
//!
//! Accepts client connections, routes messages to sessions/panes, runs the
//! serialization loop, and manages the PTY read loops.

use crate::engine::Engine;
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
use tokio::sync::Mutex;
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

    /// Build a serializable snapshot of the current state.
    #[must_use]
    pub fn build_snapshot(&self) -> ServerState {
        ServerState {
            sessions: self.sessions.values().map(Session::to_persisted).collect(),
            serialized_at: std::time::SystemTime::now(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
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
                let mut s = server.lock().await;
                let Some(session) = s.sessions.get_mut(&session_id) else {
                    return Some(protocol::error(4, "session not found".into()));
                };
                let pane_id = Uuid::new_v4();
                let pane = Pane::new(pane_id, 80, 24);
                session.add_pane(pane);
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
                Some(protocol::pane_closed(session_id, pane_id))
            }

            proto::client_message::Msg::Input(_) | proto::client_message::Msg::Resize(_) => {
                // Input and Resize are handled by the PTY task, not here.
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

/// Run the serialization loop, writing state to disk every `interval`.
pub async fn serialization_loop(server: Arc<Mutex<Server>>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        let s = server.lock().await;
        let snapshot = s.build_snapshot();
        let state_path = default_state_path(&s.os.cache_dir());
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

    loop {
        let Some(msg) = conn.read_message().await? else {
            log::info!("Client {client_id} disconnected");
            let mut s = server.lock().await;
            for session in s.sessions.values_mut() {
                session.detach_client(client_id);
            }
            return Ok(());
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
}

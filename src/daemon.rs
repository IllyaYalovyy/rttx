//! Async connection manager for the rttxd persistent session daemon.
//!
//! Provides a `DaemonConnection` that communicates with rttxd over a Unix
//! socket using the length-prefixed protobuf framing from `rttx-proto`.
//! This module has no GTK dependency — it is pure async Rust.

use bytes::BytesMut;
use rttx_proto::{
    PROTOCOL_VERSION, bytes_to_uuid, decode_frame, encode_frame, proto, uuid_to_bytes,
};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use uuid::Uuid;

/// Default socket path for the local rttxd instance.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from)
        .join("rttx-server")
        .join("v1")
        .join("rttx-server.sock")
}

/// Errors from daemon communication.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// I/O error on the Unix socket.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Protocol framing error.
    #[error("frame: {0}")]
    Frame(#[from] rttx_proto::FrameError),

    /// Protocol version mismatch.
    #[error("protocol version mismatch: server={server}, client={client}")]
    VersionMismatch {
        /// Server's protocol version.
        server: u32,
        /// Client's protocol version.
        client: u32,
    },

    /// Server returned an error message.
    #[error("server error ({code}): {message}")]
    ServerError {
        /// Error code from the server.
        code: u32,
        /// Human-readable error message.
        message: String,
    },

    /// Unexpected message type received.
    #[error("unexpected message from server")]
    UnexpectedMessage,

    /// Connection was closed by the server.
    #[error("connection closed")]
    Disconnected,
}

/// A connection to a running rttxd instance.
pub struct DaemonConnection {
    stream: UnixStream,
    read_buf: BytesMut,
    client_id: Uuid,
}

impl std::fmt::Debug for DaemonConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonConnection")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl DaemonConnection {
    /// Connect to rttxd at the given socket path and perform the handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket is unreachable, the handshake fails,
    /// or the protocol version is incompatible.
    pub async fn connect(socket_path: &Path) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path).await?;
        let client_id = Uuid::new_v4();
        let mut conn = Self { stream, read_buf: BytesMut::with_capacity(8192), client_id };
        conn.handshake().await?;
        Ok(conn)
    }

    /// Send a client message to the daemon.
    async fn send(&mut self, msg: &proto::ClientMessage) -> Result<(), DaemonError> {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Read the next server message. Returns `None` on clean disconnect.
    pub async fn recv(&mut self) -> Result<Option<proto::ServerMessage>, DaemonError> {
        loop {
            match decode_frame::<proto::ServerMessage>(&mut self.read_buf) {
                Ok(msg) => return Ok(Some(msg)),
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => return Err(DaemonError::Frame(e)),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
        }
    }

    /// Perform the Hello/HelloAck handshake.
    async fn handshake(&mut self) -> Result<(), DaemonError> {
        let hello = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Hello(proto::Hello {
                protocol_version: PROTOCOL_VERSION,
                client_id: uuid_to_bytes(self.client_id),
            })),
        };
        self.send(&hello).await?;

        let response = self.recv().await?.ok_or(DaemonError::Disconnected)?;
        match response.msg {
            Some(proto::server_message::Msg::HelloAck(ack)) => {
                if ack.protocol_version != PROTOCOL_VERSION {
                    return Err(DaemonError::VersionMismatch {
                        server: ack.protocol_version,
                        client: PROTOCOL_VERSION,
                    });
                }
                Ok(())
            }
            Some(proto::server_message::Msg::Error(e)) => {
                Err(DaemonError::ServerError { code: e.code, message: e.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// List all sessions on the daemon.
    pub async fn list_sessions(&mut self) -> Result<Vec<proto::SessionInfo>, DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        };
        self.send(&msg).await?;

        let response = self.recv().await?.ok_or(DaemonError::Disconnected)?;
        match response.msg {
            Some(proto::server_message::Msg::SessionList(list)) => Ok(list.sessions),
            Some(proto::server_message::Msg::Error(e)) => {
                Err(DaemonError::ServerError { code: e.code, message: e.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Create a new session and return its UUID.
    pub async fn create_session(&mut self, name: &str) -> Result<Uuid, DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.to_string(),
            })),
        };
        self.send(&msg).await?;

        let response = self.recv().await?.ok_or(DaemonError::Disconnected)?;
        match response.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => {
                bytes_to_uuid(&created.session_id).map_err(DaemonError::Frame)
            }
            Some(proto::server_message::Msg::Error(e)) => {
                Err(DaemonError::ServerError { code: e.code, message: e.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Attach to a session and receive the initial snapshot.
    pub async fn attach_session(
        &mut self,
        session_id: Uuid,
    ) -> Result<proto::Snapshot, DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: uuid_to_bytes(session_id),
            })),
        };
        self.send(&msg).await?;

        let response = self.recv().await?.ok_or(DaemonError::Disconnected)?;
        match response.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => Ok(snapshot),
            Some(proto::server_message::Msg::Error(e)) => {
                Err(DaemonError::ServerError { code: e.code, message: e.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Create a pane in a session and return the new pane UUID.
    pub async fn create_pane(&mut self, session_id: Uuid) -> Result<Uuid, DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: uuid_to_bytes(session_id),
            })),
        };
        self.send(&msg).await?;

        let response = self.recv().await?.ok_or(DaemonError::Disconnected)?;
        match response.msg {
            Some(proto::server_message::Msg::PaneCreated(created)) => {
                bytes_to_uuid(&created.pane_id).map_err(DaemonError::Frame)
            }
            Some(proto::server_message::Msg::Error(e)) => {
                Err(DaemonError::ServerError { code: e.code, message: e.message })
            }
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Send keyboard input to a pane.
    pub async fn send_input(
        &mut self,
        session_id: Uuid,
        pane_id: Uuid,
        data: &[u8],
    ) -> Result<(), DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: uuid_to_bytes(session_id),
                pane_id: uuid_to_bytes(pane_id),
                data: data.to_vec(),
            })),
        };
        self.send(&msg).await
    }

    /// Notify the daemon of a pane resize.
    pub async fn send_resize(
        &mut self,
        session_id: Uuid,
        pane_id: Uuid,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                session_id: uuid_to_bytes(session_id),
                pane_id: uuid_to_bytes(pane_id),
                cols: u32::from(cols),
                rows: u32::from(rows),
            })),
        };
        self.send(&msg).await
    }

    /// Detach from a session without killing it.
    pub async fn detach_session(&mut self, session_id: Uuid) -> Result<(), DaemonError> {
        let msg = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: uuid_to_bytes(session_id),
            })),
        };
        self.send(&msg).await
    }
}

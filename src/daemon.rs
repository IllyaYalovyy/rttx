//! Async connection manager for the rttxd persistent session daemon.
//!
//! Provides a `DaemonConnection` that communicates with rttxd over a Unix
//! socket or SSH subprocess using the length-prefixed protobuf framing
//! from `rttx-proto`. After the handshake, the connection can be split
//! into a `DaemonReader` and `DaemonWriter` for concurrent read/write
//! from the glib main loop. This module has no GTK dependency.

use bytes::BytesMut;
use rttx_proto::{
    PROTOCOL_VERSION, bytes_to_uuid, decode_frame, encode_frame, proto, uuid_to_bytes,
};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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

/// A connection to a running rttxd instance (pre-split).
///
/// Used for the handshake and initial request/response exchanges
/// (create session, attach, create pane). Once setup is complete,
/// call [`into_split`](DaemonConnection::into_split) to get separate
/// reader and writer halves for concurrent use.
pub struct DaemonConnection {
    reader: Box<dyn AsyncRead + Unpin + Send>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
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
    /// Connect to rttxd at the given Unix socket path and perform the handshake.
    pub async fn connect(socket_path: &Path) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (read_half, write_half) = stream.into_split();
        let mut conn = Self {
            reader: Box::new(read_half),
            writer: Box::new(write_half),
            read_buf: BytesMut::with_capacity(8192),
            client_id: Uuid::new_v4(),
        };
        conn.handshake().await?;
        Ok(conn)
    }

    /// Connect to rttxd on a remote host via SSH.
    ///
    /// Spawns `ssh <host> rttx-server attach-stdio` and speaks the protocol
    /// over the subprocess's stdin/stdout. The returned `SshHandle` must be
    /// kept alive for the connection to persist.
    pub async fn connect_ssh(host: &str) -> Result<(Self, SshHandle), DaemonError> {
        let mut child = tokio::process::Command::new("ssh")
            .arg(host)
            .arg("rttx-server")
            .arg("attach-stdio")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let child_stdin =
            child.stdin.take().ok_or_else(|| DaemonError::Io(std::io::Error::other("no stdin")))?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| DaemonError::Io(std::io::Error::other("no stdout")))?;

        let mut conn = Self {
            reader: Box::new(child_stdout),
            writer: Box::new(child_stdin),
            read_buf: BytesMut::with_capacity(8192),
            client_id: Uuid::new_v4(),
        };
        conn.handshake().await?;
        Ok((conn, SshHandle { child }))
    }

    /// Split into independent reader and writer halves.
    #[must_use]
    pub fn into_split(self) -> (DaemonReader, DaemonWriter) {
        (
            DaemonReader { stream: self.reader, read_buf: self.read_buf },
            DaemonWriter { stream: self.writer },
        )
    }

    /// Send a client message to the daemon.
    async fn send(&mut self, msg: &proto::ClientMessage) -> Result<(), DaemonError> {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf)?;
        self.writer.write_all(&buf).await?;
        self.writer.flush().await?;
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
            let n = self.reader.read_buf(&mut self.read_buf).await?;
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
}

/// Handle to the SSH subprocess. Must be kept alive for the connection
/// to persist. Dropping it kills the SSH process.
pub struct SshHandle {
    child: tokio::process::Child,
}

impl std::fmt::Debug for SshHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshHandle").finish_non_exhaustive()
    }
}

impl SshHandle {
    /// Kill the SSH subprocess.
    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl Drop for SshHandle {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Write half of a split daemon connection.
///
/// Used by input/resize/detach handlers. Shared via `Rc<RefCell<>>` on
/// the glib main thread.
pub struct DaemonWriter {
    stream: Box<dyn AsyncWrite + Unpin + Send>,
}

impl std::fmt::Debug for DaemonWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonWriter").finish_non_exhaustive()
    }
}

impl DaemonWriter {
    /// Send keyboard input to a pane.
    pub async fn send_input(
        &mut self,
        session_id: Uuid,
        pane_id: Uuid,
        data: &[u8],
    ) -> Result<(), DaemonError> {
        self.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: uuid_to_bytes(session_id),
                pane_id: uuid_to_bytes(pane_id),
                data: data.to_vec(),
            })),
        })
        .await
    }

    /// Notify the daemon of a pane resize.
    pub async fn send_resize(
        &mut self,
        session_id: Uuid,
        pane_id: Uuid,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        self.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                session_id: uuid_to_bytes(session_id),
                pane_id: uuid_to_bytes(pane_id),
                cols: u32::from(cols),
                rows: u32::from(rows),
            })),
        })
        .await
    }

    /// Detach from a session without killing it.
    pub async fn detach_session(&mut self, session_id: Uuid) -> Result<(), DaemonError> {
        self.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: uuid_to_bytes(session_id),
            })),
        })
        .await
    }

    async fn send(&mut self, msg: &proto::ClientMessage) -> Result<(), DaemonError> {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf)?;
        self.stream.write_all(&buf).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

/// Read half of a split daemon connection.
///
/// Owned by the single reader loop that dispatches server messages
/// to the correct `PersistentPaneView` by `pane_id`.
pub struct DaemonReader {
    stream: Box<dyn AsyncRead + Unpin + Send>,
    read_buf: BytesMut,
}

impl std::fmt::Debug for DaemonReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonReader").finish_non_exhaustive()
    }
}

impl DaemonReader {
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
}

/// Extract the `pane_id` bytes from a server message, if present.
#[must_use]
pub fn extract_pane_id(msg: &proto::ServerMessage) -> Option<Uuid> {
    use proto::server_message::Msg;
    let bytes = match msg.msg.as_ref()? {
        Msg::Delta(m) => &m.pane_id,
        Msg::PaneExited(m) => &m.pane_id,
        Msg::PaneCreated(m) => &m.pane_id,
        Msg::PaneClosed(m) => &m.pane_id,
        Msg::TitleChanged(m) => &m.pane_id,
        Msg::CwdChanged(m) => &m.pane_id,
        Msg::Bell(m) => &m.pane_id,
        Msg::HelloAck(_)
        | Msg::SessionList(_)
        | Msg::SessionCreated(_)
        | Msg::Snapshot(_)
        | Msg::Error(_) => return None,
    };
    bytes_to_uuid(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rttx_proto::{decode_frame, encode_frame, proto};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    /// Accept one client, read `Hello`, send `HelloAck`.
    async fn serve_handshake(listener: &UnixListener) -> UnixStream {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await.unwrap();
            assert!(n > 0, "client disconnected before Hello");
            if let Ok(_hello) = decode_frame::<proto::ClientMessage>(&mut buf) {
                break;
            }
        }
        let ack = proto::ServerMessage {
            msg: Some(proto::server_message::Msg::HelloAck(proto::HelloAck {
                protocol_version: PROTOCOL_VERSION,
                server_id: uuid_to_bytes(Uuid::new_v4()),
            })),
        };
        let mut out = BytesMut::new();
        encode_frame(&ack, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();
        stream
    }

    #[test]
    fn default_socket_path_contains_version() {
        let path = default_socket_path();
        assert!(path.to_string_lossy().contains("v1"));
        assert!(path.to_string_lossy().contains("rttx-server"));
    }

    #[tokio::test]
    async fn connect_to_nonexistent_socket_returns_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("nonexistent.sock");
        let result = DaemonConnection::connect(&sock).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handshake_succeeds_with_matching_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move { DaemonConnection::connect(&sock).await }
        });

        let _server_stream = serve_handshake(&listener).await;
        let conn = client_task.await.unwrap().unwrap();
        assert!(!conn.client_id.is_nil());
    }

    #[tokio::test]
    async fn handshake_fails_on_version_mismatch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move { DaemonConnection::connect(&sock).await }
        });

        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await.unwrap();
            assert!(n > 0);
            if decode_frame::<proto::ClientMessage>(&mut buf).is_ok() {
                break;
            }
        }
        let ack = proto::ServerMessage {
            msg: Some(proto::server_message::Msg::HelloAck(proto::HelloAck {
                protocol_version: 999,
                server_id: uuid_to_bytes(Uuid::new_v4()),
            })),
        };
        let mut out = BytesMut::new();
        encode_frame(&ack, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();

        let result = client_task.await.unwrap();
        assert!(matches!(result, Err(DaemonError::VersionMismatch { server: 999, .. })));
    }

    #[tokio::test]
    async fn handshake_fails_on_server_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move { DaemonConnection::connect(&sock).await }
        });

        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await.unwrap();
            assert!(n > 0);
            if decode_frame::<proto::ClientMessage>(&mut buf).is_ok() {
                break;
            }
        }
        let err = proto::ServerMessage {
            msg: Some(proto::server_message::Msg::Error(proto::Error {
                code: 2,
                message: "version mismatch".into(),
            })),
        };
        let mut out = BytesMut::new();
        encode_frame(&err, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();

        let result = client_task.await.unwrap();
        assert!(matches!(result, Err(DaemonError::ServerError { code: 2, .. })));
    }

    #[tokio::test]
    async fn recv_returns_none_on_clean_disconnect() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let mut conn = DaemonConnection::connect(&sock).await.unwrap();
                conn.recv().await
            }
        });

        let server_stream = serve_handshake(&listener).await;
        drop(server_stream);

        let result = client_task.await.unwrap().unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn send_input_encodes_correctly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let session_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let conn = DaemonConnection::connect(&sock).await.unwrap();
                let (_reader, mut writer) = conn.into_split();
                writer.send_input(session_id, pane_id, b"hello").await.unwrap();
            }
        });

        let mut server_stream = serve_handshake(&listener).await;
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = server_stream.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            if let Ok(msg) = decode_frame::<proto::ClientMessage>(&mut buf) {
                match msg.msg {
                    Some(proto::client_message::Msg::Input(input)) => {
                        assert_eq!(input.data, b"hello");
                        assert_eq!(bytes_to_uuid(&input.session_id).unwrap(), session_id);
                        assert_eq!(bytes_to_uuid(&input.pane_id).unwrap(), pane_id);
                    }
                    other => panic!("expected Input, got {other:?}"),
                }
                break;
            }
        }

        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn send_resize_encodes_correctly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let session_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let conn = DaemonConnection::connect(&sock).await.unwrap();
                let (_reader, mut writer) = conn.into_split();
                writer.send_resize(session_id, pane_id, 120, 40).await.unwrap();
            }
        });

        let mut server_stream = serve_handshake(&listener).await;
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = server_stream.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            if let Ok(msg) = decode_frame::<proto::ClientMessage>(&mut buf) {
                match msg.msg {
                    Some(proto::client_message::Msg::Resize(resize)) => {
                        assert_eq!(resize.cols, 120);
                        assert_eq!(resize.rows, 40);
                    }
                    other => panic!("expected Resize, got {other:?}"),
                }
                break;
            }
        }

        client_task.await.unwrap();
    }

    #[test]
    fn extract_pane_id_from_delta() {
        let pane_id = Uuid::new_v4();
        let msg = proto::ServerMessage {
            msg: Some(proto::server_message::Msg::Delta(proto::Delta {
                session_id: uuid_to_bytes(Uuid::new_v4()),
                pane_id: uuid_to_bytes(pane_id),
                data: vec![],
            })),
        };
        assert_eq!(extract_pane_id(&msg), Some(pane_id));
    }

    #[test]
    fn extract_pane_id_from_error_is_none() {
        let msg = proto::ServerMessage {
            msg: Some(proto::server_message::Msg::Error(proto::Error {
                code: 1,
                message: "test".into(),
            })),
        };
        assert_eq!(extract_pane_id(&msg), None);
    }
}

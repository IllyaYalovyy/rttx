//! Async connection manager for the `rttx-server` persistent-session daemon.
//!
//! Provides a `DaemonConnection` that communicates with `rttx-server` over a Unix
//! socket or SSH subprocess using the length-prefixed protobuf framing
//! from `rttx-proto`. After the handshake, the connection can be split
//! into a `DaemonReader` and `DaemonWriter` for concurrent read/write
//! from the glib main loop. This module has no GTK dependency.

use bytes::BytesMut;
use rttx_proto::{bytes_to_uuid, decode_frame, encode_frame, uuid_to_bytes, v3, v3_handshake};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use uuid::Uuid;

use crate::runtime::WorkspacePolicy;

/// Default socket path for the local `rttx-server` instance.
///
/// In dev mode (`RTTX_DEV_MODE=1`), uses `rttx-server-devel` instead of
/// `rttx-server` so the development daemon runs alongside production.
#[must_use]
pub fn default_socket_path() -> PathBuf {
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from);
    socket_path_for(&runtime_dir, crate::config::is_development())
}

#[must_use]
fn socket_path_for(runtime_dir: &Path, is_dev: bool) -> PathBuf {
    let dir_name = if is_dev { "rttx-server-devel" } else { "rttx-server" };
    runtime_dir.join(dir_name).join("v1").join("rttx-server.sock")
}

/// Return the daemon binary name for the current mode.
#[must_use]
pub const fn daemon_binary() -> &'static str {
    "rttx-server"
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

    /// Server returned a typed v3 protocol error.
    #[error("server error: {message}")]
    ProtocolError {
        /// Error kind from the server.
        kind: v3::ErrorKind,
        /// Human-readable error message.
        message: String,
        /// Whether the error is retryable.
        retryable: bool,
    },

    /// Server returned a legacy v2 error message.
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

    /// Attach was blocked because another client already owns the runtime.
    #[error("runtime attach blocked")]
    AttachBlocked(v3::AttachBlocked),

    /// Connection was closed by the server.
    #[error("connection closed")]
    Disconnected,
}

/// Successful detach outcome from the daemon.
#[derive(Debug, Clone)]
pub enum DetachResponse {
    Detached(v3::RuntimeDetached),
    Terminated(v3::RuntimeTerminated),
}

/// Client capabilities advertised during v3 handshake.
const CLIENT_CAPABILITIES: &[v3::Capability] = &[
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

/// A connection to a running `rttx-server` instance (pre-split).
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
    effective_caps: Vec<i32>,
    id_gen: RequestIdGenerator,
}

impl std::fmt::Debug for DaemonConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonConnection")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// Atomic request ID generator for v3 envelope correlation.
#[derive(Debug)]
struct RequestIdGenerator {
    next: AtomicU64,
}

impl RequestIdGenerator {
    const fn new() -> Self {
        Self { next: AtomicU64::new(1) }
    }

    fn next_id(&self) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if id == 0 { self.next.fetch_add(1, Ordering::Relaxed) } else { id }
    }
}

impl DaemonConnection {
    /// Connect to `rttx-server` at the given Unix socket path and perform the handshake.
    pub async fn connect(socket_path: &Path) -> Result<Self, DaemonError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (read_half, write_half) = stream.into_split();
        let mut conn = Self {
            reader: Box::new(read_half),
            writer: Box::new(write_half),
            read_buf: BytesMut::with_capacity(8192),
            client_id: Uuid::new_v4(),
            effective_caps: Vec::new(),
            id_gen: RequestIdGenerator::new(),
        };
        conn.handshake().await?;
        Ok(conn)
    }

    /// Connect to `rttx-server` on a remote host via SSH.
    pub async fn connect_ssh(host: &str) -> Result<(Self, SshHandle), DaemonError> {
        let mut child = tokio::process::Command::new("ssh")
            .args(["-o", "BatchMode=yes"])
            .args(["-o", "ConnectTimeout=10"])
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
            effective_caps: Vec::new(),
            id_gen: RequestIdGenerator::new(),
        };
        conn.handshake().await?;
        Ok((conn, SshHandle { child }))
    }

    /// Split into independent reader and writer halves.
    #[must_use]
    pub fn into_split(self) -> (DaemonReader, DaemonWriter) {
        (
            DaemonReader { stream: self.reader, read_buf: self.read_buf },
            DaemonWriter { stream: self.writer, id_gen: self.id_gen },
        )
    }

    /// Return the effective capabilities negotiated during handshake.
    #[must_use]
    pub fn effective_caps(&self) -> &[i32] {
        &self.effective_caps
    }

    /// Send a v3 client envelope to the daemon.
    pub(crate) async fn send_envelope(
        &mut self,
        env: &v3::ClientEnvelope,
    ) -> Result<(), DaemonError> {
        let mut buf = BytesMut::new();
        encode_frame(env, &mut buf)?;
        self.writer.write_all(&buf).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Read the next v3 server envelope. Returns `None` on clean disconnect.
    pub async fn recv_envelope(&mut self) -> Result<Option<v3::ServerEnvelope>, DaemonError> {
        loop {
            match decode_frame::<v3::ServerEnvelope>(&mut self.read_buf) {
                Ok(env) => return Ok(Some(env)),
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => return Err(DaemonError::Frame(e)),
            }
            let n = self.reader.read_buf(&mut self.read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
        }
    }

    /// Send a request and wait for the correlated response, dispatching
    /// push events encountered along the way.
    async fn request(
        &mut self,
        command: v3::client_envelope::Command,
    ) -> Result<v3::ServerEnvelope, DaemonError> {
        let request_id = self.id_gen.next_id();
        let env = v3::ClientEnvelope { request_id, command: Some(command) };
        self.send_envelope(&env).await?;
        loop {
            let response = self.recv_envelope().await?.ok_or(DaemonError::Disconnected)?;
            if response.request_id == request_id {
                return Ok(response);
            }
            // Push event (request_id == 0) — skip during request/response.
        }
    }

    /// Perform the v3 `ClientHello`/`ServerHello` handshake.
    async fn handshake(&mut self) -> Result<(), DaemonError> {
        let client_hello = v3_handshake::build_client_hello(
            self.client_id,
            "rttx",
            env!("CARGO_PKG_VERSION"),
            CLIENT_CAPABILITIES,
        );
        let mut buf = BytesMut::new();
        encode_frame(&client_hello, &mut buf)?;
        self.writer.write_all(&buf).await?;
        self.writer.flush().await?;

        // Server responds with bare ServerHello (not wrapped in ServerEnvelope).
        // Read the raw frame and try to decode as ServerHello or ProtocolError.
        let raw = self.read_raw_frame().await?.ok_or(DaemonError::Disconnected)?;
        let payload = &raw[4..]; // skip 4-byte length prefix

        // Try ServerHello first.
        if let Ok(server_hello) = <v3::ServerHello as prost::Message>::decode(payload) {
            if let Err(missing) =
                v3_handshake::validate_server_capabilities(&server_hello.capabilities)
            {
                let err = v3_handshake::missing_capabilities_error(&missing);
                return Err(DaemonError::ProtocolError {
                    kind: v3::ErrorKind::try_from(err.kind).unwrap_or(v3::ErrorKind::Unspecified),
                    message: err.message,
                    retryable: false,
                });
            }
            let client_caps: Vec<i32> = CLIENT_CAPABILITIES.iter().map(|c| *c as i32).collect();
            self.effective_caps =
                v3_handshake::effective_capabilities(&client_caps, &server_hello.capabilities);
            return Ok(());
        }

        // Try ProtocolError.
        if let Ok(err) = <v3::ProtocolError as prost::Message>::decode(payload) {
            return Err(DaemonError::ProtocolError {
                kind: v3::ErrorKind::try_from(err.kind).unwrap_or(v3::ErrorKind::Unspecified),
                message: err.message,
                retryable: err.retryable,
            });
        }

        Err(DaemonError::UnexpectedMessage)
    }

    /// Read a raw length-prefixed frame without decoding.
    async fn read_raw_frame(&mut self) -> Result<Option<BytesMut>, DaemonError> {
        loop {
            if self.read_buf.len() >= 4 {
                let len = u32::from_le_bytes([
                    self.read_buf[0],
                    self.read_buf[1],
                    self.read_buf[2],
                    self.read_buf[3],
                ]);
                if len > rttx_proto::MAX_MESSAGE_SIZE {
                    return Err(DaemonError::Frame(rttx_proto::FrameError::TooLarge(len)));
                }
                let total = 4 + len as usize;
                if self.read_buf.len() >= total {
                    let frame = self.read_buf.split_to(total);
                    return Ok(Some(frame));
                }
            }
            let n = self.reader.read_buf(&mut self.read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
        }
    }

    /// List all runtimes on the daemon.
    pub async fn list_runtimes(&mut self) -> Result<Vec<v3::RuntimeInfo>, DaemonError> {
        let response =
            self.request(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})).await?;
        match response.payload {
            Some(v3::server_envelope::Payload::RuntimeList(list)) => Ok(list.runtimes),
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Create a new runtime and return its UUID.
    pub async fn create_runtime(
        &mut self,
        name: &str,
        policy: WorkspacePolicy,
    ) -> Result<Uuid, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: name.to_string(),
                policy: policy.as_v3_proto(),
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::RuntimeCreated(created)) => {
                bytes_to_uuid(&created.runtime_id).map_err(DaemonError::Frame)
            }
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Attach to a runtime and receive the initial snapshot.
    pub async fn attach_runtime(
        &mut self,
        runtime_id: Uuid,
        attach_mode: v3::RuntimeAttachMode,
    ) -> Result<v3::RuntimeSnapshot, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: uuid_to_bytes(runtime_id),
                attach_mode: attach_mode as i32,
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => Ok(snapshot),
            Some(v3::server_envelope::Payload::AttachBlocked(blocked)) => {
                Err(DaemonError::AttachBlocked(blocked))
            }
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Create a pane in a runtime and return the new pane UUID.
    pub async fn create_pane(
        &mut self,
        runtime_id: Uuid,
        cwd: Option<String>,
        dark_background: Option<bool>,
        cols: u32,
        rows: u32,
        no_persist: bool,
    ) -> Result<Uuid, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: uuid_to_bytes(runtime_id),
                cwd,
                dark_background,
                cols,
                rows,
                no_persist: if no_persist { Some(true) } else { None },
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::PaneCreated(created)) => {
                bytes_to_uuid(&created.pane_id).map_err(DaemonError::Frame)
            }
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Close a pane in a runtime.
    pub async fn close_pane(
        &mut self,
        runtime_id: Uuid,
        pane_id: Uuid,
    ) -> Result<v3::PaneClosed, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::ClosePane(v3::ClosePane {
                runtime_id: uuid_to_bytes(runtime_id),
                pane_id: uuid_to_bytes(pane_id),
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::PaneClosed(closed)) => Ok(closed),
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Detach from a runtime explicitly.
    pub async fn detach_runtime(
        &mut self,
        runtime_id: Uuid,
    ) -> Result<DetachResponse, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: uuid_to_bytes(runtime_id),
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::RuntimeDetached(detached)) => {
                Ok(DetachResponse::Detached(detached))
            }
            Some(v3::server_envelope::Payload::RuntimeTerminated(terminated)) => {
                Ok(DetachResponse::Terminated(terminated))
            }
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }

    /// Terminate a runtime explicitly.
    pub async fn terminate_runtime(
        &mut self,
        runtime_id: Uuid,
    ) -> Result<v3::RuntimeTerminated, DaemonError> {
        let response = self
            .request(v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
                runtime_id: uuid_to_bytes(runtime_id),
            }))
            .await?;
        match response.payload {
            Some(v3::server_envelope::Payload::RuntimeTerminated(terminated)) => Ok(terminated),
            Some(v3::server_envelope::Payload::Error(e)) => Err(protocol_error(e)),
            _ => Err(DaemonError::UnexpectedMessage),
        }
    }
}

/// Convert a v3 `ProtocolError` to a `DaemonError`.
fn protocol_error(e: v3::ProtocolError) -> DaemonError {
    DaemonError::ProtocolError {
        kind: v3::ErrorKind::try_from(e.kind).unwrap_or(v3::ErrorKind::Unspecified),
        message: e.message,
        retryable: e.retryable,
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
    id_gen: RequestIdGenerator,
}

impl std::fmt::Debug for DaemonWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonWriter").finish_non_exhaustive()
    }
}

impl DaemonWriter {
    /// Send keyboard input to a pane (fire-and-forget).
    pub async fn send_input(
        &mut self,
        runtime_id: Uuid,
        pane_id: Uuid,
        data: &[u8],
    ) -> Result<(), DaemonError> {
        let env = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: uuid_to_bytes(runtime_id),
                pane_id: uuid_to_bytes(pane_id),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::copy_from_slice(data),
                })),
            })),
        };
        self.send_envelope(&env).await
    }

    /// Notify the daemon of a pane resize (fire-and-forget).
    pub async fn send_resize(
        &mut self,
        runtime_id: Uuid,
        pane_id: Uuid,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        let env = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                runtime_id: uuid_to_bytes(runtime_id),
                pane_id: uuid_to_bytes(pane_id),
                cols: u32::from(cols),
                rows: u32::from(rows),
            })),
        };
        self.send_envelope(&env).await
    }

    /// Send a heartbeat ping to the daemon.
    pub async fn send_ping(&mut self, nonce: u64) -> Result<(), DaemonError> {
        let env = v3::ClientEnvelope {
            request_id: self.id_gen.next_id(),
            command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce })),
        };
        self.send_envelope(&env).await
    }

    /// Detach from a runtime without killing it.
    pub async fn detach_runtime(&mut self, runtime_id: Uuid) -> Result<(), DaemonError> {
        let env = v3::ClientEnvelope {
            request_id: self.id_gen.next_id(),
            command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: uuid_to_bytes(runtime_id),
            })),
        };
        self.send_envelope(&env).await
    }

    pub(crate) async fn send_envelope(
        &mut self,
        env: &v3::ClientEnvelope,
    ) -> Result<(), DaemonError> {
        let mut buf = BytesMut::new();
        encode_frame(env, &mut buf)?;
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
    /// Read the next v3 server envelope. Returns `None` on clean disconnect.
    pub async fn recv(&mut self) -> Result<Option<v3::ServerEnvelope>, DaemonError> {
        loop {
            match decode_frame::<v3::ServerEnvelope>(&mut self.read_buf) {
                Ok(env) => return Ok(Some(env)),
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

#[cfg(test)]
pub(crate) fn split_transport_for_test<R, W>(reader: R, writer: W) -> (DaemonReader, DaemonWriter)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    (
        DaemonReader { stream: Box::new(reader), read_buf: BytesMut::new() },
        DaemonWriter { stream: Box::new(writer), id_gen: RequestIdGenerator::new() },
    )
}

/// Extract the `pane_id` bytes from a v3 server envelope, if present.
#[must_use]
pub fn extract_pane_id(env: &v3::ServerEnvelope) -> Option<Uuid> {
    use v3::server_envelope::Payload;
    let bytes = match env.payload.as_ref()? {
        Payload::OutputDelta(m) => &m.pane_id,
        Payload::PaneExited(m) => &m.pane_id,
        Payload::PaneCreated(m) => &m.pane_id,
        Payload::PaneClosed(m) => &m.pane_id,
        Payload::TitleChanged(m) => &m.pane_id,
        Payload::CwdChanged(m) => &m.pane_id,
        Payload::Bell(m) => &m.pane_id,
        Payload::PaneResized(m) => &m.pane_id,
        Payload::TerminalModeChanged(m) => &m.pane_id,
        Payload::Pong(_)
        | Payload::RuntimeList(_)
        | Payload::RuntimeCreated(_)
        | Payload::RuntimeSnapshot(_)
        | Payload::AttachBlocked(_)
        | Payload::RuntimeDetached(_)
        | Payload::RuntimeTerminated(_)
        | Payload::RuntimeRenamed(_)
        | Payload::Error(_)
        | Payload::DiagnosticsReport(_)
        | Payload::StreamOverflow(_)
        | Payload::ScrollbackChunk(_)
        | Payload::TakeoverCompleted(_)
        | Payload::LeaseLost(_)
        | Payload::OwnerDisconnected(_) => return None,
    };
    bytes_to_uuid(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rttx_proto::{decode_frame, encode_frame, v3, v3_handshake};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    /// Accept one client, read `ClientHello`, send `ServerHello`.
    async fn serve_handshake(listener: &UnixListener) -> UnixStream {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await.unwrap();
            assert!(n > 0, "client disconnected before ClientHello");
            if let Ok(_hello) = decode_frame::<v3::ClientHello>(&mut buf) {
                break;
            }
        }
        let server_hello = v3_handshake::build_server_hello(
            Uuid::new_v4(),
            "0.1.0",
            v3_handshake::V3_PROTOCOL_VERSION,
            v3_handshake::CORE_CAPABILITIES,
        );
        let mut out = BytesMut::new();
        encode_frame(&server_hello, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();
        stream
    }

    #[test]
    fn default_socket_path_contains_version() {
        let path = default_socket_path();
        assert!(path.to_string_lossy().contains("v1"));
        assert!(path.to_string_lossy().contains("rttx-server"));
    }

    #[test]
    fn socket_path_for_production_uses_production_daemon_dir() {
        let path = socket_path_for(Path::new("/tmp/runtime"), false);
        assert_eq!(path, Path::new("/tmp/runtime/rttx-server/v1/rttx-server.sock"));
    }

    #[test]
    fn socket_path_for_dev_uses_development_daemon_dir() {
        let path = socket_path_for(Path::new("/tmp/runtime"), true);
        assert_eq!(path, Path::new("/tmp/runtime/rttx-server-devel/v1/rttx-server.sock"));
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
    async fn handshake_fails_on_protocol_error() {
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
            if decode_frame::<v3::ClientHello>(&mut buf).is_ok() {
                break;
            }
        }
        let err = v3::ProtocolError {
            kind: v3::ErrorKind::ProtocolMismatch as i32,
            message: "version mismatch".into(),
            operation: "Handshake".into(),
            retryable: false,
            user_action_required: true,
            retry_after_seconds: 0,
        };
        let mut out = BytesMut::new();
        encode_frame(&err, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();

        let result = client_task.await.unwrap();
        assert!(matches!(result, Err(DaemonError::ProtocolError { .. })));
    }

    #[tokio::test]
    async fn handshake_fails_on_missing_capabilities() {
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
            if decode_frame::<v3::ClientHello>(&mut buf).is_ok() {
                break;
            }
        }
        // Send a ServerHello with no capabilities — client should reject.
        let server_hello = v3::ServerHello {
            negotiated_protocol_version: v3_handshake::V3_PROTOCOL_VERSION,
            server_id: uuid_to_bytes(Uuid::new_v4()),
            server_version: "0.1.0".into(),
            capabilities: vec![],
        };
        let mut out = BytesMut::new();
        encode_frame(&server_hello, &mut out).unwrap();
        stream.write_all(&out).await.unwrap();

        let result = client_task.await.unwrap();
        assert!(matches!(result, Err(DaemonError::ProtocolError { .. })));
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
                conn.recv_envelope().await
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

        let runtime_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let conn = DaemonConnection::connect(&sock).await.unwrap();
                let (_reader, mut writer) = conn.into_split();
                writer.send_input(runtime_id, pane_id, b"hello").await.unwrap();
            }
        });

        let mut server_stream = serve_handshake(&listener).await;
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = server_stream.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            if let Ok(env) = decode_frame::<v3::ClientEnvelope>(&mut buf) {
                match env.command {
                    Some(v3::client_envelope::Command::TerminalInput(input)) => {
                        if let Some(v3::terminal_input::Kind::Raw(raw)) = input.kind {
                            assert_eq!(raw.data, &b"hello"[..]);
                        } else {
                            panic!("expected RawInput kind");
                        }
                        assert_eq!(bytes_to_uuid(&input.runtime_id).unwrap(), runtime_id);
                        assert_eq!(bytes_to_uuid(&input.pane_id).unwrap(), pane_id);
                    }
                    other => panic!("expected TerminalInput, got {other:?}"),
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

        let runtime_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let conn = DaemonConnection::connect(&sock).await.unwrap();
                let (_reader, mut writer) = conn.into_split();
                writer.send_resize(runtime_id, pane_id, 120, 40).await.unwrap();
            }
        });

        let mut server_stream = serve_handshake(&listener).await;
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = server_stream.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            if let Ok(env) = decode_frame::<v3::ClientEnvelope>(&mut buf) {
                match env.command {
                    Some(v3::client_envelope::Command::ResizePane(resize)) => {
                        assert_eq!(resize.cols, 120);
                        assert_eq!(resize.rows, 40);
                    }
                    other => panic!("expected ResizePane, got {other:?}"),
                }
                break;
            }
        }

        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn send_ping_encodes_correctly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let client_task = tokio::spawn({
            let sock = sock.clone();
            async move {
                let conn = DaemonConnection::connect(&sock).await.unwrap();
                let (_reader, mut writer) = conn.into_split();
                writer.send_ping(99).await.unwrap();
            }
        });

        let mut server_stream = serve_handshake(&listener).await;
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = server_stream.read_buf(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            if let Ok(env) = decode_frame::<v3::ClientEnvelope>(&mut buf) {
                match env.command {
                    Some(v3::client_envelope::Command::Ping(ping)) => {
                        assert_eq!(ping.nonce, 99);
                    }
                    other => panic!("expected Ping, got {other:?}"),
                }
                break;
            }
        }

        client_task.await.unwrap();
    }

    #[test]
    fn extract_pane_id_from_delta() {
        let pane_id = Uuid::new_v4();
        let env = v3::ServerEnvelope {
            request_id: 0,
            payload: Some(v3::server_envelope::Payload::OutputDelta(v3::OutputDelta {
                runtime_id: uuid_to_bytes(Uuid::new_v4()),
                pane_id: uuid_to_bytes(pane_id),
                pane_output_seq: 1,
                data: bytes::Bytes::new(),
            })),
        };
        assert_eq!(extract_pane_id(&env), Some(pane_id));
    }

    #[test]
    fn extract_pane_id_from_error_is_none() {
        let env = v3::ServerEnvelope {
            request_id: 0,
            payload: Some(v3::server_envelope::Payload::Error(v3::ProtocolError {
                kind: v3::ErrorKind::Internal as i32,
                message: "test".into(),
                operation: String::new(),
                retryable: false,
                user_action_required: false,
                retry_after_seconds: 0,
            })),
        };
        assert_eq!(extract_pane_id(&env), None);
    }

    #[tokio::test]
    async fn connect_ssh_to_bogus_host_fails_fast() {
        let start = std::time::Instant::now();
        let result = DaemonConnection::connect_ssh("rttx-nonexistent-host-test").await;
        assert!(result.is_err(), "SSH to bogus host should fail");
        // BatchMode=yes makes SSH fail immediately instead of hanging for auth.
        assert!(start.elapsed().as_secs() < 15, "SSH should fail fast, not hang");
    }

    #[test]
    fn request_id_generator_never_returns_zero() {
        let id_gen = RequestIdGenerator::new();
        for _ in 0..1000 {
            assert_ne!(
                id_gen.next_id(),
                0,
                "request_id must never be zero (reserved for push events)"
            );
        }
    }

    #[test]
    fn client_capabilities_include_all_core_capabilities() {
        let core = v3_handshake::CORE_CAPABILITIES;
        for cap in core {
            assert!(
                CLIENT_CAPABILITIES.contains(cap),
                "CLIENT_CAPABILITIES must include core capability {cap:?}",
            );
        }
    }

    #[test]
    fn protocol_error_conversion_preserves_kind_and_retryable() {
        let err = protocol_error(v3::ProtocolError {
            kind: v3::ErrorKind::RuntimeNotFound as i32,
            message: "runtime gone".into(),
            operation: "AttachRuntime".into(),
            retryable: false,
            user_action_required: false,
            retry_after_seconds: 0,
        });
        match err {
            DaemonError::ProtocolError { kind, message, retryable } => {
                assert_eq!(kind, v3::ErrorKind::RuntimeNotFound);
                assert_eq!(message, "runtime gone");
                assert!(!retryable);
            }
            other => panic!("expected ProtocolError, got {other:?}"),
        }
    }

    #[test]
    fn extract_pane_id_from_terminal_mode_changed() {
        let pane_id = Uuid::new_v4();
        let env = v3::ServerEnvelope {
            request_id: 0,
            payload: Some(v3::server_envelope::Payload::TerminalModeChanged(
                v3::TerminalModeChanged {
                    runtime_id: uuid_to_bytes(Uuid::new_v4()),
                    pane_id: uuid_to_bytes(pane_id),
                    runtime_revision: 1,
                    modes: None,
                },
            )),
        };
        assert_eq!(extract_pane_id(&env), Some(pane_id));
    }
}

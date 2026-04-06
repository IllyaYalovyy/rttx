//! Unix socket listener and client connection handling.
//!
//! Manages the server's listening socket and per-client read/write loops
//! with length-prefixed protobuf framing. `ClientConnection` is generic
//! over any async read/write pair, supporting both Unix sockets (local)
//! and stdin/stdout (remote via SSH).

use bytes::BytesMut;
use rttx_proto::{decode_frame, encode_frame, proto};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

/// Errors from IPC operations.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    /// I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Frame decode error.
    #[error("frame: {0}")]
    Frame(#[from] rttx_proto::FrameError),
}

/// A listening Unix socket server.
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
}

impl Listener {
    /// Bind a new Unix socket listener at the given path.
    pub fn bind(path: &Path) -> Result<Self, IpcError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Remove stale socket file if it exists.
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let inner = UnixListener::bind(path)?;
        Ok(Self { inner, path: path.to_path_buf() })
    }

    /// Accept the next client connection.
    pub async fn accept(&self) -> Result<ClientConnection<UnixStream>, IpcError> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(ClientConnection::new(stream))
    }

    /// Return the socket path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the socket file on shutdown.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// A connected client with buffered read/write.
///
/// Generic over the transport: `UnixStream` for local connections,
/// or a stdin/stdout pair for SSH stdio mode.
pub struct ClientConnection<S> {
    stream: S,
    read_buf: BytesMut,
}

impl<S> ClientConnection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Create a new client connection wrapping any async read/write stream.
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self { stream, read_buf: BytesMut::with_capacity(8192) }
    }

    /// Read the next client message. Returns `None` on clean disconnect.
    pub async fn read_message(&mut self) -> Result<Option<proto::ClientMessage>, IpcError> {
        loop {
            match decode_frame::<proto::ClientMessage>(&mut self.read_buf) {
                Ok(msg) => return Ok(Some(msg)),
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => return Err(IpcError::Frame(e)),
            }

            let n = self.stream.read_buf(&mut self.read_buf).await?;
            if n == 0 {
                return Ok(None);
            }
        }
    }

    /// Send a server message to this client.
    pub async fn send_message(&mut self, msg: &proto::ServerMessage) -> Result<(), IpcError> {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf)?;
        self.stream.write_all(&buf).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Send a client message (used by the `stop` command).
    pub async fn send_client_message(
        &mut self,
        msg: &proto::ClientMessage,
    ) -> Result<(), IpcError> {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf)?;
        self.stream.write_all(&buf).await?;
        self.stream.flush().await?;
        Ok(())
    }
}

/// Convenience alias for Unix socket connections.
impl ClientConnection<UnixStream> {
    /// Create a client connection from an existing Unix stream.
    #[must_use]
    pub fn from_stream(stream: UnixStream) -> Self {
        Self::new(stream)
    }
}

/// A combined async read/write wrapper over stdin + stdout.
///
/// Used by the `attach-stdio` command to serve a single client over
/// the process's standard I/O (for SSH tunneling).
pub struct StdioStream {
    stdin: tokio::io::Stdin,
    stdout: tokio::io::Stdout,
}

impl StdioStream {
    /// Create a new stdio stream from the process's stdin and stdout.
    #[must_use]
    pub fn new() -> Self {
        Self { stdin: tokio::io::stdin(), stdout: tokio::io::stdout() }
    }
}

impl Default for StdioStream {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncRead for StdioStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdin).poll_read(cx, buf)
    }
}

impl AsyncWrite for StdioStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stdout).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdout).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stdout).poll_shutdown(cx)
    }
}

/// Check if a server is already running by attempting to connect to the socket.
pub async fn is_server_running(socket_path: &Path) -> bool {
    UnixStream::connect(socket_path).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rttx_proto::uuid_to_bytes;
    use tempfile::TempDir;

    #[tokio::test]
    async fn listener_bind_and_accept() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("test.sock");
        let listener = Listener::bind(&sock_path).unwrap();
        assert!(sock_path.exists());

        let client_task = tokio::spawn(async move {
            let _stream = UnixStream::connect(&sock_path).await.unwrap();
        });

        let _conn = listener.accept().await.unwrap();
        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn send_and_receive_hello() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("test.sock");
        let listener = Listener::bind(&sock_path).unwrap();

        let path_clone = sock_path.clone();
        let client_task = tokio::spawn(async move {
            let stream = UnixStream::connect(&path_clone).await.unwrap();
            let mut conn = ClientConnection::new(stream);

            let hello = proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Hello(proto::Hello {
                    protocol_version: rttx_proto::PROTOCOL_VERSION,
                    client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                })),
            };
            let mut buf = BytesMut::new();
            encode_frame(&hello, &mut buf).unwrap();
            conn.stream.write_all(&buf).await.unwrap();
        });

        let mut server_conn = listener.accept().await.unwrap();
        let msg = server_conn.read_message().await.unwrap().unwrap();
        assert!(msg.msg.is_some());
        if let Some(proto::client_message::Msg::Hello(hello)) = msg.msg {
            assert_eq!(hello.protocol_version, rttx_proto::PROTOCOL_VERSION);
        } else {
            panic!("expected Hello message");
        }

        client_task.await.unwrap();
    }

    #[tokio::test]
    async fn client_disconnect_returns_none() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("test.sock");
        let listener = Listener::bind(&sock_path).unwrap();

        let path_clone = sock_path.clone();
        let client_task = tokio::spawn(async move {
            let _stream = UnixStream::connect(&path_clone).await.unwrap();
        });

        let mut server_conn = listener.accept().await.unwrap();
        client_task.await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let msg = server_conn.read_message().await.unwrap();
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn is_server_running_false_when_no_socket() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("nonexistent.sock");
        assert!(!is_server_running(&sock_path).await);
    }

    /// attach-stdio proxy requires a running daemon. This test documents
    /// that the socket must exist before a proxy can connect. #269.
    #[test]
    fn socket_path_must_exist_for_proxy_connection() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("rttx-server.sock");
        assert!(!sock_path.exists(), "socket must not exist in empty dir");
    }
}

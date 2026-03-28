//! Common test utilities for integration tests.

#![allow(dead_code)]

use bytes::BytesMut;
use rttx_proto::{decode_frame, encode_frame, proto, uuid_to_bytes};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// A test client that connects to the server socket.
pub struct TestClient {
    stream: UnixStream,
    read_buf: BytesMut,
}

/// Default timeout for `recv_timeout`.
const DEFAULT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl TestClient {
    /// Connect to the server at the given socket path.
    pub async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.expect("failed to connect to server");
        Self { stream, read_buf: BytesMut::with_capacity(8192) }
    }

    /// Send a client message.
    pub async fn send(&mut self, msg: &proto::ClientMessage) {
        let mut buf = BytesMut::new();
        encode_frame(msg, &mut buf).expect("encode failed");
        self.stream.write_all(&buf).await.expect("write failed");
    }

    /// Receive a server message.
    pub async fn recv(&mut self) -> proto::ServerMessage {
        loop {
            match decode_frame::<proto::ServerMessage>(&mut self.read_buf) {
                Ok(msg) => return msg,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => panic!("decode error: {e}"),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await.expect("read failed");
            assert!(n > 0, "unexpected EOF");
        }
    }

    /// Try to receive a server message with a timeout.
    /// Returns `None` if the timeout expires.
    pub async fn try_recv(&mut self, timeout: std::time::Duration) -> Option<proto::ServerMessage> {
        tokio::time::timeout(timeout, self.recv()).await.ok()
    }

    /// Receive a server message with the default timeout.
    pub async fn recv_or_timeout(&mut self) -> proto::ServerMessage {
        self.try_recv(DEFAULT_RECV_TIMEOUT).await.expect("timed out waiting for server message")
    }

    /// Collect all messages received within a time window.
    pub async fn drain(&mut self, window: std::time::Duration) -> Vec<proto::ServerMessage> {
        let mut msgs = Vec::new();
        let deadline = tokio::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.try_recv(remaining).await {
                Some(msg) => msgs.push(msg),
                None => break,
            }
        }
        msgs
    }

    /// Send Hello and receive `HelloAck`.
    pub async fn handshake(&mut self) -> proto::HelloAck {
        let hello = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Hello(proto::Hello {
                protocol_version: rttx_proto::PROTOCOL_VERSION,
                client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
            })),
        };
        self.send(&hello).await;
        let resp = self.recv().await;
        match resp.msg {
            Some(proto::server_message::Msg::HelloAck(ack)) => ack,
            other => panic!("expected HelloAck, got {other:?}"),
        }
    }
}

/// Start a server in the background and return the socket path.
pub async fn start_test_server(
    tmp_dir: &Path,
) -> (PathBuf, tokio::task::JoinHandle<anyhow::Result<()>>) {
    use rttx_server::os::OsInterface;
    use rttx_server::server::Server;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Debug)]
    struct TestOs {
        runtime_dir: PathBuf,
        cache_dir: PathBuf,
    }
    impl OsInterface for TestOs {
        fn runtime_dir(&self) -> PathBuf {
            self.runtime_dir.clone()
        }
        fn cache_dir(&self) -> PathBuf {
            self.cache_dir.clone()
        }
    }

    let runtime_dir = tmp_dir.join("runtime");
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let socket_path = runtime_dir.join("rttx-server.sock");

    let os = TestOs { runtime_dir, cache_dir };
    let server = Arc::new(Mutex::new(Server::new(Box::new(os))));

    // Load persisted state and reconstruct sessions (if any).
    {
        let mut s = server.lock().await;
        s.load_persisted_state();
    }
    Server::reconstruct_sessions(&server).await;

    let sock = socket_path.clone();
    let handle = tokio::spawn(async move { rttx_server::server::run(server).await });

    // Wait for socket to appear.
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "server socket did not appear");

    (socket_path, handle)
}

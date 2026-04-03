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
const DEFAULT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

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

/// Wait until the state file exists and has been written at least once.
/// Polls every 200ms for up to `timeout`.
pub async fn wait_for_state_file(cache_dir: &std::path::Path, timeout: std::time::Duration) {
    let state_path = cache_dir.join("state.json");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if state_path.exists() && std::fs::metadata(&state_path).is_ok_and(|m| m.len() > 2) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "state file not written within {}ms at {}",
            timeout.as_millis(),
            state_path.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

// ── Reusable protocol helpers ───────────────────────────────────
//
// These cover the most common test operations. All helpers that receive
// server responses drain interleaved Delta messages so they work
// reliably on slow CI runners.

/// Create a session and return its ID.
pub async fn create_session(
    client: &mut TestClient,
    name: &str,
    policy: proto::RuntimePolicy,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.into(),
                policy: policy as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    }
}

/// Attach as read-write and return the snapshot.
pub async fn attach_rw(client: &mut TestClient, session_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(s)) => s,
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

/// Attach as read-only and return the snapshot.
pub async fn attach_ro(client: &mut TestClient, session_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(s)) => s,
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

/// Create a pane, draining interleaved Deltas until `PaneCreated` arrives.
pub async fn create_pane(client: &mut TestClient, session_id: &[u8]) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => return pc.pane_id,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

/// Close a pane, draining interleaved Deltas and `PaneExited` until `PaneClosed`.
pub async fn close_pane(client: &mut TestClient, session_id: &[u8], pane_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.to_vec(),
                pane_id: pane_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneClosed(_)) => return,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
}

/// Detach from a session, draining Deltas until `SessionDetached` or `SessionTerminated`.
pub async fn detach_session(client: &mut TestClient, session_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(
                proto::server_message::Msg::SessionDetached(_)
                | proto::server_message::Msg::SessionTerminated(_),
            ) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionDetached/Terminated, got {other:?}"),
        }
    }
}

/// Terminate a session, draining Deltas until `SessionTerminated`.
pub async fn terminate_session(client: &mut TestClient, session_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionTerminated(_)) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionTerminated, got {other:?}"),
        }
    }
}

/// List sessions, draining pending Deltas.
pub async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    client.drain(std::time::Duration::from_millis(50)).await;
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => return sl.sessions,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionList, got {other:?}"),
        }
    }
}

/// Send input to a pane (fire-and-forget, no response expected).
pub async fn send_input(client: &mut TestClient, session_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.to_vec(),
                pane_id: pane_id.to_vec(),
                data: data.to_vec(),
            })),
        })
        .await;
}

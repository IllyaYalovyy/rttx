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
        fn state_dir(&self) -> PathBuf {
            self.cache_dir.parent().unwrap_or(self.cache_dir.as_path()).join("state/rttx/daemon")
        }
    }

    let runtime_dir = tmp_dir.join("runtime");
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let socket_path = runtime_dir.join("rttx-server.sock");

    let os = TestOs { runtime_dir, cache_dir };
    let metrics = Arc::new(rttx_server::metrics::DaemonMetrics::new());
    let ring = Arc::new(rttx_server::flight::RingWriter::open(tmp_dir).unwrap());
    let server = Arc::new(Mutex::new(Server::new(Box::new(os), metrics, ring)));

    // Load persisted state and reconstruct sessions (if any).
    {
        let mut s = server.lock().await;
        s.load_persisted_state();
    }
    Server::reconstruct_runtimes(&server).await;

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

// ── Reusable protocol helpers ───────────────────────────────────
//
// These cover the most common test operations. All helpers that receive
// server responses drain interleaved Delta messages so they work
// reliably on slow CI runners.

/// Wait until the v2 daemon index exists and has been written at least once.
/// Polls every 200ms for up to `timeout`.
pub async fn wait_for_state_file(state_dir: &std::path::Path, timeout: std::time::Duration) {
    let index_path = state_dir.join("state/rttx/daemon/daemon.json");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if index_path.exists() && std::fs::metadata(&index_path).is_ok_and(|m| m.len() > 2) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "v2 daemon index not written within {}ms at {}",
            timeout.as_millis(),
            index_path.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Wait until at least one `.log` file appears under
/// `base_dir/state/rttx/daemon/runtimes/<id>/scrollback/`.
/// Polls every 200ms for up to `timeout`.
pub async fn wait_for_scrollback_log(
    base_dir: &std::path::Path,
    timeout: std::time::Duration,
) -> Vec<std::path::PathBuf> {
    let runtimes_dir = base_dir.join("state/rttx/daemon/runtimes");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let logs = find_scrollback_logs(&runtimes_dir);
        if !logs.is_empty() {
            return logs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no scrollback log appeared under {} within {}s",
            runtimes_dir.display(),
            timeout.as_secs()
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

fn find_scrollback_logs(runtimes_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut logs = Vec::new();
    let Ok(entries) = std::fs::read_dir(runtimes_dir) else { return logs };
    for entry in entries.flatten() {
        let scrollback_dir = entry.path().join("scrollback");
        if let Ok(files) = std::fs::read_dir(&scrollback_dir) {
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|ext| ext == "log") {
                    logs.push(path);
                }
            }
        }
    }
    logs
}

/// Wait until any v2 runtime file under the state dir contains a specific
/// substring. Polls every 200ms for up to `timeout`.
///
/// The `base_dir` is the test's temp root (same as passed to
/// `start_test_server`). The v2 state lives under
/// `base_dir/state/rttx/daemon/runtimes/`.
pub async fn wait_for_state_containing(
    base_dir: &std::path::Path,
    needle: &str,
    timeout: std::time::Duration,
) {
    let runtimes_dir = base_dir.join("state/rttx/daemon/runtimes");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(entries) = std::fs::read_dir(&runtimes_dir) {
            for entry in entries.flatten() {
                let runtime_json = entry.path().join("runtime.json");
                if let Ok(content) = std::fs::read_to_string(&runtime_json)
                    && content.contains(needle)
                {
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "v2 runtime files under {} never contained '{needle}'",
            runtimes_dir.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Create a session and return its ID.
pub async fn create_runtime(
    client: &mut TestClient,
    name: &str,
    policy: proto::RuntimePolicy,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: name.into(),
                policy: policy as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

/// Attach as read-write and return the snapshot.
pub async fn attach_rw(client: &mut TestClient, runtime_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => return s,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

/// Attach as read-only and return the snapshot.
pub async fn attach_ro(client: &mut TestClient, runtime_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => return s,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

/// Create a pane, draining interleaved Deltas until `PaneCreated` arrives.
pub async fn create_pane(client: &mut TestClient, runtime_id: &[u8]) -> Vec<u8> {
    create_pane_with_cwd(client, runtime_id, None).await
}

/// Create a pane with an optional CWD, draining interleaved Deltas until `PaneCreated` arrives.
pub async fn create_pane_with_cwd(
    client: &mut TestClient,
    runtime_id: &[u8],
    cwd: Option<String>,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                runtime_id: runtime_id.to_vec(),
                cwd,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
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
pub async fn close_pane(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                runtime_id: runtime_id.to_vec(),
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

/// Detach from a session, draining Deltas until `RuntimeDetached` or `RuntimeTerminated`.
pub async fn detach_runtime(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
                runtime_id: runtime_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(
                proto::server_message::Msg::RuntimeDetached(_)
                | proto::server_message::Msg::RuntimeTerminated(_),
            ) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected RuntimeDetached/Terminated, got {other:?}"),
        }
    }
}

/// Terminate a session, draining Deltas until `RuntimeTerminated`.
pub async fn terminate_runtime(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
                runtime_id: runtime_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeTerminated(_)) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected RuntimeTerminated, got {other:?}"),
        }
    }
}

/// List sessions, draining pending Deltas.
pub async fn list_runtimes(client: &mut TestClient) -> Vec<proto::RuntimeInfo> {
    client.drain(std::time::Duration::from_millis(50)).await;
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => return sl.runtimes,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected RuntimeList, got {other:?}"),
        }
    }
}

/// Send input to a pane (fire-and-forget, no response expected).
pub async fn send_input(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                data: bytes::Bytes::copy_from_slice(data),
            })),
        })
        .await;
}

// ── V3 test client ──────────────────────────────────────────────

use rttx_proto::v3;

/// A test client that speaks the v3 protocol over a real Unix socket.
pub struct TestV3Client {
    stream: UnixStream,
    read_buf: BytesMut,
    request_id: std::sync::atomic::AtomicU64,
}

impl TestV3Client {
    /// Connect to the server and perform the v3 handshake.
    pub async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.expect("failed to connect to server");
        let mut client = Self {
            stream,
            read_buf: BytesMut::with_capacity(8192),
            request_id: std::sync::atomic::AtomicU64::new(1),
        };
        client.v3_handshake().await;
        client
    }

    async fn v3_handshake(&mut self) {
        let hello = rttx_proto::v3_handshake::build_client_hello(
            uuid::Uuid::new_v4(),
            "test-v3",
            "0.0.0",
            rttx_proto::v3_handshake::CORE_CAPABILITIES,
        );
        let mut buf = BytesMut::new();
        encode_frame(&hello, &mut buf).expect("encode ClientHello");
        self.stream.write_all(&buf).await.expect("write ClientHello");

        // Read ServerHello
        loop {
            match decode_frame::<v3::ServerHello>(&mut self.read_buf) {
                Ok(_) => return,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => panic!("decode ServerHello error: {e}"),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await.expect("read");
            assert!(n > 0, "unexpected EOF during v3 handshake");
        }
    }

    fn next_request_id(&self) -> u64 {
        self.request_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a v3 client envelope.
    pub async fn send(&mut self, env: &v3::ClientEnvelope) {
        let mut buf = BytesMut::new();
        encode_frame(env, &mut buf).expect("encode");
        self.stream.write_all(&buf).await.expect("write");
    }

    /// Receive a v3 server envelope.
    pub async fn recv(&mut self) -> v3::ServerEnvelope {
        loop {
            match decode_frame::<v3::ServerEnvelope>(&mut self.read_buf) {
                Ok(env) => return env,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => panic!("decode error: {e}"),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await.expect("read");
            assert!(n > 0, "unexpected EOF");
        }
    }

    /// Receive with timeout.
    pub async fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Option<v3::ServerEnvelope> {
        tokio::time::timeout(timeout, self.recv()).await.ok()
    }

    /// Create a runtime via v3 and return the `runtime_id` bytes.
    pub async fn create_runtime(&mut self, name: &str) -> Vec<u8> {
        let env = v3::ClientEnvelope {
            request_id: self.next_request_id(),
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: name.into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        };
        self.send(&env).await;
        loop {
            let resp = self.recv().await;
            match resp.payload {
                Some(v3::server_envelope::Payload::RuntimeCreated(rc)) => return rc.runtime_id,
                Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
                other => panic!("expected RuntimeCreated, got {other:?}"),
            }
        }
    }

    /// Attach read-write and return the snapshot.
    pub async fn attach_rw(&mut self, runtime_id: &[u8]) -> v3::RuntimeSnapshot {
        let env = v3::ClientEnvelope {
            request_id: self.next_request_id(),
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        };
        self.send(&env).await;
        loop {
            let resp = self.recv().await;
            match resp.payload {
                Some(v3::server_envelope::Payload::RuntimeSnapshot(snap)) => return snap,
                Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
                other => panic!("expected RuntimeSnapshot, got {other:?}"),
            }
        }
    }

    /// Create a pane and return the `pane_id` bytes.
    pub async fn create_pane(&mut self, runtime_id: &[u8]) -> Vec<u8> {
        let env = v3::ClientEnvelope {
            request_id: self.next_request_id(),
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.to_vec(),
                cwd: None,
                dark_background: None,
                cols: 80,
                rows: 24,
                no_persist: None,
            })),
        };
        self.send(&env).await;
        loop {
            let resp = self.recv().await;
            match resp.payload {
                Some(v3::server_envelope::Payload::PaneCreated(pc)) => return pc.pane_id,
                Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
                other => panic!("expected PaneCreated, got {other:?}"),
            }
        }
    }

    /// Send raw input to a pane.
    pub async fn send_input(&mut self, runtime_id: &[u8], pane_id: &[u8], data: &[u8]) {
        let env = v3::ClientEnvelope {
            request_id: self.next_request_id(),
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::copy_from_slice(data),
                })),
            })),
        };
        self.send(&env).await;
    }

    /// Collect all `OutputDelta` messages within a time window, returning their `pane_output_seq` values.
    pub async fn collect_output_seqs(&mut self, window: std::time::Duration) -> Vec<u64> {
        let mut seqs = Vec::new();
        let deadline = tokio::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.recv_timeout(remaining).await {
                Some(env) => {
                    if let Some(v3::server_envelope::Payload::OutputDelta(delta)) = env.payload {
                        seqs.push(delta.pane_output_seq);
                    }
                }
                None => break,
            }
        }
        seqs
    }
}

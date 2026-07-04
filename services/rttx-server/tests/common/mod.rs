//! Common test utilities for integration tests.

#![allow(dead_code)]

use bytes::BytesMut;
use rttx_proto::{decode_frame, encode_frame, v3, v3_handshake};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// A test client that connects to the server socket using v3 protocol.
pub struct TestClient {
    stream: UnixStream,
    read_buf: BytesMut,
    request_id: AtomicU64,
}

/// Default timeout for `recv_timeout`.
const DEFAULT_RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

impl TestClient {
    pub async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.expect("failed to connect to server");
        Self { stream, read_buf: BytesMut::with_capacity(8192), request_id: AtomicU64::new(1) }
    }

    pub fn next_request_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn send(&mut self, env: &v3::ClientEnvelope) {
        let mut buf = BytesMut::new();
        encode_frame(env, &mut buf).expect("encode failed");
        self.stream.write_all(&buf).await.expect("write failed");
    }

    /// Receive one server envelope, bounded by [`DEFAULT_RECV_TIMEOUT`].
    ///
    /// The timeout lives in the foundation on purpose: every call site —
    /// including the many direct `recv().await` calls across the test
    /// suite — inherits it, so a missing or delayed server message fails
    /// the test promptly instead of hanging the whole `cargo test` run.
    pub async fn recv(&mut self) -> v3::ServerEnvelope {
        tokio::time::timeout(DEFAULT_RECV_TIMEOUT, self.recv_raw()).await.unwrap_or_else(|_| {
            panic!(
                "TestClient::recv timed out after {DEFAULT_RECV_TIMEOUT:?} waiting for a server message"
            )
        })
    }

    /// Inner receive loop with no timeout. Callers must wrap this in a
    /// timeout (see [`recv`](Self::recv) and [`try_recv`](Self::try_recv)).
    async fn recv_raw(&mut self) -> v3::ServerEnvelope {
        loop {
            match decode_frame::<v3::ServerEnvelope>(&mut self.read_buf) {
                Ok(env) => return env,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => panic!("decode error: {e}"),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await.expect("read failed");
            assert!(n > 0, "unexpected EOF");
        }
    }

    pub async fn try_recv(&mut self, timeout: std::time::Duration) -> Option<v3::ServerEnvelope> {
        tokio::time::timeout(timeout, self.recv_raw()).await.ok()
    }

    pub async fn recv_or_timeout(&mut self) -> v3::ServerEnvelope {
        self.try_recv(DEFAULT_RECV_TIMEOUT).await.expect("timed out waiting for server message")
    }

    pub async fn drain(&mut self, window: std::time::Duration) -> Vec<v3::ServerEnvelope> {
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

    pub async fn handshake(&mut self) -> v3::ServerHello {
        // A test client advertises every capability so that all server
        // features (diagnostics, inventory, takeover, etc.) are negotiated.
        const ALL_CAPABILITIES: &[v3::Capability] = &[
            v3::Capability::CoreWorkspaceLifecycle,
            v3::Capability::CorePaneLifecycle,
            v3::Capability::CoreTerminalIo,
            v3::Capability::CoreTerminalModes,
            v3::Capability::CorePasteIntent,
            v3::Capability::CoreFocusEvents,
            v3::Capability::OptWorkspaceInventory,
            v3::Capability::OptResync,
            v3::Capability::OptChunkedScrollback,
            v3::Capability::OptDiagnostics,
            v3::Capability::OptWorkspaceTakeover,
        ];
        let hello = v3_handshake::build_client_hello(
            uuid::Uuid::new_v4(),
            "test-client",
            "0.0.0",
            ALL_CAPABILITIES,
        );
        let mut buf = BytesMut::new();
        encode_frame(&hello, &mut buf).expect("encode ClientHello");
        self.stream.write_all(&buf).await.expect("write ClientHello");
        loop {
            match decode_frame::<v3::ServerHello>(&mut self.read_buf) {
                Ok(sh) => return sh,
                Err(rttx_proto::FrameError::Incomplete) => {}
                Err(e) => panic!("decode ServerHello error: {e}"),
            }
            let n = self.stream.read_buf(&mut self.read_buf).await.expect("read failed");
            assert!(n > 0, "unexpected EOF during handshake");
        }
    }

    pub async fn send_cmd(&mut self, command: v3::client_envelope::Command) -> u64 {
        let request_id = if rttx_proto::v3_envelope::is_fire_and_forget(&command) {
            0
        } else {
            self.next_request_id()
        };
        let env = v3::ClientEnvelope { request_id, command: Some(command) };
        self.send(&env).await;
        request_id
    }

    /// Send a command and return the reply envelope whose `request_id`
    /// matches the request, skipping any interleaved push events.
    ///
    /// Push events (`OutputDelta`, `PaneExited`, etc.) carry `request_id == 0`;
    /// command replies echo the request's id. PTY activity can interleave
    /// pushes with the ack, so matching by id is the only robust way to
    /// read a specific command's reply.
    pub async fn request(&mut self, command: v3::client_envelope::Command) -> v3::ServerEnvelope {
        let request_id = self.next_request_id();
        let env = v3::ClientEnvelope { request_id, command: Some(command) };
        self.send(&env).await;
        loop {
            let reply = self.recv().await;
            if reply.request_id == request_id {
                return reply;
            }
            // Interleaved push event or a stale reply — keep reading.
        }
    }

    /// Wait for the first server envelope whose payload matches `predicate`,
    /// skipping all other (typically push) events.
    ///
    /// Fire-and-forget commands (resize, set-title, input) produce broadcast
    /// events with `request_id == 0` that arrive interleaved with other push
    /// events such as `OutputDelta` and `CwdChanged`. This skips everything
    /// until the awaited event is seen. Each read is bounded by the recv
    /// timeout, so a never-arriving event fails the test instead of hanging.
    pub async fn recv_matching<F>(&mut self, mut predicate: F) -> v3::ServerEnvelope
    where
        F: FnMut(&v3::server_envelope::Payload) -> bool,
    {
        loop {
            let env = self.recv().await;
            if env.payload.as_ref().is_some_and(&mut predicate) {
                return env;
            }
        }
    }

    /// Round-trip a Ping/Pong, acting as a barrier that flushes all
    /// previously-sent fire-and-forget commands.
    ///
    /// The server processes a single client's frames in order, so once the
    /// matching Pong is received every earlier command (resize, set-title,
    /// input, …) has been applied. v3 fire-and-forget commands produce no
    /// ack of their own, so this is the canonical way to synchronise before
    /// observing their effect (e.g. via a reattach snapshot).
    pub async fn ping(&mut self) {
        let nonce = self.next_request_id();
        let reply = self.request(v3::client_envelope::Command::Ping(v3::Ping { nonce })).await;
        match reply.payload {
            Some(v3::server_envelope::Payload::Pong(pong)) => {
                assert_eq!(pong.nonce, nonce, "Pong nonce must match Ping");
            }
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    pub async fn collect_output_seqs(&mut self, window: std::time::Duration) -> Vec<u64> {
        let mut seqs = Vec::new();
        let deadline = tokio::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.try_recv(remaining).await {
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

    let runtime_dir = tmp_dir.join("workspace");
    let cache_dir = tmp_dir.join("cache");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    let socket_path = runtime_dir.join("rttx-server.sock");
    let os = TestOs { runtime_dir, cache_dir };
    let metrics = Arc::new(rttx_server::metrics::DaemonMetrics::new());
    let ring = Arc::new(rttx_server::flight::RingWriter::open(tmp_dir).unwrap());
    let server = Arc::new(Mutex::new(Server::new(Box::new(os), metrics, ring)));
    {
        let mut s = server.lock().await;
        s.load_persisted_state();
    }
    Server::reconstruct_workspaces(&server).await;
    let sock = socket_path.clone();
    let handle = tokio::spawn(async move { rttx_server::server::run(server).await });
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "server socket did not appear");
    (socket_path, handle)
}

// ── Helpers ─────────────────────────────────────────────────────

pub async fn wait_for_state_file(state_dir: &Path, timeout: std::time::Duration) {
    let index_path = state_dir.join("state/rttx/daemon/daemon.json");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if index_path.exists() && std::fs::metadata(&index_path).is_ok_and(|m| m.len() > 2) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "daemon index not written");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

pub async fn wait_for_scrollback_log(
    base_dir: &Path,
    timeout: std::time::Duration,
) -> Vec<PathBuf> {
    let runtimes_dir = base_dir.join("state/rttx/daemon/workspaces");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let logs = find_scrollback_logs(&runtimes_dir);
        if !logs.is_empty() {
            return logs;
        }
        assert!(tokio::time::Instant::now() < deadline, "no scrollback log appeared");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

fn find_scrollback_logs(runtimes_dir: &Path) -> Vec<PathBuf> {
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

pub async fn wait_for_state_containing(
    base_dir: &Path,
    needle: &str,
    timeout: std::time::Duration,
) {
    let runtimes_dir = base_dir.join("state/rttx/daemon/workspaces");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(entries) = std::fs::read_dir(&runtimes_dir) {
            for entry in entries.flatten() {
                let workspace_json = entry.path().join("workspace.json");
                if let Ok(content) = std::fs::read_to_string(&workspace_json)
                    && content.contains(needle)
                {
                    return;
                }
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "state never contained '{needle}'");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

pub async fn create_workspace(
    client: &mut TestClient,
    name: &str,
    policy: v3::WorkspacePolicy,
) -> Vec<u8> {
    client
        .send_cmd(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: name.into(),
            policy: policy as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(rc)) => return rc.runtime_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        }
    }
}

pub async fn attach_rw(client: &mut TestClient, runtime_id: &[u8]) -> v3::WorkspaceSnapshot {
    client
        .send_cmd(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.to_vec(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceSnapshot, got {other:?}"),
        }
    }
}

pub async fn attach_ro(client: &mut TestClient, runtime_id: &[u8]) -> v3::WorkspaceSnapshot {
    client
        .send_cmd(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.to_vec(),
            attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceSnapshot, got {other:?}"),
        }
    }
}

pub async fn create_pane(client: &mut TestClient, runtime_id: &[u8]) -> Vec<u8> {
    create_pane_with_cwd(client, runtime_id, None).await
}

pub async fn create_pane_with_cwd(
    client: &mut TestClient,
    runtime_id: &[u8],
    cwd: Option<String>,
) -> Vec<u8> {
    client
        .send_cmd(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.to_vec(),
            cwd,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => return pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

pub async fn close_pane(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: runtime_id.to_vec(),
            pane_id: pane_id.to_vec(),
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneClosed(_)) => return,
            Some(
                v3::server_envelope::Payload::OutputDelta(_)
                | v3::server_envelope::Payload::PaneExited(_)
                | v3::server_envelope::Payload::TitleChanged(_),
            ) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
}

pub async fn detach_workspace(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
            runtime_id: runtime_id.to_vec(),
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(
                v3::server_envelope::Payload::WorkspaceDetached(_)
                | v3::server_envelope::Payload::WorkspaceTerminated(_),
            ) => return,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceDetached/Terminated, got {other:?}"),
        }
    }
}

pub async fn terminate_workspace(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
            runtime_id: runtime_id.to_vec(),
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceTerminated(_)) => return,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceTerminated, got {other:?}"),
        }
    }
}

pub async fn list_workspaces(client: &mut TestClient) -> Vec<v3::WorkspaceInfo> {
    client.drain(std::time::Duration::from_millis(50)).await;
    client.send_cmd(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(rl)) => return rl.workspaces,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceList, got {other:?}"),
        }
    }
}

pub async fn send_input(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.to_vec(),
            pane_id: pane_id.to_vec(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::copy_from_slice(data),
            })),
        }))
        .await;
}

/// Alias retained for tests that refer to `TestV3Client`.
pub type TestV3Client = TestClient;

// ── RFC-031 §5 tree-protocol helpers ────────────────────────────────

/// Split `target_pane_id`, returning the server's `PaneSplit` delta (carrying
/// the server-assigned new pane id).
pub async fn split_pane(
    client: &mut TestClient,
    runtime_id: &[u8],
    target_pane_id: &[u8],
    axis: v3::PaneSplitAxis,
    ratio: f32,
) -> v3::PaneSplit {
    let reply = client
        .request(v3::client_envelope::Command::SplitPane(v3::SplitPane {
            runtime_id: runtime_id.to_vec(),
            target_pane_id: target_pane_id.to_vec(),
            axis: axis as i32,
            ratio,
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        }))
        .await;
    match reply.payload {
        Some(v3::server_envelope::Payload::PaneSplit(p)) => p,
        other => panic!("expected PaneSplit, got {other:?}"),
    }
}

/// Resize the split addressed by `path`, returning the `SplitResized` delta.
pub async fn resize_split(
    client: &mut TestClient,
    runtime_id: &[u8],
    path: &[v3::PaneTreeSide],
    ratio: f32,
) -> v3::SplitResized {
    let reply = client
        .request(v3::client_envelope::Command::ResizeSplit(v3::ResizeSplit {
            runtime_id: runtime_id.to_vec(),
            path: path.iter().map(|s| *s as i32).collect(),
            ratio,
        }))
        .await;
    match reply.payload {
        Some(v3::server_envelope::Payload::SplitResized(r)) => r,
        other => panic!("expected SplitResized, got {other:?}"),
    }
}

/// Set the fallback focus pane, returning the `FocusChanged` delta.
pub async fn set_focus(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> v3::FocusChanged {
    let reply = client
        .request(v3::client_envelope::Command::SetFocus(v3::SetFocus {
            runtime_id: runtime_id.to_vec(),
            pane_id: pane_id.to_vec(),
        }))
        .await;
    match reply.payload {
        Some(v3::server_envelope::Payload::FocusChanged(f)) => f,
        other => panic!("expected FocusChanged, got {other:?}"),
    }
}

/// Report per-pane render sizes (fire-and-forget) and barrier on a ping so the
/// min-size policy has been applied before the caller observes its effect.
pub async fn report_client_size(
    client: &mut TestClient,
    runtime_id: &[u8],
    panes: &[(Vec<u8>, u32, u32)],
) {
    client
        .send_cmd(v3::client_envelope::Command::ReportClientSize(v3::ReportClientSize {
            runtime_id: runtime_id.to_vec(),
            panes: panes
                .iter()
                .map(|(id, cols, rows)| v3::ClientPaneSize {
                    pane_id: id.clone(),
                    cols: *cols,
                    rows: *rows,
                })
                .collect(),
        }))
        .await;
    client.ping().await;
}

/// Reattach (resync) and return a fresh snapshot carrying the authoritative
/// tree.
pub async fn resync(client: &mut TestClient, runtime_id: &[u8]) -> v3::WorkspaceSnapshot {
    let reply = client
        .request(v3::client_envelope::Command::ResyncWorkspace(v3::ResyncWorkspace {
            runtime_id: runtime_id.to_vec(),
        }))
        .await;
    match reply.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(s)) => s,
        other => panic!("expected WorkspaceSnapshot, got {other:?}"),
    }
}

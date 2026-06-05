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

    pub async fn recv(&mut self) -> v3::ServerEnvelope {
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
        tokio::time::timeout(timeout, self.recv()).await.ok()
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
        let hello = v3_handshake::build_client_hello(
            uuid::Uuid::new_v4(),
            "test-client",
            "0.0.0",
            v3_handshake::CORE_CAPABILITIES,
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

    let runtime_dir = tmp_dir.join("runtime");
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
    Server::reconstruct_runtimes(&server).await;
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
    let runtimes_dir = base_dir.join("state/rttx/daemon/runtimes");
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
        assert!(tokio::time::Instant::now() < deadline, "state never contained '{needle}'");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

pub async fn create_runtime(
    client: &mut TestClient,
    name: &str,
    policy: v3::RuntimePolicy,
) -> Vec<u8> {
    client
        .send_cmd(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: name.into(),
            policy: policy as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeCreated(rc)) => return rc.runtime_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeCreated, got {other:?}"),
        }
    }
}

pub async fn attach_rw(client: &mut TestClient, runtime_id: &[u8]) -> v3::RuntimeSnapshot {
    client
        .send_cmd(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.to_vec(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeSnapshot, got {other:?}"),
        }
    }
}

pub async fn attach_ro(client: &mut TestClient, runtime_id: &[u8]) -> v3::RuntimeSnapshot {
    client
        .send_cmd(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.to_vec(),
            attach_mode: v3::RuntimeAttachMode::ReadOnly as i32,
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeSnapshot, got {other:?}"),
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
                | v3::server_envelope::Payload::PaneExited(_),
            ) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
}

pub async fn detach_runtime(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
            runtime_id: runtime_id.to_vec(),
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(
                v3::server_envelope::Payload::RuntimeDetached(_)
                | v3::server_envelope::Payload::RuntimeTerminated(_),
            ) => return,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeDetached/Terminated, got {other:?}"),
        }
    }
}

pub async fn terminate_runtime(client: &mut TestClient, runtime_id: &[u8]) {
    client
        .send_cmd(v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: runtime_id.to_vec(),
        }))
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeTerminated(_)) => return,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeTerminated, got {other:?}"),
        }
    }
}

pub async fn list_runtimes(client: &mut TestClient) -> Vec<v3::RuntimeInfo> {
    client.drain(std::time::Duration::from_millis(50)).await;
    client.send_cmd(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeList(rl)) => return rl.runtimes,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeList, got {other:?}"),
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

/// Alias for backward compatibility with tests that imported TestV3Client.
pub type TestV3Client = TestClient;

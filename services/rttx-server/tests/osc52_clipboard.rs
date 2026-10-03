//! Integration tests: OSC 52 clipboard writes reach the client as events and
//! never as bytes (#46).
//!
//! A real shell emits the sequence Claude Code and friends emit. The daemon
//! owns the PTY, so it must decode the payload, push one `ClipboardWrite` to
//! the client holding the write lease, and strip the sequence from the output
//! stream — VTE ignores OSC 52, and the bytes would otherwise also land in
//! the on-disk scrollback log in plaintext.

mod common;

use common::{TestClient, attach_ro, attach_rw, create_pane, create_workspace, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// Everything but the clipboard capability: an older client must not be sent
/// a push it cannot parse.
const CAPS_WITHOUT_CLIPBOARD: &[v3::Capability] = &[
    v3::Capability::CoreWorkspaceLifecycle,
    v3::Capability::CorePaneLifecycle,
    v3::Capability::CoreTerminalIo,
    v3::Capability::CoreTerminalModes,
    v3::Capability::CorePasteIntent,
    v3::Capability::CoreFocusEvents,
    v3::Capability::OptWorkspaceInventory,
];

/// `printf` the exact OSC 52 write an application emits to copy `text`.
async fn emit_osc52(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], text: &str) {
    let script =
        format!("printf '\\033]52;c;%s\\007' \"$(printf '{text}' | base64 | tr -d '\\n')\"\n");
    common::send_input(client, runtime_id, pane_id, script.as_bytes()).await;
}

/// Same write, but with the payload already encoded, so the clipboard text
/// itself never appears on the command line the shell echoes back.
async fn emit_osc52_encoded(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
    base64_payload: &str,
) {
    let script = format!("printf '\\033]52;c;{base64_payload}\\007'\n");
    common::send_input(client, runtime_id, pane_id, script.as_bytes()).await;
}

fn clipboard_writes(msgs: &[v3::ServerEnvelope]) -> Vec<v3::ClipboardWrite> {
    msgs.iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::ClipboardWrite(w)) => Some(w.clone()),
            _ => None,
        })
        .collect()
}

fn delta_bytes(msgs: &[v3::ServerEnvelope]) -> Vec<u8> {
    msgs.iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) => Some(d.data.to_vec()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Next envelope from the `attach-stdio` proxy that stands in for SSH.
async fn next_remote_envelope(
    stdout: &mut tokio::process::ChildStdout,
    read_buf: &mut bytes::BytesMut,
) -> v3::ServerEnvelope {
    use tokio::io::AsyncReadExt;
    loop {
        if let Ok(msg) = rttx_proto::decode_frame::<v3::ServerEnvelope>(read_buf) {
            return msg;
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stdout.read_buf(read_buf))
            .await
            .expect("timed out waiting for the remote daemon")
            .expect("read failed");
        assert!(n > 0, "unexpected EOF");
    }
}

fn assert_no_osc52_bytes(output: &[u8]) {
    assert!(
        !output.windows(5).any(|w| w == b"\x1b]52;"),
        "OSC 52 must not reach the client: {}",
        String::from_utf8_lossy(output)
    );
}

#[tokio::test]
async fn shell_osc52_write_arrives_as_one_event_and_no_output_bytes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "clip", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    attach_rw(&mut client, &runtime_id).await;
    client.drain(Duration::from_millis(500)).await;

    emit_osc52(&mut client, &runtime_id, &pane_id, "hello").await;
    let msgs = client.drain(Duration::from_secs(5)).await;

    let writes = clipboard_writes(&msgs);
    assert_eq!(writes.len(), 1, "expected exactly one ClipboardWrite, got {writes:?}");
    assert_eq!(writes[0].target, "c");
    assert_eq!(writes[0].data, bytes::Bytes::from_static(b"hello"));
    assert_eq!(writes[0].runtime_id, runtime_id);
    assert_eq!(writes[0].pane_id, pane_id);

    assert_no_osc52_bytes(&delta_bytes(&msgs));
}

/// The daemon must not persist clipboard text: the sequence used to be
/// appended to the pane's scrollback log verbatim.
#[tokio::test]
async fn osc52_payload_never_reaches_the_scrollback_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id =
        create_workspace(&mut client, "clip-log", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    attach_rw(&mut client, &runtime_id).await;
    client.drain(Duration::from_millis(500)).await;

    // "topsecret" base64-encoded; the plaintext is never typed.
    emit_osc52_encoded(&mut client, &runtime_id, &pane_id, "dG9wc2VjcmV0").await;
    let msgs = client.drain(Duration::from_secs(5)).await;
    assert_eq!(clipboard_writes(&msgs).len(), 1, "clipboard write must be observed first");

    // Give the periodic flush time to put the pane's output on disk.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut logs = Vec::new();
    for entry in walk(tmp.path()) {
        if entry.extension().is_some_and(|e| e == "log") {
            logs.push(std::fs::read(&entry).unwrap_or_default());
        }
    }
    for log in &logs {
        assert_no_osc52_bytes(log);
        assert!(
            !log.windows(9).any(|w| w == b"topsecret"),
            "clipboard text must not be written to disk"
        );
    }
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// A read-only mirror left behind by a take-over must not be handed the
/// user's clipboard; only the lease holder is.
#[tokio::test]
async fn only_the_write_lease_holder_receives_the_clipboard_event() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "clip-lease", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut writer, &runtime_id).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    writer.drain(Duration::from_millis(500)).await;
    reader.drain(Duration::from_millis(500)).await;

    emit_osc52(&mut writer, &runtime_id, &pane_id, "lease-only").await;
    let writer_msgs = writer.drain(Duration::from_secs(5)).await;
    let reader_msgs = reader.drain(Duration::from_secs(2)).await;

    let writer_writes = clipboard_writes(&writer_msgs);
    assert_eq!(writer_writes.len(), 1, "the lease holder acts on the write");
    assert_eq!(writer_writes[0].data, bytes::Bytes::from_static(b"lease-only"));
    assert!(
        clipboard_writes(&reader_msgs).is_empty(),
        "a read-only client must never be told to replace the clipboard"
    );

    // The reader still sees the pane's output, just without the sequence.
    assert_no_osc52_bytes(&delta_bytes(&reader_msgs));
}

/// A client that never negotiated `OPT_CLIPBOARD_OSC52` gets no such push,
/// and the sequence is still stripped from its output.
#[tokio::test]
async fn client_without_the_capability_receives_no_clipboard_event() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake_with_caps(CAPS_WITHOUT_CLIPBOARD).await;
    let runtime_id =
        create_workspace(&mut client, "clip-old", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    attach_rw(&mut client, &runtime_id).await;
    client.drain(Duration::from_millis(500)).await;

    emit_osc52(&mut client, &runtime_id, &pane_id, "unseen").await;
    let msgs = client.drain(Duration::from_secs(5)).await;

    assert!(
        clipboard_writes(&msgs).is_empty(),
        "an old client must not receive a capability-gated push"
    );
    assert_no_osc52_bytes(&delta_bytes(&msgs));
}

/// The same write over the transport a remote daemon uses.
///
/// A workspace on another host runs its daemon behind `attach-stdio` over an
/// SSH pipe, which is the whole point of handling OSC 52 daemon-side: a
/// program on the remote machine puts text on the *local* clipboard. SSH only
/// carries the pipe, so driving `attach-stdio` directly proves the remote
/// path end to end without needing a second host.
#[tokio::test]
async fn clipboard_write_survives_the_remote_stdio_transport() {
    use bytes::BytesMut;
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("run");
    let cache_dir = tmp.path().join("cache");
    let state_dir = tmp.path().join("state");
    for dir in [&runtime_dir, &cache_dir, &state_dir] {
        tokio::fs::create_dir_all(dir).await.unwrap();
    }
    let daemon_env = [
        ("XDG_RUNTIME_DIR", runtime_dir.as_os_str()),
        ("RTTX_DEV_MODE", std::ffi::OsStr::new("")),
        ("XDG_CACHE_HOME", cache_dir.as_os_str()),
        ("XDG_STATE_HOME", state_dir.as_os_str()),
    ];

    let mut daemon = tokio::process::Command::new(bin)
        .args(["start", "--foreground"])
        .envs(daemon_env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn daemon");

    let socket = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(tokio::time::Instant::now() < deadline, "daemon socket did not appear");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut proxy = tokio::process::Command::new(bin)
        .arg("attach-stdio")
        .envs(daemon_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn attach-stdio");
    let mut stdin = proxy.stdin.take().unwrap();
    let mut stdout = proxy.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);
    let mut out_buf = BytesMut::new();

    let send = |env: &v3::ClientEnvelope, buf: &mut BytesMut| {
        buf.clear();
        rttx_proto::encode_frame(env, buf).unwrap();
        buf.split().freeze()
    };

    let mut caps = rttx_proto::v3_handshake::CORE_CAPABILITIES.to_vec();
    caps.push(v3::Capability::OptClipboardOsc52);
    let hello = rttx_proto::v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "test-stdio-clipboard",
        "0.0.0",
        &caps,
    );
    out_buf.clear();
    rttx_proto::encode_frame(&hello, &mut out_buf).unwrap();
    stdin.write_all(&out_buf).await.unwrap();
    stdin.flush().await.unwrap();
    let server_hello = loop {
        let n = stdout.read_buf(&mut read_buf).await.unwrap();
        assert!(n > 0, "unexpected EOF during handshake");
        if let Ok(sh) = rttx_proto::decode_frame::<v3::ServerHello>(&mut read_buf) {
            break sh;
        }
    };
    assert!(
        server_hello.capabilities.contains(&(v3::Capability::OptClipboardOsc52 as i32)),
        "a remote daemon must advertise the clipboard capability"
    );

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "remote-clip".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    let frame = send(&create, &mut out_buf);
    stdin.write_all(&frame).await.unwrap();
    stdin.flush().await.unwrap();
    let runtime_id = loop {
        match next_remote_envelope(&mut stdout, &mut read_buf).await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(c)) => break c.runtime_id,
            Some(_) => {}
            None => panic!("empty envelope"),
        }
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    let frame = send(&create_pane, &mut out_buf);
    stdin.write_all(&frame).await.unwrap();
    stdin.flush().await.unwrap();
    let pane_id = loop {
        match next_remote_envelope(&mut stdout, &mut read_buf).await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(p)) => break p.pane_id,
            Some(_) => {}
            None => panic!("empty envelope"),
        }
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    let frame = send(&attach, &mut out_buf);
    stdin.write_all(&frame).await.unwrap();
    stdin.flush().await.unwrap();
    loop {
        match next_remote_envelope(&mut stdout, &mut read_buf).await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(_) => {}
            None => panic!("empty envelope"),
        }
    }

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                // "from the remote host" base64-encoded.
                data: bytes::Bytes::from_static(
                    b"printf '\\033]52;c;ZnJvbSB0aGUgcmVtb3RlIGhvc3Q=\\007'\n",
                ),
            })),
        })),
    };
    let frame = send(&input, &mut out_buf);
    stdin.write_all(&frame).await.unwrap();
    stdin.flush().await.unwrap();

    let mut deltas = Vec::new();
    let write = loop {
        match next_remote_envelope(&mut stdout, &mut read_buf).await.payload {
            Some(v3::server_envelope::Payload::ClipboardWrite(w)) => break w,
            Some(v3::server_envelope::Payload::OutputDelta(d)) => deltas.extend(d.data.to_vec()),
            Some(_) => {}
            None => panic!("empty envelope"),
        }
    };
    assert_eq!(write.data, bytes::Bytes::from_static(b"from the remote host"));
    assert_eq!(write.pane_id, pane_id);
    assert_no_osc52_bytes(&deltas);

    drop(stdin);
    daemon.kill().await.ok();
}

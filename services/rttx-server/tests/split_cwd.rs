mod common;

use common::{TestClient, create_pane_with_cwd, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// Pane created with a CWD should spawn its shell in that directory. #297.
#[tokio::test]
async fn create_pane_with_cwd_spawns_in_target_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "cwd-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    let target_dir = std::env::temp_dir();
    let canonical_target =
        std::fs::canonicalize(&target_dir).unwrap_or_else(|_| target_dir.clone());
    let target_str = canonical_target.to_string_lossy().to_string();

    let pane_id = create_pane_with_cwd(&mut client, &runtime_id, Some(target_str.clone())).await;

    // Attach to receive output.
    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;

    // Drain the snapshot.
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    // Send `pwd` and read output.
    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"pwd\n"),
            })),
        })),
    };
    client.send(&input).await;

    // Collect output until we see the target directory.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&target_str) {
                return; // Success
            }
        }
    }
    panic!(
        "pwd output did not contain target directory {target_str:?} within timeout.\nOutput: {output}"
    );
}

/// Pane created without CWD should still work (spawns in default directory). #297.
#[tokio::test]
async fn create_pane_without_cwd_uses_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "no-cwd-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Should not panic — None CWD is valid.
    let _pane_id = create_pane_with_cwd(&mut client, &runtime_id, None).await;
}

/// Pane created without CWD starts in the user's home directory, not the
/// daemon's working directory. Regression test for #644.
#[tokio::test]
async fn create_pane_without_cwd_starts_in_home_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "home-cwd-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    let pane_id = create_pane_with_cwd(&mut client, &runtime_id, None).await;

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"pwd\n"),
            })),
        })),
    };
    client.send(&input).await;

    let home = std::env::var("HOME").expect("HOME must be set");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&home) {
                return;
            }
        }
    }
    panic!("pwd output did not contain home directory {home:?} within timeout.\nOutput: {output}");
}

/// When a second pane is created without an explicit CWD, the daemon
/// falls back to the effective CWD of an existing pane in the same
/// workspace. Regression test for #773.
#[tokio::test]
async fn create_pane_without_cwd_inherits_sibling_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "sibling-cwd-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    let target_dir = std::env::temp_dir();
    let canonical_target =
        std::fs::canonicalize(&target_dir).unwrap_or_else(|_| target_dir.clone());
    let target_str = canonical_target.to_string_lossy().to_string();

    // First pane: explicit CWD.
    let _first_pane =
        create_pane_with_cwd(&mut client, &runtime_id, Some(target_str.clone())).await;

    // Second pane: no CWD — should inherit from the first pane.
    let second_pane = create_pane_with_cwd(&mut client, &runtime_id, None).await;

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: second_pane.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"pwd\n"),
            })),
        })),
    };
    client.send(&input).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&target_str) {
                return;
            }
        }
    }
    panic!(
        "second pane pwd should contain sibling CWD {target_str:?} within timeout.\nOutput: {output}"
    );
}

/// Pane created with a tilde CWD (`~`) should expand to $HOME. Regression
/// test for #905.
#[tokio::test]
async fn create_pane_with_tilde_cwd_expands_to_home() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "tilde-cwd-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    let pane_id = create_pane_with_cwd(&mut client, &runtime_id, Some("~".into())).await;

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"pwd\n"),
            })),
        })),
    };
    client.send(&input).await;

    let home = std::env::var("HOME").expect("HOME must be set");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&home) {
                return;
            }
        }
    }
    panic!("tilde CWD pane should start in $HOME ({home:?}) within timeout.\nOutput: {output}");
}

/// Pane created with a tilde-prefixed CWD (`~/subdir`) should expand to
/// `$HOME/subdir`. Regression test for #905.
#[tokio::test]
async fn create_pane_with_tilde_prefix_cwd_expands_correctly() {
    let home = std::env::var("HOME").expect("HOME must be set");
    // Create a temporary subdirectory under $HOME for the test.
    let subdir_name = format!("rttx-test-{}", uuid::Uuid::new_v4());
    let subdir_path = std::path::PathBuf::from(&home).join(&subdir_name);
    std::fs::create_dir(&subdir_path).expect("create test subdir under $HOME");

    let tilde_path = format!("~/{subdir_name}");
    let expected_abs = subdir_path.to_string_lossy().to_string();

    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "tilde-prefix-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    let pane_id = create_pane_with_cwd(&mut client, &runtime_id, Some(tilde_path)).await;

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"pwd\n"),
            })),
        })),
    };
    client.send(&input).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&expected_abs) {
                let _ = std::fs::remove_dir(&subdir_path);
                return;
            }
        }
    }
    let _ = std::fs::remove_dir(&subdir_path);
    panic!(
        "tilde-prefix CWD pane should start in {expected_abs:?} within timeout.\nOutput: {output}"
    );
}

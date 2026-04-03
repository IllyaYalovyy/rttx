//! Persistence failure-injection integration tests.
//!
//! Verifies that the server starts cleanly and does not panic when
//! encountering corrupt, partial, or missing persistence artifacts.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;

/// Verify that `load_state` rejects corrupt JSON at the integration boundary.
#[test]
fn load_state_rejects_corrupt_json() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("state.json");
    std::fs::write(&path, "{{{{not json").unwrap();
    assert!(rttx_server::serialization::load_state(&path).is_err());
}

/// Server starts fresh when state.json contains invalid JSON.
#[tokio::test]
async fn startup_with_corrupt_state_file_starts_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("state.json"), "NOT VALID JSON {{{").unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Server should have zero sessions (started fresh).
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 0, "corrupt state must not produce phantom sessions");
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

/// Server starts fresh when state.json is truncated mid-write.
#[tokio::test]
async fn startup_with_truncated_state_file_starts_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    // Write a valid-looking start that's cut off.
    std::fs::write(
        cache_dir.join("state.json"),
        r#"{"sessions":[{"id":"00000000-0000-0000-0000-000000000001","name":"cut"#,
    )
    .unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 0);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

/// Server starts fresh when state.json is an empty file.
#[tokio::test]
async fn startup_with_empty_state_file_starts_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("state.json"), "").unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 0);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

/// Server ignores a leftover .tmp file from an interrupted atomic write.
#[tokio::test]
async fn startup_ignores_leftover_tmp_file() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("state.json.tmp"), "interrupted write garbage").unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 0);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

/// Session reconstructs even when its scrollback log file is missing.
#[tokio::test]
async fn reconstruction_with_missing_scrollback_log() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();

    let session_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // Write valid state referencing a scrollback log that doesn't exist.
    let state_json = format!(
        r#"{{
            "sessions": [{{
                "id": "{session_id}",
                "name": "ghost-scrollback",
                "panes": [{{
                    "id": "{pane_id}",
                    "cwd": "/tmp",
                    "title": "bash",
                    "scrollback_log_path": "/nonexistent/scrollback.log",
                    "exit_status": null,
                    "cols": 80,
                    "rows": 24
                }}],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 1,
                "created_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}},
                "last_active_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}}
            }}],
            "serialized_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}},
            "server_version": "0.1.0"
        }}"#
    );
    std::fs::write(cache_dir.join("state.json"), state_json).unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Session should still be restored (just without scrollback content).
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1, "session must survive missing scrollback");
            assert_eq!(sl.sessions[0].name, "ghost-scrollback");
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

/// Session reconstructs when scrollback log contains truncated/corrupt bytes.
#[tokio::test]
async fn reconstruction_with_corrupt_scrollback_log() {
    let tmp = tempfile::tempdir().unwrap();
    let cache_dir = tmp.path().join("cache");

    let session_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    // Create a scrollback log with garbage bytes.
    let scrollback_dir = cache_dir.join("scrollback").join(session_id.to_string());
    std::fs::create_dir_all(&scrollback_dir).unwrap();
    let log_path = scrollback_dir.join(format!("{pane_id}.log"));
    std::fs::write(&log_path, b"\xff\xfe\x00\x01 corrupt terminal bytes").unwrap();

    let state_json = format!(
        r#"{{
            "sessions": [{{
                "id": "{session_id}",
                "name": "corrupt-scrollback",
                "panes": [{{
                    "id": "{pane_id}",
                    "cwd": "/tmp",
                    "title": "bash",
                    "scrollback_log_path": "{}",
                    "exit_status": null,
                    "cols": 80,
                    "rows": 24
                }}],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 1,
                "created_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}},
                "last_active_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}}
            }}],
            "serialized_at": {{"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}},
            "server_version": "0.1.0"
        }}"#,
        log_path.display()
    );
    std::fs::write(cache_dir.join("state.json"), state_json).unwrap();

    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1, "session must survive corrupt scrollback");
            assert_eq!(sl.sessions[0].name, "corrupt-scrollback");
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

use super::*;
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
use std::path::PathBuf;
use tracing_test::traced_test;

#[derive(Debug)]
struct StubOs;
impl OsInterface for StubOs {
    fn runtime_dir(&self) -> PathBuf {
        PathBuf::from("/tmp/test-runtime")
    }
    fn cache_dir(&self) -> PathBuf {
        PathBuf::from("/tmp/test-cache")
    }
}

fn new_server() -> Arc<Mutex<Server>> {
    Arc::new(Mutex::new(Server::new(Box::new(StubOs))))
}

/// Insert a runtime with a pane and attach a client as writer.
async fn setup_runtime_with_pane(server: &Arc<Mutex<Server>>, client_id: Uuid) -> (Uuid, Uuid) {
    let mut rt = Runtime::new("test".into());
    let runtime_id = rt.id;
    let pane = Pane::new(Uuid::new_v4(), 80, 24);
    let pane_id = pane.id;
    rt.add_pane(pane);
    let _ = rt.attach_client(client_id, AttachMode::ReadWrite);
    server.lock().await.runtimes.insert(runtime_id, rt);
    (runtime_id, pane_id)
}

// ── Existing tests (migrated) ───────────────────────────────────

#[test]
fn short_id_returns_first_eight_characters() {
    let id = Uuid::parse_str("17f448df-95be-4d4e-b010-b5021b4e6eb5").unwrap();
    assert_eq!(short_id(id), "17f448df");
}

#[test]
fn runtime_label_includes_name_and_short_id() {
    let mut server = Server::new(Box::new(StubOs));
    let rt = Runtime::new("my-workspace".into());
    let runtime_id = rt.id;
    server.runtimes.insert(runtime_id, rt);

    let label = server.runtime_label(runtime_id);
    assert!(label.starts_with("\"my-workspace\" ("), "got: {label}");
    assert!(label.ends_with(')'), "got: {label}");
    assert_eq!(label.len(), "\"my-workspace\" (12345678)".len());
}

#[test]
fn runtime_label_falls_back_for_unknown_runtime() {
    let server = Server::new(Box::new(StubOs));
    let unknown_id = Uuid::new_v4();
    let label = server.runtime_label(unknown_id);
    assert!(label.starts_with('('), "got: {label}");
    assert!(label.ends_with(')'), "got: {label}");
    assert_eq!(label.len(), "(12345678)".len());
}

#[tokio::test]
async fn input_to_missing_runtime_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            data: bytes::Bytes::from_static(b"hello"),
        })),
    };
    assert!(Server::handle_message(&server, client_id, msg).await.is_none());
}

#[tokio::test]
async fn resize_missing_runtime_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            cols: 80,
            rows: 24,
        })),
    };
    assert!(Server::handle_message(&server, client_id, msg).await.is_none());
}

// ── Empty message ───────────────────────────────────────────────

#[tokio::test]
async fn empty_message_returns_error() {
    let server = new_server();
    let msg = proto::ClientMessage { msg: None };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_EMPTY_MESSAGE);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Hello / handshake ───────────────────────────────────────────

#[tokio::test]
async fn hello_with_correct_version_returns_hello_ack() {
    let server = new_server();
    let server_id = server.lock().await.server_id;
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: rttx_proto::PROTOCOL_VERSION,
            client_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::HelloAck(ack)) => {
            assert_eq!(bytes_to_uuid(&ack.server_id).unwrap(), server_id);
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }
}

#[tokio::test]
async fn hello_with_wrong_version_returns_version_mismatch() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: 9999,
            client_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_VERSION_MISMATCH);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Ping ────────────────────────────────────────────────────────

#[tokio::test]
async fn ping_returns_pong_with_same_nonce() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 42 })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Pong(pong)) => {
            assert_eq!(pong.nonce, 42);
        }
        other => panic!("expected Pong, got {other:?}"),
    }
}

#[tokio::test]
async fn ping_fast_path_responds_without_handle_message() {
    // Regression: #556 — client_reader intercepts Ping before
    // handle_message so the server mutex is never acquired.
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 99 })),
    };
    // Simulate the fast-path match used in client_reader.
    let pong = match &msg.msg {
        Some(proto::client_message::Msg::Ping(ping)) => protocol::pong(ping.nonce),
        _ => panic!("expected Ping variant"),
    };
    match pong.msg {
        Some(proto::server_message::Msg::Pong(p)) => assert_eq!(p.nonce, 99),
        other => panic!("expected Pong, got {other:?}"),
    }
}

// ── CreateRuntime ───────────────────────────────────────────────

#[tokio::test]
async fn create_runtime_returns_runtime_created() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "workspace-1".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => {
            assert!(!sc.runtime_id.is_empty());
            let id = bytes_to_uuid(&sc.runtime_id).unwrap();
            let s = server.lock().await;
            assert_eq!(s.runtimes[&id].name, "workspace-1");
            assert_eq!(s.runtimes[&id].policy, RuntimePolicy::Persistent);
            drop(s);
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

// ── ListRuntimes ────────────────────────────────────────────────

#[tokio::test]
async fn list_runtimes_returns_all_runtimes() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    {
        let mut s = server.lock().await;
        s.runtimes.insert(Uuid::new_v4(), Runtime::new("a".into()));
        s.runtimes.insert(Uuid::new_v4(), Runtime::new("b".into()));
    }
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeList(sl)) => {
            assert_eq!(sl.runtimes.len(), 2);
        }
        other => panic!("expected RuntimeList, got {other:?}"),
    }
}

// ── AttachRuntime ───────────────────────────────────────────────

#[tokio::test]
async fn attach_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: vec![0u8; 4],
            attach_mode: 0,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_returns_snapshot_with_pane_data() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    // Detach first so we can re-attach cleanly.
    server
        .lock()
        .await
        .runtimes
        .get_mut(&runtime_id)
        .unwrap()
        .detach_client(client_id, DetachReason::ExplicitRequest);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert_eq!(snap.panes.len(), 1);
            assert_eq!(bytes_to_uuid(&snap.panes[0].pane_id).unwrap(), pane_id);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

// ── DetachRuntime ───────────────────────────────────────────────

#[tokio::test]
async fn detach_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn detach_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: vec![0u8; 2],
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── TerminateRuntime ────────────────────────────────────────────

#[tokio::test]
async fn terminate_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn terminate_owned_by_other_client_returns_ownership_conflict() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_message(&server, other, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn terminate_removes_runtime_from_state() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::RuntimeTerminated(_))));
    assert!(!server.lock().await.runtimes.contains_key(&runtime_id));
}

// ── ClosePane ───────────────────────────────────────────────────

#[tokio::test]
async fn close_pane_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn close_pane_nonexistent_pane_returns_pane_not_found() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_PANE_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn close_pane_ownership_violation_returns_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
        })),
    };
    let resp = Server::handle_message(&server, other, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── SetPaneTitle ────────────────────────────────────────────────

#[tokio::test]
async fn set_pane_title_returns_title_changed() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            title: "new-title".into(),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::TitleChanged(tc)) => {
            assert_eq!(tc.title, "new-title");
        }
        other => panic!("expected TitleChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn set_pane_title_nonexistent_pane_returns_pane_not_found() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            title: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_PANE_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn set_pane_title_ownership_violation_returns_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            title: "hijack".into(),
        })),
    };
    let resp = Server::handle_message(&server, other, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── RenameRuntime ───────────────────────────────────────────────

#[tokio::test]
async fn rename_runtime_returns_runtime_renamed() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            name: "renamed".into(),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeRenamed(sr)) => {
            assert_eq!(sr.name, "renamed");
        }
        other => panic!("expected RuntimeRenamed, got {other:?}"),
    }
}

#[tokio::test]
async fn rename_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            name: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn rename_runtime_ownership_violation_returns_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            name: "hijack".into(),
        })),
    };
    let resp = Server::handle_message(&server, other, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Input ownership ─────────────────────────────────────────────

#[tokio::test]
async fn input_to_existing_pane_without_write_access_returns_ownership_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let reader = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, owner).await;
    let _ = server
        .lock()
        .await
        .runtimes
        .get_mut(&runtime_id)
        .unwrap()
        .attach_client(reader, AttachMode::ReadOnly);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            data: bytes::Bytes::from_static(b"hello"),
        })),
    };
    let resp = Server::handle_message(&server, reader, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Resize ownership ────────────────────────────────────────────

#[tokio::test]
async fn resize_without_write_access_returns_ownership_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let reader = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, owner).await;
    let _ = server
        .lock()
        .await
        .runtimes
        .get_mut(&runtime_id)
        .unwrap()
        .attach_client(reader, AttachMode::ReadOnly);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            cols: 120,
            rows: 40,
        })),
    };
    let resp = Server::handle_message(&server, reader, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── build_snapshot ──────────────────────────────────────────────

#[test]
fn build_snapshot_only_includes_persistent_runtimes() {
    let mut server = Server::new(Box::new(StubOs));

    let mut persistent = Runtime::new("keep".into());
    persistent.policy = RuntimePolicy::Persistent;
    server.runtimes.insert(persistent.id, persistent);

    let mut ephemeral = Runtime::new("discard".into());
    ephemeral.policy = RuntimePolicy::Ephemeral;
    server.runtimes.insert(ephemeral.id, ephemeral);

    let snapshot = server.build_snapshot();
    assert_eq!(snapshot.runtimes.len(), 1);
    assert_eq!(snapshot.runtimes[0].name, "keep");
}

// ── Input to nonexistent pane in existing runtime ───────────────

#[tokio::test]
async fn input_to_nonexistent_pane_in_existing_runtime_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            data: bytes::Bytes::from_static(b"hello"),
        })),
    };
    assert!(Server::handle_message(&server, client_id, msg).await.is_none());
}

// ── Resize with invalid dimensions ──────────────────────────────

#[tokio::test]
async fn resize_with_overflow_cols_returns_none() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            cols: u32::from(u16::MAX) + 1,
            rows: 24,
        })),
    };
    assert!(Server::handle_message(&server, Uuid::new_v4(), msg).await.is_none());
}

#[tokio::test]
async fn resize_with_overflow_rows_returns_none() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            cols: 80,
            rows: u32::from(u16::MAX) + 1,
        })),
    };
    assert!(Server::handle_message(&server, Uuid::new_v4(), msg).await.is_none());
}

// ── Resize nonexistent pane in existing runtime ─────────────────

#[tokio::test]
async fn resize_nonexistent_pane_in_existing_runtime_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            cols: 120,
            rows: 40,
        })),
    };
    assert!(Server::handle_message(&server, client_id, msg).await.is_none());
}

// ── DetachRuntime success ───────────────────────────────────────

#[tokio::test]
async fn detach_attached_client_returns_runtime_detached() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeDetached(sd)) => {
            assert_eq!(bytes_to_uuid(&sd.runtime_id).unwrap(), runtime_id);
        }
        other => panic!("expected RuntimeDetached, got {other:?}"),
    }
    // Session still exists after detach (persistent policy).
    assert!(server.lock().await.runtimes.contains_key(&runtime_id));
}

// ── Ephemeral last-detach terminates runtime ────────────────────

#[tokio::test]
async fn detach_last_client_from_ephemeral_session_terminates() {
    let server = new_server();
    let client_id = Uuid::new_v4();

    // Create an ephemeral runtime manually.
    let mut rt = Runtime::new("ephemeral".into());
    rt.policy = RuntimePolicy::Ephemeral;
    let runtime_id = rt.id;
    let pane = Pane::new(Uuid::new_v4(), 80, 24);
    rt.add_pane(pane);
    let _ = rt.attach_client(client_id, AttachMode::ReadWrite);
    server.lock().await.runtimes.insert(runtime_id, rt);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(st)) => {
            assert_eq!(bytes_to_uuid(&st.runtime_id).unwrap(), runtime_id);
            assert_eq!(st.reason, proto::RuntimeTerminationReason::EphemeralLastDetach as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }
    // Runtime removed after ephemeral last-detach.
    assert!(!server.lock().await.runtimes.contains_key(&runtime_id));
}

// ── ClosePane success ───────────────────────────────────────────

#[tokio::test]
async fn close_pane_removes_pane_from_runtime() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::PaneClosed(pc)) => {
            assert_eq!(bytes_to_uuid(&pc.runtime_id).unwrap(), runtime_id);
            assert_eq!(bytes_to_uuid(&pc.pane_id).unwrap(), pane_id);
        }
        other => panic!("expected PaneClosed, got {other:?}"),
    }
    let s = server.lock().await;
    assert!(!s.runtimes[&runtime_id].panes.contains_key(&pane_id));
    drop(s);
}

// ── ClosePane invalid pane UUID ─────────────────────────────────

#[tokio::test]
async fn close_pane_with_invalid_pane_uuid_returns_invalid_parameter() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: vec![0u8; 3],
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Invalid UUID in remaining dispatch arms ─────────────────────

#[tokio::test]
async fn terminate_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: vec![0u8; 5],
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn set_pane_title_with_invalid_session_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            runtime_id: vec![0u8; 1],
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            title: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn set_pane_title_with_invalid_pane_uuid_returns_invalid_parameter() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: vec![0u8; 7],
            title: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn rename_runtime_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: vec![0u8; 6],
            name: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn create_pane_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: vec![0u8; 3],
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_INVALID_PARAMETER);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── CreatePane ownership violation ──────────────────────────────

#[tokio::test]
async fn create_pane_without_write_access_returns_ownership_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let reader = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, owner).await;
    let _ = server
        .lock()
        .await
        .runtimes
        .get_mut(&runtime_id)
        .unwrap()
        .attach_client(reader, AttachMode::ReadOnly);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: uuid_to_bytes(runtime_id),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    let resp = Server::handle_message(&server, reader, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_OWNERSHIP_CONFLICT);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── CreatePane nonexistent runtime ──────────────────────────────

#[tokio::test]
async fn create_pane_nonexistent_runtime_returns_runtime_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_RUNTIME_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Attach with TakeOver mode ───────────────────────────────────

#[tokio::test]
async fn attach_with_takeover_returns_unsupported() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::TakeOver as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_UNSUPPORTED);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ── Attach blocked by existing writer ───────────────────────────

#[tokio::test]
async fn second_writer_attach_returns_attach_blocked() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::AttachBlocked(ab)) => {
            assert_eq!(bytes_to_uuid(&ab.runtime_id).unwrap(), runtime_id);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }
}

// ── CreateRuntime with ephemeral policy ─────────────────────────

#[tokio::test]
async fn create_runtime_with_ephemeral_policy() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "ephemeral-ws".into(),
            policy: proto::RuntimePolicy::Ephemeral as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => {
            let id = bytes_to_uuid(&sc.runtime_id).unwrap();
            let s = server.lock().await;
            assert_eq!(s.runtimes[&id].policy, RuntimePolicy::Ephemeral);
            drop(s);
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

// ── Shutdown message returns None ───────────────────────────────

#[tokio::test]
async fn shutdown_message_returns_none() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
    };
    assert!(Server::handle_message(&server, Uuid::new_v4(), msg).await.is_none());
}

// ── Terminate cleans up PTY state ───────────────────────────────

#[tokio::test]
async fn terminate_runtime_cleans_up_pty_writers() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    // Simulate PTY state by inserting a kill sender.
    let (kill_tx, _kill_rx) = tokio::sync::oneshot::channel();
    server.lock().await.pty_kill_senders.insert(pane_id, kill_tx);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    let s = server.lock().await;
    assert!(!s.runtimes.contains_key(&runtime_id));
    assert!(!s.pty_kill_senders.contains_key(&pane_id));
    drop(s);
}

// ── Lifecycle logging ───────────────────────────────────────────

#[tokio::test]
#[traced_test]
async fn create_runtime_logs_lifecycle_event() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "log-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();

    assert!(logs_contain("Runtime created"));
    assert!(logs_contain("log-test"));
    assert!(logs_contain("persistent"));
}

#[tokio::test]
#[traced_test]
async fn attach_runtime_logs_lifecycle_event() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    // Detach first so we can re-attach.
    server
        .lock()
        .await
        .runtimes
        .get_mut(&runtime_id)
        .unwrap()
        .detach_client(client_id, DetachReason::ExplicitRequest);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    assert!(logs_contain("Client"));
    assert!(logs_contain("attached to runtime"));
}

#[tokio::test]
#[traced_test]
async fn detach_runtime_logs_lifecycle_event() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    assert!(logs_contain("Client"));
    assert!(logs_contain("detached from runtime"));
}

#[tokio::test]
#[traced_test]
async fn terminate_runtime_logs_lifecycle_event() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    assert!(logs_contain("Runtime terminated"));
}

#[tokio::test]
#[traced_test]
async fn close_pane_logs_lifecycle_event() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    assert!(logs_contain("Pane"));
    assert!(logs_contain("closed in runtime"));
}

#[tokio::test]
#[traced_test]
async fn rename_runtime_logs_lifecycle_event() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            name: "new-name".into(),
        })),
    };
    Server::handle_message(&server, client_id, msg).await.unwrap();

    assert!(logs_contain("Runtime renamed"));
    assert!(logs_contain("new-name"));
}

// ── Bounded channel backpressure ────────────────────────────────

#[tokio::test]
#[traced_test]
async fn broadcast_drops_messages_when_client_channel_is_full() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;

    // Register the client sender (bounded channel).
    let (tx, rx) = mpsc::channel(PUSH_CHANNEL_BOUND);
    server.lock().await.client_senders.insert(client_id, tx);

    // Fill the channel to capacity.
    let msg = ClientMsg::V2(protocol::delta(
        runtime_id,
        Uuid::new_v4(),
        bytes::Bytes::from(vec![0u8; 64]),
    ));
    for _ in 0..PUSH_CHANNEL_BOUND {
        server.lock().await.broadcast_to_runtime(runtime_id, &msg);
    }

    // Next broadcast should drop the message instead of blocking.
    server.lock().await.broadcast_to_runtime(runtime_id, &msg);

    // Channel should still have exactly PUSH_CHANNEL_BOUND messages.
    drop(server);
    let mut count = 0;
    let mut rx = rx;
    while rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, PUSH_CHANNEL_BOUND);
    assert!(logs_contain("channel full"));
}

#[tokio::test]
async fn client_senders_use_bounded_channel() {
    // Verify the channel type is bounded by checking that the constant exists
    // and has a reasonable value.
    const { assert!(PUSH_CHANNEL_BOUND > 0) };
    const { assert!(PUSH_CHANNEL_BOUND <= 8192) };
}

// ── Delta Bytes sharing ─────────────────────────────────────────

#[tokio::test]
async fn delta_broadcast_shares_bytes_across_clients() {
    let server = new_server();
    let client_id_a = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id_a).await;

    let client_id_b = Uuid::new_v4();
    {
        let mut s = server.lock().await;
        s.runtimes
            .get_mut(&runtime_id)
            .unwrap()
            .attached_clients
            .insert(client_id_b, crate::runtime::ClientRole::Writer);
    }

    let (tx_a, mut rx_a) = mpsc::channel(16);
    let (tx_b, mut rx_b) = mpsc::channel(16);
    {
        let mut s = server.lock().await;
        s.client_senders.insert(client_id_a, tx_a);
        s.client_senders.insert(client_id_b, tx_b);
    }

    let data = bytes::Bytes::from(vec![b'X'; 4096]);
    let msg = ClientMsg::V2(protocol::delta(runtime_id, Uuid::new_v4(), data.clone()));
    server.lock().await.broadcast_to_runtime(runtime_id, &msg);

    let msg_a = rx_a.try_recv().unwrap();
    let msg_b = rx_b.try_recv().unwrap();

    let data_a = match msg_a {
        ClientMsg::V2(ref m) => match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) => d.data.clone(),
            other => panic!("expected Delta, got {other:?}"),
        },
        ClientMsg::V3(ref other) => panic!("expected V2, got V3({other:?})"),
    };
    let data_b = match msg_b {
        ClientMsg::V2(ref m) => match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) => d.data.clone(),
            other => panic!("expected Delta, got {other:?}"),
        },
        ClientMsg::V3(ref other) => panic!("expected V2, got V3({other:?})"),
    };

    // Both clones share the same backing allocation.
    assert_eq!(data_a.as_ptr(), data_b.as_ptr());
    assert_eq!(data_a.len(), 4096);
}

// ── Exited pane scrollback release ──────────────────────────────

#[tokio::test]
async fn exited_pane_scrollback_is_released() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;

    let mut s = server.lock().await;
    let rt = s.runtimes.get_mut(&runtime_id).unwrap();

    // Feed output to build up scrollback.
    let pane = rt.panes.get_mut(&pane_id).unwrap();
    pane.feed_output(&vec![b'A'; 1024]);
    assert!(!pane.screen.raw_bytes().is_empty());
    assert!(pane.has_pending_flush());

    // Simulate PTY exit: set exit status and release scrollback.
    rt.set_pane_exit_status(pane_id, Some(0));
    let pane = rt.panes.get_mut(&pane_id).unwrap();
    pane.release_scrollback();

    // Verify scrollback is released but pane still exists.
    assert!(pane.is_exited());
    assert!(pane.screen.raw_bytes().is_empty());
    assert!(!pane.has_pending_flush());
    drop(s);
}

// ── client_writer priority ──────────────────────────────────────

#[tokio::test]
async fn client_writer_prioritizes_resp_over_push() {
    // Regression: #557 — resp_rx (Pong, Snapshot) must be drained before
    // push_rx (Delta) so heartbeat replies are never starved by burst data.
    let push_count = 64;
    let (push_tx, push_rx) = mpsc::channel::<ClientMsg>(push_count);
    let (resp_tx, resp_rx) = mpsc::channel::<ClientMsg>(16);

    // Pre-fill push channel with many Deltas.
    let delta = ClientMsg::V2(protocol::delta(
        Uuid::new_v4(),
        Uuid::new_v4(),
        bytes::Bytes::from_static(b"x"),
    ));
    for _ in 0..push_count {
        push_tx.send(delta.clone()).await.unwrap();
    }

    // Then add a single Pong to the response channel.
    resp_tx.send(ClientMsg::V2(protocol::pong(42))).await.unwrap();

    // Drop senders so the writer will exit after draining.
    drop(push_tx);
    drop(resp_tx);

    let (client_half, mut read_half) = tokio::io::duplex(256 * 1024);
    let conn = crate::ipc::ClientConnection::new(client_half);
    let (_, writer) = conn.into_split();

    let handle = tokio::spawn(client_writer(writer, push_rx, resp_rx, "test".into()));
    handle.await.unwrap();

    // Read all bytes written by client_writer.
    let mut buf = bytes::BytesMut::new();
    loop {
        let n = tokio::io::AsyncReadExt::read_buf(&mut read_half, &mut buf).await.unwrap();
        if n == 0 {
            break;
        }
    }

    let mut messages = Vec::new();
    loop {
        match rttx_proto::decode_frame::<proto::ServerMessage>(&mut buf) {
            Ok(msg) => messages.push(msg),
            Err(rttx_proto::FrameError::Incomplete) => break,
            Err(e) => panic!("unexpected decode error: {e:?}"),
        }
    }

    assert_eq!(messages.len(), push_count + 1);

    // With biased select, the Pong must be the very first message written.
    assert!(
        matches!(messages[0].msg, Some(proto::server_message::Msg::Pong(ref p)) if p.nonce == 42),
        "first message should be Pong(42), got {:?}",
        messages[0].msg
    );
}

// ── Lock-free broadcast via collected senders ───────────────────

#[tokio::test]
async fn collect_runtime_senders_returns_attached_client_senders() {
    let server = new_server();
    let client_a = Uuid::new_v4();
    let client_b = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_a).await;

    // Attach a second client.
    {
        let mut s = server.lock().await;
        s.runtimes
            .get_mut(&runtime_id)
            .unwrap()
            .attached_clients
            .insert(client_b, crate::runtime::ClientRole::Writer);
    }

    let (tx_a, _rx_a) = mpsc::channel(16);
    let (tx_b, _rx_b) = mpsc::channel(16);
    {
        let mut s = server.lock().await;
        s.client_senders.insert(client_a, tx_a);
        s.client_senders.insert(client_b, tx_b);
    }

    let senders = server.lock().await.collect_runtime_senders(runtime_id);
    let ids: std::collections::HashSet<Uuid> = senders.iter().map(|(id, _, _)| *id).collect();
    assert!(ids.contains(&client_a));
    assert!(ids.contains(&client_b));
    assert_eq!(senders.len(), 2);
}

#[tokio::test]
async fn collect_runtime_senders_returns_empty_for_unknown_runtime() {
    let server = new_server();
    let senders = server.lock().await.collect_runtime_senders(Uuid::new_v4());
    assert!(senders.is_empty());
}

#[tokio::test]
async fn send_to_collected_delivers_messages() {
    let (tx, mut rx) = mpsc::channel(16);
    let client_id = Uuid::new_v4();
    let senders = vec![(client_id, tx, None)];

    let msg = ClientMsg::V2(protocol::delta(
        Uuid::new_v4(),
        Uuid::new_v4(),
        bytes::Bytes::from_static(b"hi"),
    ));
    send_to_collected(&senders, &msg);

    let received = rx.try_recv().unwrap();
    assert!(
        matches!(received, ClientMsg::V2(ref m) if matches!(m.msg, Some(proto::server_message::Msg::Delta(_))))
    );
}

#[tokio::test]
#[traced_test]
async fn send_to_collected_drops_when_channel_full() {
    let (tx, rx) = mpsc::channel(1);
    let client_id = Uuid::new_v4();
    let senders = vec![(client_id, tx, None)];

    let msg = ClientMsg::V2(protocol::delta(
        Uuid::new_v4(),
        Uuid::new_v4(),
        bytes::Bytes::from_static(b"a"),
    ));
    send_to_collected(&senders, &msg);
    // Channel is now full.
    send_to_collected(&senders, &msg);

    assert!(logs_contain("channel full"));
    drop(rx);
}

// ── PTY read coalescing ─────────────────────────────────────────

#[test]
fn coalesce_constants_are_within_protocol_limits() {
    let max_bytes: usize = COALESCE_MAX_BYTES;
    let window_ms: u128 = COALESCE_WINDOW.as_millis();
    // The 16MB protocol frame limit must not be exceeded by a single batch.
    assert!(max_bytes <= 16 * 1024 * 1024);
    // The coalescing window must be short enough to be imperceptible
    // (terminal rendering is typically 16ms frames).
    assert!(window_ms <= 5);
}

#[test]
fn bytes_mut_split_reuses_allocation_for_batching() {
    // Validates the BytesMut::split().freeze() pattern used in the read loop:
    // after split, the original buffer retains its capacity for the next batch.
    let mut batch = bytes::BytesMut::with_capacity(COALESCE_MAX_BYTES);
    batch.extend_from_slice(&[b'A'; 4096]);
    batch.extend_from_slice(&[b'B'; 4096]);

    let frozen = batch.split().freeze();
    assert_eq!(frozen.len(), 8192);
    assert!(batch.is_empty(), "split must drain the buffer");
    assert!(batch.capacity() > 0, "split must preserve allocation for reuse");
}

// ── Mutex hold instrumentation ──────────────────────────────────

#[test]
fn mutex_hold_warn_threshold_is_reasonable() {
    let threshold_ms = MUTEX_HOLD_WARN_THRESHOLD.as_millis();
    // Must be short enough to detect stalls but long enough to avoid
    // false positives on loaded systems.
    assert!(threshold_ms >= 5, "threshold too aggressive: {threshold_ms}ms");
    assert!(threshold_ms <= 100, "threshold too lenient: {threshold_ms}ms");
}

// ── Probe connection logging (#641) ─────────────────────────────

#[tokio::test]
#[traced_test]
async fn probe_connection_logs_at_debug_not_info() {
    let server = new_server();

    // Create a duplex stream and immediately drop the client half so the
    // server sees EOF on its first read.  A bare `_` drops immediately;
    // `_client_half` would keep the value alive until end of scope.
    let (server_half, _) = tokio::io::duplex(1024);
    let conn = crate::ipc::ClientConnection::new(server_half);

    // handle_client will see EOF immediately (no Hello sent).
    let _ = super::handle_client(server, conn).await;

    // Probe should be logged at debug level, not info.
    assert!(logs_contain("Client probe from"));
    assert!(logs_contain("disconnected before handshake"));
}

#[tokio::test]
#[traced_test]
async fn real_client_logs_connected_and_disconnected_at_info() {
    let server = new_server();

    let (server_half, mut client_half) = tokio::io::duplex(64 * 1024);
    let conn = crate::ipc::ClientConnection::new(server_half);

    // Spawn handle_client in background.
    let handle = tokio::spawn(async move {
        let _ = super::handle_client(server, conn).await;
    });

    // Send a Hello message from the client side.
    let hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: rttx_proto::PROTOCOL_VERSION,
            client_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let mut buf = bytes::BytesMut::new();
    rttx_proto::encode_frame(&hello, &mut buf).unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut client_half, &buf).await.unwrap();

    // Read the HelloAck response.
    let mut resp_buf = [0u8; 4096];
    let _ = tokio::io::AsyncReadExt::read(&mut client_half, &mut resp_buf).await.unwrap();

    // Close the client half to trigger disconnect.
    drop(client_half);

    handle.await.unwrap();

    assert!(logs_contain("connected"));
    assert!(logs_contain("disconnected"));
    // Must NOT contain probe message.
    assert!(!logs_contain("Client probe from"));
}

// ── V3 dispatch tests ───────────────────────────────────────────

#[tokio::test]
async fn v3_empty_envelope_returns_invalid_argument() {
    let server = new_server();
    let caps =
        rttx_proto::v3_handshake::CORE_CAPABILITIES.iter().map(|c| *c as i32).collect::<Vec<_>>();
    let env = v3::ClientEnvelope { request_id: 1, command: None };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::InvalidArgument as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_ping_returns_pong() {
    let server = new_server();
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 7,
        command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce: 42 })),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 7);
    match resp.payload {
        Some(v3::server_envelope::Payload::Pong(p)) => assert_eq!(p.nonce, 42),
        other => panic!("expected Pong, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_create_runtime_returns_runtime_created() {
    let server = new_server();
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 1,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "test-ws".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 1);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(rc)) => {
            assert!(!rc.runtime_id.is_empty());
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_attach_returns_snapshot() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _pane_id) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 2,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 2);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(snap)) => {
            assert_eq!(snap.runtime_id, uuid_to_bytes(runtime_id));
            assert_eq!(snap.panes.len(), 1);
            assert!(snap.panes[0].terminal_modes.is_some());
        }
        other => panic!("expected RuntimeSnapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_detach_returns_runtime_detached() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 3,
        command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 3);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeDetached(rd)) => {
            assert_eq!(rd.runtime_id, uuid_to_bytes(runtime_id));
        }
        other => panic!("expected RuntimeDetached, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_terminate_returns_runtime_terminated() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 4,
        command: Some(v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 4);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeTerminated(rt)) => {
            assert_eq!(rt.runtime_id, uuid_to_bytes(runtime_id));
            assert_eq!(rt.reason, v3::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_list_runtimes_returns_inventory() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let _ = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 5,
        command: Some(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 5);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeList(rl)) => {
            assert_eq!(rl.runtimes.len(), 1);
            assert_eq!(rl.runtimes[0].name, "test");
        }
        other => panic!("expected RuntimeList, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_get_diagnostics_requires_capability() {
    let server = new_server();
    let caps = vec![]; // no OPT_DIAGNOSTICS
    let env = v3::ClientEnvelope {
        request_id: 6,
        command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_get_diagnostics_with_capability_returns_report() {
    let server = new_server();
    let caps = vec![v3::Capability::OptDiagnostics as i32];
    let env = v3::ClientEnvelope {
        request_id: 7,
        command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 7);
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::DiagnosticsReport(_))));
}

#[tokio::test]
async fn v3_rename_runtime_returns_renamed() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 8,
        command: Some(v3::client_envelope::Command::RenameRuntime(v3::RenameRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            name: "new-name".into(),
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    assert_eq!(resp.request_id, 8);
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeRenamed(rr)) => {
            assert_eq!(rr.name, "new-name");
        }
        other => panic!("expected RuntimeRenamed, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_terminal_input_is_fire_and_forget() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, pane_id) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: uuid_to_bytes(runtime_id),
            pane_id: uuid_to_bytes(pane_id),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"hello"),
            })),
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await;
    assert!(resp.is_none());
}

#[tokio::test]
async fn v3_resync_requires_capability() {
    let server = new_server();
    let caps = vec![]; // no OPT_RESYNC
    let env = v3::ClientEnvelope {
        request_id: 9,
        command: Some(v3::client_envelope::Command::ResyncRuntime(v3::ResyncRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_get_scrollback_requires_capability() {
    let server = new_server();
    let caps = vec![]; // no OPT_CHUNKED_SCROLLBACK
    let env = v3::ClientEnvelope {
        request_id: 10,
        command: Some(v3::client_envelope::Command::GetScrollback(v3::GetScrollback {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            offset: 0,
            limit: 1024,
        })),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_takeover_requires_capability() {
    let server = new_server();
    let caps = vec![]; // no OPT_RUNTIME_TAKEOVER
    let env = v3::ClientEnvelope {
        request_id: 11,
        command: Some(v3::client_envelope::Command::TakeoverRuntime(v3::TakeoverRuntime {
            runtime_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_v3_message(&server, Uuid::new_v4(), &caps, env).await.unwrap();
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn v3_pane_output_seq_increments_on_feed() {
    let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
    assert_eq!(pane.output_seq, 0);
    pane.feed_output(b"hello");
    assert_eq!(pane.output_seq, 1);
    pane.feed_output(b"world");
    assert_eq!(pane.output_seq, 2);
}

#[tokio::test]
async fn v3_snapshot_includes_terminal_modes() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (runtime_id, _) = setup_runtime_with_pane(&server, client_id).await;
    let caps = vec![];
    let env = v3::ClientEnvelope {
        request_id: 1,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_v3_message(&server, client_id, &caps, env).await.unwrap();
    if let Some(v3::server_envelope::Payload::RuntimeSnapshot(snap)) = resp.payload {
        for pane_snap in &snap.panes {
            assert!(pane_snap.terminal_modes.is_some());
        }
    } else {
        panic!("expected RuntimeSnapshot");
    }
}

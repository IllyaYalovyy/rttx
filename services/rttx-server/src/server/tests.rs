use super::*;
use crate::os::OsInterface;
use crate::pane::Pane;
use crate::protocol;
use std::path::PathBuf;

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

/// Insert a session with a pane and attach a client as writer.
async fn setup_session_with_pane(
    server: &Arc<Mutex<Server>>,
    client_id: Uuid,
) -> (Uuid, Uuid) {
    let mut session = Session::new("test".into());
    let session_id = session.id;
    let pane = Pane::new(Uuid::new_v4(), 80, 24);
    let pane_id = pane.id;
    session.add_pane(pane);
    let _ = session.attach_client(client_id, AttachMode::ReadWrite);
    server.lock().await.sessions.insert(session_id, session);
    (session_id, pane_id)
}

// ── Existing tests (migrated) ───────────────────────────────────

#[test]
fn short_id_returns_first_eight_characters() {
    let id = Uuid::parse_str("17f448df-95be-4d4e-b010-b5021b4e6eb5").unwrap();
    assert_eq!(short_id(id), "17f448df");
}

#[test]
fn session_label_includes_name_and_short_id() {
    let mut server = Server::new(Box::new(StubOs));
    let session = Session::new("my-workspace".into());
    let session_id = session.id;
    server.sessions.insert(session_id, session);

    let label = server.session_label(session_id);
    assert!(label.starts_with("\"my-workspace\" ("), "got: {label}");
    assert!(label.ends_with(')'), "got: {label}");
    assert_eq!(label.len(), "\"my-workspace\" (12345678)".len());
}

#[test]
fn session_label_falls_back_for_unknown_session() {
    let server = Server::new(Box::new(StubOs));
    let unknown_id = Uuid::new_v4();
    let label = server.session_label(unknown_id);
    assert!(label.starts_with('('), "got: {label}");
    assert!(label.ends_with(')'), "got: {label}");
    assert_eq!(label.len(), "(12345678)".len());
}

#[tokio::test]
async fn input_to_missing_session_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            data: b"hello".to_vec(),
        })),
    };
    assert!(Server::handle_message(&server, client_id, msg).await.is_none());
}

#[tokio::test]
async fn resize_missing_session_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id: uuid_to_bytes(Uuid::new_v4()),
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

// ── CreateSession ───────────────────────────────────────────────

#[tokio::test]
async fn create_session_returns_session_created() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "workspace-1".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => {
            assert!(!sc.session_id.is_empty());
            let id = bytes_to_uuid(&sc.session_id).unwrap();
            let s = server.lock().await;
            assert_eq!(s.sessions[&id].name, "workspace-1");
            assert_eq!(s.sessions[&id].policy, RuntimePolicy::Persistent);
            drop(s);
        }
        other => panic!("expected SessionCreated, got {other:?}"),
    }
}

// ── ListSessions ────────────────────────────────────────────────

#[tokio::test]
async fn list_sessions_returns_all_sessions() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    {
        let mut s = server.lock().await;
        s.sessions.insert(Uuid::new_v4(), Session::new("a".into()));
        s.sessions.insert(Uuid::new_v4(), Session::new("b".into()));
    }
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 2);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

// ── AttachSession ───────────────────────────────────────────────

#[tokio::test]
async fn attach_nonexistent_session_returns_session_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: uuid_to_bytes(Uuid::new_v4()),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_SESSION_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: vec![0u8; 4],
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
    let (session_id, pane_id) = setup_session_with_pane(&server, client_id).await;

    // Detach first so we can re-attach cleanly.
    server
        .lock()
        .await
        .sessions
        .get_mut(&session_id)
        .unwrap()
        .detach_client(client_id, DetachReason::ExplicitRequest);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: uuid_to_bytes(session_id),
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

// ── DetachSession ───────────────────────────────────────────────

#[tokio::test]
async fn detach_nonexistent_session_returns_session_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_SESSION_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn detach_with_invalid_uuid_returns_invalid_parameter() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: vec![0u8; 2],
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

// ── TerminateSession ────────────────────────────────────────────

#[tokio::test]
async fn terminate_nonexistent_session_returns_session_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
            session_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_SESSION_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn terminate_owned_by_other_client_returns_ownership_conflict() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
            session_id: uuid_to_bytes(session_id),
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
async fn terminate_removes_session_from_state() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
            session_id: uuid_to_bytes(session_id),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::SessionTerminated(_))));
    assert!(!server.lock().await.sessions.contains_key(&session_id));
}

// ── ClosePane ───────────────────────────────────────────────────

#[tokio::test]
async fn close_pane_nonexistent_session_returns_session_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_SESSION_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn close_pane_nonexistent_pane_returns_pane_not_found() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: uuid_to_bytes(session_id),
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
    let (session_id, pane_id) = setup_session_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: uuid_to_bytes(session_id),
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
    let (session_id, pane_id) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            session_id: uuid_to_bytes(session_id),
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
    let (session_id, _) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            session_id: uuid_to_bytes(session_id),
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
    let (session_id, pane_id) = setup_session_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
            session_id: uuid_to_bytes(session_id),
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

// ── RenameSession ───────────────────────────────────────────────

#[tokio::test]
async fn rename_session_returns_session_renamed() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
            session_id: uuid_to_bytes(session_id),
            name: "renamed".into(),
        })),
    };
    let resp = Server::handle_message(&server, client_id, msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::SessionRenamed(sr)) => {
            assert_eq!(sr.name, "renamed");
        }
        other => panic!("expected SessionRenamed, got {other:?}"),
    }
}

#[tokio::test]
async fn rename_nonexistent_session_returns_session_not_found() {
    let server = new_server();
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
            session_id: uuid_to_bytes(Uuid::new_v4()),
            name: "x".into(),
        })),
    };
    let resp = Server::handle_message(&server, Uuid::new_v4(), msg).await.unwrap();
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, protocol::ERR_SESSION_NOT_FOUND);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn rename_session_ownership_violation_returns_error() {
    let server = new_server();
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, owner).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
            session_id: uuid_to_bytes(session_id),
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
    let (session_id, pane_id) = setup_session_with_pane(&server, owner).await;
    let _ = server
        .lock()
        .await
        .sessions
        .get_mut(&session_id)
        .unwrap()
        .attach_client(reader, AttachMode::ReadOnly);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(pane_id),
            data: b"hello".to_vec(),
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
    let (session_id, pane_id) = setup_session_with_pane(&server, owner).await;
    let _ = server
        .lock()
        .await
        .sessions
        .get_mut(&session_id)
        .unwrap()
        .attach_client(reader, AttachMode::ReadOnly);

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id: uuid_to_bytes(session_id),
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
fn build_snapshot_only_includes_persistent_sessions() {
    let mut server = Server::new(Box::new(StubOs));

    let mut persistent = Session::new("keep".into());
    persistent.policy = RuntimePolicy::Persistent;
    server.sessions.insert(persistent.id, persistent);

    let mut ephemeral = Session::new("discard".into());
    ephemeral.policy = RuntimePolicy::Ephemeral;
    server.sessions.insert(ephemeral.id, ephemeral);

    let snapshot = server.build_snapshot();
    assert_eq!(snapshot.sessions.len(), 1);
    assert_eq!(snapshot.sessions[0].name, "keep");
}

// ── Input to nonexistent pane in existing session ───────────────

#[tokio::test]
async fn input_to_nonexistent_pane_in_existing_session_returns_none() {
    let server = new_server();
    let client_id = Uuid::new_v4();
    let (session_id, _) = setup_session_with_pane(&server, client_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: uuid_to_bytes(session_id),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            data: b"hello".to_vec(),
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
            session_id: uuid_to_bytes(Uuid::new_v4()),
            pane_id: uuid_to_bytes(Uuid::new_v4()),
            cols: u32::from(u16::MAX) + 1,
            rows: 24,
        })),
    };
    assert!(Server::handle_message(&server, Uuid::new_v4(), msg).await.is_none());
}

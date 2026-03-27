//! Integration tests for runtime revisions and mutation acknowledgements.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;

    match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionList(list)) => list.sessions,
        other => panic!("expected SessionList, got {other:?}"),
    }
}

#[tokio::test]
async fn mutation_acks_return_monotonic_runtime_revisions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "revision-acks".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => {
            assert_eq!(created.revision, 1);
            created.session_id
        }
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    let snapshot = match client.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => snapshot,
        other => panic!("expected Snapshot, got {other:?}"),
    };
    assert_eq!(snapshot.revision, 2);

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        })
        .await;
    let pane_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::PaneCreated(created)) => {
            assert_eq!(created.revision, 3);
            created.pane_id
        }
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                cols: 100,
                rows: 30,
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::PaneResized(resized)) => {
            assert_eq!(resized.revision, 4);
            assert_eq!(resized.cols, 100);
            assert_eq!(resized.rows, 30);
        }
        other => panic!("expected PaneResized, got {other:?}"),
    }

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                title: "acked-title".into(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::TitleChanged(changed)) => {
            assert_eq!(changed.revision, 5);
            assert_eq!(changed.title, "acked-title");
        }
        other => panic!("expected TitleChanged, got {other:?}"),
    }

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::PaneClosed(closed)) => {
            assert_eq!(closed.revision, 6);
            assert_eq!(closed.pane_id, pane_id);
        }
        other => panic!("expected PaneClosed, got {other:?}"),
    }

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.clone(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionDetached(detached)) => {
            assert_eq!(detached.revision, 7);
            assert_eq!(detached.session_id, session_id);
        }
        other => panic!("expected SessionDetached, got {other:?}"),
    }
}

#[tokio::test]
async fn runtime_revision_survives_restart_and_attach_advances_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_id;

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "restart-revision".into(),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            })
            .await;
        session_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => {
                assert_eq!(created.revision, 1);
                created.session_id
            }
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    session_id: session_id.clone(),
                })),
            })
            .await;
        let pane_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(created)) => {
                assert_eq!(created.revision, 2);
                created.pane_id
            }
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                    session_id: session_id.clone(),
                    pane_id,
                    cols: 110,
                    rows: 35,
                })),
            })
            .await;
        match client.recv().await.msg {
            Some(proto::server_message::Msg::PaneResized(resized)) => {
                assert_eq!(resized.revision, 3);
            }
            other => panic!("expected PaneResized, got {other:?}"),
        }

        tokio::time::sleep(Duration::from_millis(1500)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let sessions = list_sessions(&mut client).await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].revision, 3);

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        match client.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => {
                assert_eq!(snapshot.revision, 4);
                assert_eq!(snapshot.session_id, session_id);
            }
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn failed_close_pane_returns_error_without_revision_change() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "revision-error".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::PaneCreated(created)) => {
            assert_eq!(created.revision, 2);
        }
        other => panic!("expected PaneCreated, got {other:?}"),
    }

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.clone(),
                pane_id: vec![0; 16],
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::Error(error)) => {
            assert_eq!(error.code, 6);
            assert!(error.message.contains("pane not found"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].revision, 2);
    assert_eq!(sessions[0].pane_count, 1);
}

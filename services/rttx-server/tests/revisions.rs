//! Integration tests for runtime revisions and mutation acknowledgements.

mod common;

use common::{TestClient, list_runtimes, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn mutation_acks_return_monotonic_runtime_revisions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "revision-acks".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => {
            assert_eq!(created.revision, 1);
            created.runtime_id
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
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
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
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
                runtime_id: runtime_id.clone(),
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
                runtime_id: runtime_id.clone(),
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
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
            })),
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_close = false;
    while tokio::time::Instant::now() < deadline {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneClosed(closed)) => {
                assert_eq!(closed.revision, 6);
                assert_eq!(closed.pane_id, pane_id);
                saw_close = true;
                break;
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
    assert!(saw_close, "timed out waiting for PaneClosed");

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for RuntimeDetached");
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeDetached(detached)) => {
                assert_eq!(detached.revision, 7);
                assert_eq!(detached.runtime_id, runtime_id);
                break;
            }
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn runtime_revision_survives_restart_and_attach_advances_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_id;

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                    name: "restart-revision".into(),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            })
            .await;
        runtime_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeCreated(created)) => {
                assert_eq!(created.revision, 1);
                created.runtime_id
            }
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    runtime_id: runtime_id.clone(),
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                    no_persist: None,
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
                    runtime_id: runtime_id.clone(),
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

        wait_for_state_containing(tmp.path(), "restart-revision", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let runtimes = list_runtimes(&mut client).await;
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0].revision, 3);

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                    runtime_id: runtime_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        match client.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => {
                assert_eq!(snapshot.revision, 4);
                assert_eq!(snapshot.runtime_id, runtime_id);
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
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "revision-error".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
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
                runtime_id: runtime_id.clone(),
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

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].revision, 2);
    assert_eq!(runtimes[0].pane_count, 1);
}

//! Integration tests for workspace revisions and mutation acknowledgements.

mod common;

use common::{TestClient, list_workspaces, start_test_server, wait_for_state_containing};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn mutation_acks_return_monotonic_workspace_revisions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "revision-acks".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => {
            assert_eq!(created.workspace_revision, 1);
            created.runtime_id
        }
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    let snapshot = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => snapshot,
        other => panic!("expected Snapshot, got {other:?}"),
    };
    assert_eq!(snapshot.workspace_revision, 2);

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
            })),
        })
        .await;
    let pane_id = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(created)) => {
            assert_eq!(created.workspace_revision, 3);
            created.pane_id
        }
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // ResizePane is fire-and-forget in v3 (no ack), but it still bumps the
    // workspace revision to 4. Flush it with a Ping/Pong barrier; the bump is
    // observed later via the ClosePane ack.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
                cols: 100,
                rows: 30,
            })),
        })
        .await;
    client.ping().await;

    // SetPaneTitle is also fire-and-forget; it bumps the revision to 5.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
                title: "acked-title".into(),
            })),
        })
        .await;
    client.ping().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
            })),
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_close = false;
    while tokio::time::Instant::now() < deadline {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneClosed(closed)) => {
                assert_eq!(closed.workspace_revision, 6);
                assert_eq!(closed.pane_id, pane_id);
                saw_close = true;
                break;
            }
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
    assert!(saw_close, "timed out waiting for PaneClosed");

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for WorkspaceDetached");
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceDetached(detached)) => {
                assert_eq!(detached.workspace_revision, 7);
                assert_eq!(detached.runtime_id, runtime_id);
                break;
            }
            Some(
                v3::server_envelope::Payload::OutputDelta(_)
                | v3::server_envelope::Payload::PaneExited(_),
            ) => {}
            other => panic!("expected WorkspaceDetached, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn workspace_revision_survives_restart_and_attach_advances_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_id;

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                    name: "restart-revision".into(),
                    policy: v3::WorkspacePolicy::Persistent as i32,
                })),
            })
            .await;
        runtime_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => {
                assert_eq!(created.workspace_revision, 1);
                created.runtime_id
            }
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        };

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                    runtime_id: runtime_id.clone(),
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                    no_persist: None,
                })),
            })
            .await;
        let pane_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(created)) => {
                assert_eq!(created.workspace_revision, 2);
                created.pane_id
            }
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                    runtime_id: runtime_id.clone(),
                    pane_id,
                    cols: 110,
                    rows: 35,
                })),
            })
            .await;
        // ResizePane is fire-and-forget in v3 (no ack); it bumps the revision
        // to 3. Flush it with a Ping/Pong barrier so it is persisted before the
        // server is aborted below.
        client.ping().await;

        wait_for_state_containing(tmp.path(), "restart-revision", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let workspaces = list_workspaces(&mut client).await;
        assert_eq!(workspaces.len(), 1);
        // Revision is at least 3 (persisted). Login shells may emit OSC 7
        // on startup which bumps it further — that's expected behavior.
        let pre_attach_revision = workspaces[0].workspace_revision;
        assert!(
            pre_attach_revision >= 3,
            "revision after restart should be >= 3, got {pre_attach_revision}"
        );

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                    runtime_id: runtime_id.clone(),
                    attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        match client.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
                assert_eq!(snapshot.workspace_revision, pre_attach_revision + 1);
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
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "revision-error".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
            })),
        })
        .await;
    match client.recv().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(created)) => {
            assert_eq!(created.workspace_revision, 2);
        }
        other => panic!("expected PaneCreated, got {other:?}"),
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
                runtime_id: runtime_id.clone(),
                pane_id: vec![0; 16],
            })),
        })
        .await;
    match client.recv().await.payload {
        Some(v3::server_envelope::Payload::Error(error)) => {
            assert_eq!(error.kind, v3::ErrorKind::PaneNotFound as i32);
            assert!(error.message.contains("pane not found"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].workspace_revision, 2);
    assert_eq!(workspaces[0].pane_count, 1);
}

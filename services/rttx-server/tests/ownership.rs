//! Integration tests for workspace ownership and single-writer attach semantics.

mod common;

use common::{TestClient, list_workspaces, start_test_server};
use rttx_proto::v3;

#[tokio::test]
async fn second_writer_attach_returns_attach_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "writer-conflict".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match second.recv().await.payload {
        Some(v3::server_envelope::Payload::AttachBlocked(blocked)) => {
            assert_eq!(blocked.runtime_id, runtime_id);
            assert_eq!(blocked.current_client_role, v3::WorkspaceClientRole::Unattached as i32);
            assert_eq!(blocked.read_only_client_count, 0);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }

    let workspaces = list_workspaces(&mut second).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Unattached as i32);
    assert!(workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 0);
}

#[tokio::test]
async fn read_only_attach_cannot_mutate_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "reader-denied".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.workspace_revision, 2);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.workspace_revision, 3);
            assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Reader as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    reader
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
    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::Error(error)) => {
            assert_eq!(error.kind, v3::ErrorKind::OwnershipConflict as i32);
            assert!(error.message.contains("owned by another client"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let workspaces = list_workspaces(&mut reader).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].workspace_revision, 3);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Reader as i32);
    assert!(workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 1);
}

#[tokio::test]
async fn terminate_workspace_notifies_other_attached_clients_and_removes_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "terminate-workspace".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        writer.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(
        reader.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminateWorkspace(
                v3::TerminateWorkspace { runtime_id: runtime_id.clone() },
            )),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::WorkspaceTerminationReason::Explicit as i32);
        }
        other => panic!("expected WorkspaceTerminated, got {other:?}"),
    }

    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::WorkspaceTerminationReason::Explicit as i32);
        }
        other => panic!("expected pushed WorkspaceTerminated, got {other:?}"),
    }

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let workspaces = list_workspaces(&mut third).await;
    assert!(workspaces.is_empty());
}

#[tokio::test]
async fn read_only_client_cannot_rename_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_workspace(&mut writer, "rename-denied", v3::WorkspacePolicy::Persistent)
            .await;
    common::attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: runtime_id.clone(),
                name: "hijacked".into(),

                automatic: false,
            })),
        })
        .await;
    match reader.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::OwnershipConflict as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Verify name unchanged.
    let workspaces = list_workspaces(&mut writer).await;
    assert_eq!(workspaces[0].name, "rename-denied");
}

#[tokio::test]
async fn read_only_client_cannot_set_pane_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_workspace(&mut writer, "title-denied", v3::WorkspacePolicy::Persistent)
            .await;
    common::attach_rw(&mut writer, &runtime_id).await;
    let pane_id = common::create_pane(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
                runtime_id: runtime_id.clone(),
                pane_id,
                title: "hijacked".into(),
            })),
        })
        .await;

    // SetPaneTitle is fire-and-forget; the server silently drops it for a
    // read-only client. Use a Ping/Pong barrier to flush, then confirm the
    // reader sees neither an error nor a TitleChanged carrying its title —
    // proving the title was never changed. The shell itself may set a
    // title through OSC 0 in its prompt (bash on Fedora does), so only the
    // hijacked title counts.
    reader.ping().await;
    let events = reader.drain(std::time::Duration::from_millis(200)).await;
    assert!(
        events.iter().all(|e| match &e.payload {
            Some(v3::server_envelope::Payload::Error(_)) => false,
            Some(v3::server_envelope::Payload::TitleChanged(t)) => t.title != "hijacked",
            _ => true,
        }),
        "read-only client must not be able to change the pane title"
    );
}

async fn rename(
    client: &mut TestClient,
    runtime_id: &[u8],
    name: &str,
    automatic: bool,
) -> v3::WorkspaceRenamed {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: runtime_id.to_vec(),
                name: name.into(),
                automatic,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceRenamed(r)) => return r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceRenamed, got {other:?}"),
        }
    }
}

async fn next_renamed_push(client: &mut TestClient) -> v3::WorkspaceRenamed {
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceRenamed(r)) => return r,
            Some(_) => {}
            None => panic!("connection closed before WorkspaceRenamed"),
        }
    }
}

/// The daemon owns the name: an attach snapshot carries it, and every rename —
/// automatic or user — is pushed to the other attached clients.
#[tokio::test]
async fn renames_travel_in_snapshots_and_are_pushed_to_other_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        common::create_workspace(&mut writer, "Projects", v3::WorkspacePolicy::Persistent).await;
    let snapshot = common::attach_rw(&mut writer, &runtime_id).await;
    assert_eq!(snapshot.name, "Projects");
    assert!(!snapshot.user_renamed);

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    // The writer's shell moved: it proposes an automatic name.
    let ack = rename(&mut writer, &runtime_id, "dev1_rttx", true).await;
    assert_eq!(ack.name, "dev1_rttx");
    assert!(!ack.user_renamed, "a directory-derived name is not a user rename");
    let pushed = next_renamed_push(&mut reader).await;
    assert_eq!(pushed.name, "dev1_rttx");
    assert!(!pushed.user_renamed);

    // A fresh client sees the daemon's current name in its snapshot.
    let mut late = TestClient::connect(&sock).await;
    late.handshake().await;
    let snapshot = common::attach_ro(&mut late, &runtime_id).await;
    assert_eq!(snapshot.name, "dev1_rttx");
    assert!(!snapshot.user_renamed);
}

/// Once a user names a workspace, automatic proposals lose — and the response
/// tells the proposing client what to show instead.
#[tokio::test]
async fn automatic_rename_never_overrides_a_user_rename() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        common::create_workspace(&mut writer, "Projects", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    let ack = rename(&mut writer, &runtime_id, "Blog: pipeline", false).await;
    assert_eq!(ack.name, "Blog: pipeline");
    assert!(ack.user_renamed);
    let pushed = next_renamed_push(&mut reader).await;
    assert_eq!(pushed.name, "Blog: pipeline");
    assert!(pushed.user_renamed);
    let user_revision = ack.workspace_revision;

    let ack = rename(&mut writer, &runtime_id, "pipeline", true).await;
    assert_eq!(ack.name, "Blog: pipeline", "the user's name is reported back");
    assert!(ack.user_renamed);
    assert_eq!(ack.workspace_revision, user_revision, "nothing changed, nothing bumped");

    // Nothing was pushed to the reader for the refused proposal.
    reader.ping().await;
    let events = reader.drain(std::time::Duration::from_millis(200)).await;
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.payload, Some(v3::server_envelope::Payload::WorkspaceRenamed(_)))),
        "a refused automatic rename must not be announced"
    );

    let workspaces = list_workspaces(&mut writer).await;
    assert_eq!(workspaces[0].name, "Blog: pipeline");
    assert!(workspaces[0].user_renamed);
}

/// The daemon names the workspace after its shell's directory, on its own:
/// a `cd` in the pane renames the workspace and every attached client is
/// told. A user's rename then sticks regardless of where the shell goes.
#[tokio::test]
async fn daemon_names_the_workspace_after_the_shells_directory() {
    use std::time::Duration;
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let target = tmp.path().join("zebra-project");
    std::fs::create_dir_all(&target).unwrap();

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        common::create_workspace(&mut writer, "Workspace 1", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;
    let pane_id = common::create_pane(&mut writer, &runtime_id).await;

    let mut watcher = TestClient::connect(&sock).await;
    watcher.handshake().await;
    common::attach_ro(&mut watcher, &runtime_id).await;

    // The shell moves: the daemon renames within one /proc poll, and the
    // reader hears about it without doing anything.
    common::send_input(
        &mut writer,
        &runtime_id,
        &pane_id,
        format!("cd {}\n", target.display()).as_bytes(),
    )
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut renamed_to = None;
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = watcher.try_recv(Duration::from_millis(300)).await
            && let Some(v3::server_envelope::Payload::WorkspaceRenamed(r)) = msg.payload
        {
            renamed_to = Some((r.name, r.user_renamed));
            break;
        }
    }
    assert_eq!(renamed_to, Some(("zebra-project".to_string(), false)), "daemon renames after cd");
    let listed = list_workspaces(&mut writer).await;
    assert_eq!(listed[0].name, "zebra-project");
    assert!(!listed[0].user_renamed);

    // A user rename wins, and a later cd no longer renames.
    let ack = rename(&mut writer, &runtime_id, "My Zebra", false).await;
    assert!(ack.user_renamed);
    let _ = next_renamed_push(&mut watcher).await;
    common::send_input(&mut writer, &runtime_id, &pane_id, b"cd /\n").await;
    tokio::time::sleep(Duration::from_secs(7)).await;
    let _ = watcher.drain(Duration::from_millis(300)).await;
    let listed = list_workspaces(&mut writer).await;
    assert_eq!(listed[0].name, "My Zebra", "a user's name is never replaced by a directory");
}

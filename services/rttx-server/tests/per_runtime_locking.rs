//! Verify that per-workspace locking allows independent workspaces to
//! operate without blocking each other.
//!
//! Regression test for #834.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, uuid_to_bytes, v3};

/// Create two workspaces and verify they can be operated independently.
///
/// This exercises the per-workspace lock path: creating panes in one
/// workspace must not block operations on the other.
#[tokio::test]
async fn independent_workspaces_do_not_block_each_other() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create two workspaces.
    let rt_a = create_workspace(&mut client, "workspace-a").await;
    let rt_b = create_workspace(&mut client, "workspace-b").await;
    assert_ne!(rt_a, rt_b);

    // Attach to both.
    attach_workspace(&mut client, rt_a).await;
    attach_workspace(&mut client, rt_b).await;

    // Create a pane in each — operations on independent workspaces must
    // succeed without blocking.
    let pane_a = create_pane(&mut client, rt_a).await;
    let pane_b = create_pane(&mut client, rt_b).await;
    assert_ne!(pane_a, pane_b);
}

async fn create_workspace(client: &mut TestClient, name: &str) -> uuid::Uuid {
    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: name.into(),
            policy: v3::WorkspacePolicy::Ephemeral as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(rc)) => {
            bytes_to_uuid(&rc.runtime_id).unwrap()
        }
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    }
}

async fn attach_workspace(client: &mut TestClient, runtime_id: uuid::Uuid) {
    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn create_pane(client: &mut TestClient, runtime_id: uuid::Uuid) -> uuid::Uuid {
    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: uuid_to_bytes(runtime_id),
            cwd: None,
            dark_background: None,
            cols: 80,
            rows: 24,
            no_persist: Some(true),
        })),
    };
    client.send(&msg).await;
    let resp = client.recv_or_timeout().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => bytes_to_uuid(&pc.pane_id).unwrap(),
        other => panic!("expected PaneCreated, got {other:?}"),
    }
}

//! Verify that per-runtime locking allows independent runtimes to
//! operate without blocking each other.
//!
//! Regression test for #834.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes};

/// Create two runtimes and verify they can be operated independently.
///
/// This exercises the per-runtime lock path: creating panes in one
/// runtime must not block operations on the other.
#[tokio::test]
async fn independent_runtimes_do_not_block_each_other() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create two runtimes.
    let rt_a = create_runtime(&mut client, "runtime-a").await;
    let rt_b = create_runtime(&mut client, "runtime-b").await;
    assert_ne!(rt_a, rt_b);

    // Attach to both.
    attach_runtime(&mut client, rt_a).await;
    attach_runtime(&mut client, rt_b).await;

    // Create a pane in each — operations on independent runtimes must
    // succeed without blocking.
    let pane_a = create_pane(&mut client, rt_a).await;
    let pane_b = create_pane(&mut client, rt_b).await;
    assert_ne!(pane_a, pane_b);
}

async fn create_runtime(client: &mut TestClient, name: &str) -> uuid::Uuid {
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: name.into(),
            policy: proto::RuntimePolicy::Ephemeral as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(rc)) => {
            bytes_to_uuid(&rc.runtime_id).unwrap()
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

async fn attach_runtime(client: &mut TestClient, runtime_id: uuid::Uuid) {
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn create_pane(client: &mut TestClient, runtime_id: uuid::Uuid) -> uuid::Uuid {
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
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
    match resp.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => bytes_to_uuid(&pc.pane_id).unwrap(),
        other => panic!("expected PaneCreated, got {other:?}"),
    }
}

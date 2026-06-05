//! Integration test: lifecycle events produce correct protocol responses
//! through the full server path (the same path that now emits log messages).

mod common;

use common::{
    attach_rw, close_pane, create_pane, create_runtime, detach_runtime, start_test_server,
    terminate_runtime,
};
use rttx_proto::v3;

#[tokio::test]
async fn full_lifecycle_produces_expected_responses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create → attach → create pane → close pane → detach → terminate.
    let sid =
        create_runtime(&mut client, "lifecycle-log-test", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;
    close_pane(&mut client, &sid, &pane_id).await;
    detach_runtime(&mut client, &sid).await;

    // Re-attach to terminate (need write access).
    attach_rw(&mut client, &sid).await;
    terminate_runtime(&mut client, &sid).await;

    // Session should be gone.
    let runtimes = common::list_runtimes(&mut client).await;
    assert!(runtimes.is_empty(), "session should be removed after terminate");
}

#[tokio::test]
async fn rename_runtime_through_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "before-rename", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameRuntime(v3::RenameRuntime {
                runtime_id: sid.clone(),
                name: "after-rename".into(),
            })),
        })
        .await;

    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeRenamed(sr)) => {
                assert_eq!(sr.name, "after-rename");
                break;
            }
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeRenamed, got {other:?}"),
        }
    }

    let runtimes = common::list_runtimes(&mut client).await;
    assert_eq!(runtimes[0].name, "after-rename");
}

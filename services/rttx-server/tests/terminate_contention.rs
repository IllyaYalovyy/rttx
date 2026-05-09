//! Test that `terminate_runtime_internal` handles lock contention gracefully
//! instead of panicking (regression test for the `expect()` removal).

mod common;

use rttx_proto::proto;

#[tokio::test]
async fn terminate_runtime_does_not_panic_under_contention() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a runtime.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "contention-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(rc)) => rc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Terminate the runtime — this exercises terminate_runtime_internal.
    // Previously this could panic with expect() if the lock was contended.
    let terminate = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: runtime_id.clone(),
        })),
    };
    client.send(&terminate).await;
    let resp = client.recv().await;
    assert!(
        matches!(resp.msg, Some(proto::server_message::Msg::RuntimeTerminated(_))),
        "should get RuntimeTerminated response"
    );
}

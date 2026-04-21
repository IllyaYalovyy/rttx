//! Integration tests for the `no_persist` pane flag.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn no_persist_pane_does_not_write_scrollback_to_disk() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create a persistent runtime.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "no-persist-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Create a pane with no_persist = true.
    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 80,
            rows: 24,
            no_persist: Some(true),
        })),
    };
    client.send(&create_pane).await;
    let _pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Wait for multiple serialization ticks so scrollback would be flushed
    // if the flag were not respected.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify no scrollback log was created for this pane.
    // The scrollback directory lives under cache_dir/scrollback/<runtime_id>/.
    let scrollback_dir = tmp.path().join("scrollback");
    if scrollback_dir.exists() {
        for entry in std::fs::read_dir(&scrollback_dir).unwrap().filter_map(Result::ok) {
            let runtime_dir = entry.path();
            if runtime_dir.is_dir() {
                let pane_logs: Vec<_> = std::fs::read_dir(&runtime_dir)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
                    .collect();
                assert!(
                    pane_logs.is_empty(),
                    "no_persist pane should not have scrollback logs, found: {pane_logs:?}"
                );
            }
        }
    }
}

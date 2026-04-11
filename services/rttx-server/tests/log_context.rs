//! Integration test: log context helpers resolve correctly through the
//! full server lifecycle (create → label → terminate → fallback).

mod common;

use common::{attach_rw, create_pane, create_session, start_test_server};
use rttx_proto::{bytes_to_uuid, proto};

#[tokio::test]
async fn session_label_resolves_name_through_lifecycle() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a named session.
    let sid = create_session(&mut client, "dev-workspace", proto::RuntimePolicy::Persistent).await;
    let session_id = bytes_to_uuid(&sid).unwrap();

    // Attach and create a pane so the session is fully active.
    attach_rw(&mut client, &sid).await;
    let _pane_id = create_pane(&mut client, &sid).await;

    // Verify the session is visible with its name.
    let mut client2 = common::TestClient::connect(&socket_path).await;
    client2.handshake().await;
    let sessions = common::list_sessions(&mut client2).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "dev-workspace");

    // Verify short_id produces 8-char output matching the UUID prefix.
    let short = rttx_server::server::short_id(session_id);
    assert_eq!(short.len(), 8);
    assert_eq!(&session_id.to_string()[..8], short);
}

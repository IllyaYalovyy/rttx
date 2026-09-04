//! Live daemon tests for `OPT_WORKSPACE_TAKEOVER` (#1083).
//!
//! Drives two real clients against a real server socket to prove the
//! handshake and the role broadcast: the second client seizes the write
//! lease, the first is demoted to reader and told why.

mod common;

use common::{TestClient, attach_rw, create_workspace, list_workspaces, start_test_server};
use rttx_proto::v3;

/// Every core capability plus enriched inventory, but no
/// `OPT_WORKSPACE_TAKEOVER`: the handshake succeeds, takeover does not.
const CAPS_WITHOUT_TAKEOVER: &[v3::Capability] = &[
    v3::Capability::CoreWorkspaceLifecycle,
    v3::Capability::CorePaneLifecycle,
    v3::Capability::CoreTerminalIo,
    v3::Capability::CoreTerminalModes,
    v3::Capability::CorePasteIntent,
    v3::Capability::CoreFocusEvents,
    v3::Capability::OptWorkspaceInventory,
];

fn workspace_info<'a>(
    workspaces: &'a [v3::WorkspaceInfo],
    runtime_id: &[u8],
) -> &'a v3::WorkspaceInfo {
    workspaces.iter().find(|info| info.id == runtime_id).expect("workspace missing from inventory")
}

async fn takeover(client: &mut TestClient, runtime_id: &[u8]) -> v3::ServerEnvelope {
    client
        .request(v3::client_envelope::Command::TakeoverWorkspace(v3::TakeoverWorkspace {
            runtime_id: runtime_id.to_vec(),
        }))
        .await
}

#[tokio::test]
async fn takeover_transfers_the_write_lease_and_demotes_the_previous_writer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut owner = TestClient::connect(&sock).await;
    owner.handshake().await;
    let runtime_id =
        create_workspace(&mut owner, "seizable", v3::WorkspacePolicy::Persistent).await;
    let snapshot = attach_rw(&mut owner, &runtime_id).await;
    assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Writer as i32);

    let mut challenger = TestClient::connect(&sock).await;
    challenger.handshake().await;

    let before = list_workspaces(&mut challenger).await;
    let info = workspace_info(&before, &runtime_id);
    assert!(info.has_write_owner, "the first client owns the write lease");
    assert!(info.takeover_eligible, "an owned persistent workspace can be seized");

    let reply = takeover(&mut challenger, &runtime_id).await;
    let Some(v3::server_envelope::Payload::TakeoverCompleted(completed)) = reply.payload else {
        panic!("expected TakeoverCompleted, got {:?}", reply.payload);
    };
    assert_eq!(completed.runtime_id, runtime_id);

    // The demoted writer is told it lost the lease, and to whom.
    let lease_lost = owner
        .recv_matching(|payload| matches!(payload, v3::server_envelope::Payload::LeaseLost(_)))
        .await;
    let Some(v3::server_envelope::Payload::LeaseLost(lost)) = lease_lost.payload else {
        unreachable!("filtered above");
    };
    assert_eq!(lease_lost.request_id, 0, "LeaseLost is a push event");
    assert_eq!(lost.runtime_id, runtime_id);
    assert_eq!(lost.workspace_revision, completed.workspace_revision);
    let new_owner = rttx_proto::bytes_to_uuid(&lost.new_owner_id).expect("new owner id");
    assert!(!new_owner.is_nil(), "the demoted writer learns which client seized the lease");

    // Both clients see the new role assignment.
    let owner_view = list_workspaces(&mut owner).await;
    assert_eq!(
        workspace_info(&owner_view, &runtime_id).current_client_role,
        v3::WorkspaceClientRole::Reader as i32,
        "the previous writer is now a reader",
    );
    let challenger_view = list_workspaces(&mut challenger).await;
    let challenger_info = workspace_info(&challenger_view, &runtime_id);
    assert_eq!(challenger_info.current_client_role, v3::WorkspaceClientRole::Writer as i32);
    assert!(!challenger_info.takeover_eligible, "the new owner has nothing left to seize");

    // The demotion is enforced, not merely reported.
    let rejected = owner
        .request(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
            runtime_id: runtime_id.clone(),
            name: "renamed-by-reader".into(),
        }))
        .await;
    let Some(v3::server_envelope::Payload::Error(error)) = rejected.payload else {
        panic!("a reader must not be able to rename the workspace");
    };
    assert_eq!(error.kind, v3::ErrorKind::OwnershipConflict as i32);
}

#[tokio::test]
async fn takeover_is_refused_without_the_negotiated_capability() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut owner = TestClient::connect(&sock).await;
    owner.handshake().await;
    let runtime_id = create_workspace(&mut owner, "guarded", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut owner, &runtime_id).await;

    let mut challenger = TestClient::connect(&sock).await;
    challenger.handshake_with_caps(CAPS_WITHOUT_TAKEOVER).await;

    let inventory = list_workspaces(&mut challenger).await;
    assert!(
        !workspace_info(&inventory, &runtime_id).takeover_eligible,
        "a client that cannot issue TakeoverWorkspace is never offered it",
    );

    let reply = takeover(&mut challenger, &runtime_id).await;
    let Some(v3::server_envelope::Payload::Error(error)) = reply.payload else {
        panic!("expected an error, got {:?}", reply.payload);
    };
    assert_eq!(error.kind, v3::ErrorKind::UnsupportedCapability as i32);
}

#[tokio::test]
async fn takeover_is_refused_for_an_ephemeral_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut owner = TestClient::connect(&sock).await;
    owner.handshake().await;
    let runtime_id =
        create_workspace(&mut owner, "throwaway", v3::WorkspacePolicy::Ephemeral).await;
    attach_rw(&mut owner, &runtime_id).await;

    let mut challenger = TestClient::connect(&sock).await;
    challenger.handshake().await;

    let inventory = list_workspaces(&mut challenger).await;
    assert!(!workspace_info(&inventory, &runtime_id).takeover_eligible);

    let reply = takeover(&mut challenger, &runtime_id).await;
    let Some(v3::server_envelope::Payload::Error(error)) = reply.payload else {
        panic!("expected an error, got {:?}", reply.payload);
    };
    assert_eq!(error.kind, v3::ErrorKind::OwnershipConflict as i32);

    let owner_view = list_workspaces(&mut owner).await;
    assert_eq!(
        workspace_info(&owner_view, &runtime_id).current_client_role,
        v3::WorkspaceClientRole::Writer as i32,
        "the ephemeral workspace keeps its original owner",
    );
}
